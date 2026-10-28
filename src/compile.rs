//! The app's side of the bundle pipeline: the clock and bundle IDs, decoding keyframes from a Mac
//! or iPhone movie, transcription, and `finalize_capture_bundle` for a finished Mac capture. The
//! pipeline itself is `scrawl_core::build`, re-exported here.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::audio::RecordedAudio;
use crate::keyframes::select_keyframes;
use crate::model::{
    CaptureMode, InteractionEvent, RgbaFrame, ScrawlBundle, Stroke, TargetInfo, TranscriptData,
    keyframe_budget,
};
use crate::sketch::marks_of;
use crate::{capture, helper, model, recover, speech_model, transcribe, windows};

pub use scrawl_core::build::{
    BuildOptions, FullFrames, PreparedVisuals, SourceExtras, StageTimings, merge_full_keyframes,
    prepare_visuals,
};

pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn make_bundle_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Two IDs in the same millisecond (parallel tests, overlapping ingests) must still differ.
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let ms = now_epoch_ms();
    let pid = std::process::id();
    match COUNT.fetch_add(1, Ordering::Relaxed) {
        0 => format!("scrawl-{ms}-{pid:x}"),
        n => format!("scrawl-{ms}-{pid:x}-{n}"),
    }
}

/// Keyframes at native size from a Mac screen recording, whose clock starts `offset_ms` into the
/// capture. Each frame comes back stamped with the capture time it was asked for.
pub fn movie_keyframes(movie: PathBuf, offset_ms: u64) -> FullFrames {
    movie_keyframes_with_cache(movie, offset_ms, Vec::new())
}

fn movie_keyframes_with_cache(
    movie: PathBuf,
    offset_ms: u64,
    predecoded: Vec<RgbaFrame>,
) -> FullFrames {
    let cache = std::sync::Mutex::new(predecoded);
    Box::new(move |times: &[u64]| {
        let mut pool = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::with_capacity(times.len());
        let mut missing = Vec::new();
        for &t in times {
            if let Some(idx) = pool.iter().position(|f| f.t_ms == t) {
                out.push(pool.swap_remove(idx));
            } else {
                missing.push(t);
            }
        }
        drop(pool);
        if !missing.is_empty() {
            out.extend(movie_frames_at(&movie, &missing, offset_ms, 0)?);
        }
        Ok(out)
    })
}

/// Frames of a movie whose clock starts `offset_ms` into the bundle's, at bundle times.
pub fn movie_frames_at(
    movie: &Path,
    times: &[u64],
    offset_ms: u64,
    max_edge: u32,
) -> Result<Vec<RgbaFrame>> {
    movie_frames_at_with(helper::frames, movie, times, offset_ms, max_edge)
}

fn movie_frames_at_with(
    decode: fn(&Path, helper::FrameTimes, u32) -> Result<Vec<RgbaFrame>>,
    movie: &Path,
    times: &[u64],
    offset_ms: u64,
    max_edge: u32,
) -> Result<Vec<RgbaFrame>> {
    let at: Vec<u64> = times.iter().map(|t| t.saturating_sub(offset_ms)).collect();
    let mut frames = decode(movie, helper::FrameTimes::At(&at), max_edge)?;
    // The helper stamps each frame with the time it was asked for, clamped to the movie (the
    // last keyframe can fall just past its end); map it back to the bundle time.
    for frame in &mut frames {
        if let Some(i) = (0..at.len()).min_by_key(|&i| at[i].abs_diff(frame.t_ms)) {
            frame.t_ms = times[i];
        }
    }
    Ok(frames)
}

/// Core pipeline that turns raw frames, strokes, events, and audio/transcript into a complete `.scrawl` bundle on disk.
#[allow(clippy::too_many_arguments)]
///
/// The recording's candidate frames are small; the keyframes are decoded again from its video at
/// full size, falling back to the candidates if that fails.
pub fn finalize_capture_bundle(
    bundle_dir: &Path,
    target: TargetInfo,
    recording: capture::ScreenRecording,
    strokes: Vec<Stroke>,
    events: Vec<InteractionEvent>,
    recorded_audio: Option<RecordedAudio>,
    note: Option<String>,
    activity: windows::ActivityLog,
    options: &BuildOptions,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    let started_at = activity
        .stop_started
        .unwrap_or_else(std::time::Instant::now);
    let stop_ms = activity.stop_ms;
    let capture::ScreenRecording {
        frames,
        video: video_path,
        duration_ms,
        video_offset_ms,
    } = recording;
    if let Some(ref audio) = recorded_audio {
        let wav_path = bundle_dir.join("audio.wav");
        let _ = std::fs::create_dir_all(bundle_dir);
        let _ = std::fs::write(&wav_path, &audio.wav_bytes);
    }
    let wav_path = bundle_dir.join("audio.wav");
    let has_wav = wav_path.exists();
    recover::write(
        bundle_dir,
        &recover::Unfinished {
            target: target.clone(),
            duration_ms,
            video: video_path
                .as_ref()
                .and_then(|v| v.file_name())
                .map(|name| name.to_string_lossy().into_owned()),
            video_offset_ms,
            strokes: strokes.clone(),
            events: events.clone(),
            note: note.clone(),
            windows: activity.windows.clone(),
            cursor: activity.cursor.clone(),
        },
    );
    let crop_settings = (target.mode == CaptureMode::Display)
        .then(|| options.autocrop.clone())
        .flatten();
    let ((transcript, transcribe_ms), (predecoded, prepared)) = std::thread::scope(|scope| {
        let transcribe_handle = scope.spawn(|| {
            let t0 = std::time::Instant::now();
            let tr = transcribe::transcribe(
                has_wav.then_some(wav_path.as_path()),
                recorded_audio.as_ref(),
            );
            (tr, t0.elapsed().as_millis() as u64)
        });
        let pre = if let Some(ref movie) = video_path
            && !frames.is_empty()
        {
            let mut preview_marks = marks_of(&strokes);
            let budget = keyframe_budget(duration_ms);
            let preview = select_keyframes(
                &frames,
                &strokes,
                &mut preview_marks,
                &events,
                &TranscriptData::default(),
                budget,
            );
            // Marks alone filled the budget, so speech can't change the selection.
            let marks_fill = preview.len() == budget
                && preview
                    .iter()
                    .all(|(k, _)| k.reason == model::KeyframeReason::Mark);
            let times: Vec<u64> = preview.iter().map(|(k, _)| k.t).collect();
            let decoded = movie_frames_at(movie, &times, video_offset_ms, 0).unwrap_or_default();
            if duration_ms >= 20_000 {
                let cache = if marks_fill {
                    Vec::new()
                } else {
                    decoded.clone()
                };
                let (merged, _) = merge_full_keyframes(preview, decoded);
                let vis = prepare_visuals(
                    merged,
                    frames.first(),
                    crop_settings.as_ref(),
                    &target,
                    duration_ms,
                    &strokes,
                    &preview_marks,
                    &events,
                    &activity.windows,
                    &activity.cursor,
                );
                (cache, vis)
            } else {
                (decoded, None)
            }
        } else {
            (Vec::new(), None)
        };
        (transcribe_handle.join().unwrap(), pre)
    });

    let built = build_bundle_from_parts(
        bundle_dir,
        target,
        duration_ms,
        frames,
        strokes,
        events,
        transcript,
        video_path.clone(),
        note,
        SourceExtras {
            windows: activity.windows,
            cursor: activity.cursor,
            video_offset_ms,
            autocrop: Some(crop_settings),
            full_frames: video_path
                .map(|movie| movie_keyframes_with_cache(movie, video_offset_ms, predecoded)),
            prepared,
            timings: StageTimings {
                started_at: Some(started_at),
                stop_ms,
                transcribe_ms: Some(transcribe_ms),
                ..StageTimings::default()
            },
            ..SourceExtras::default()
        },
        options,
    );
    if built.is_ok() {
        recover::finished(bundle_dir);
    }
    built
}

/// Builds a screen capture's bundle again from what `recover::write` kept: its movie and
/// `audio.wav` in `out`, transcribed and decoded afresh.
pub fn rebuild_screen_bundle(out: &Path, old: recover::Unfinished) -> Result<String> {
    rebuild_screen_bundle_with(out, old, None, &recover::Media::SYSTEM)
}

pub fn rebuild_screen_bundle_with(
    out: &Path,
    old: recover::Unfinished,
    autocrop: Option<Option<crate::autocrop::Settings>>,
    media: &recover::Media,
) -> Result<String> {
    let started_at = std::time::Instant::now();
    let video = old
        .video
        .as_ref()
        .map(|name| out.join(name))
        .filter(|p| p.is_file())
        .context("The bundle has no video")?;
    let wav = out.join("audio.wav");
    let recorded = if wav.is_file() {
        Some(crate::audio::recorded_audio_from_wav(std::fs::read(&wav)?)?)
    } else {
        None
    };
    let offset = old.video_offset_ms;
    let ((transcript, transcribe_ms), joined) = std::thread::scope(|scope| {
        let frames = scope.spawn(|| {
            let t0 = std::time::Instant::now();
            let res = (media.frames)(&video, helper::FrameTimes::Every(220), 640);
            (res, t0.elapsed().as_millis() as u64)
        });
        let t0 = std::time::Instant::now();
        let tr = (media.transcribe)(recorded.as_ref().map(|_| wav.as_path()), recorded.as_ref());
        ((tr, t0.elapsed().as_millis() as u64), frames.join())
    });
    let (frames, candidates_ms) = joined.map_err(|_| anyhow::anyhow!("Frame decoding panicked"))?;
    let mut frames = frames?;
    for frame in &mut frames {
        frame.t_ms += offset;
    }
    let (decode, movie) = (media.frames, video.clone());
    let full_frames: Option<FullFrames> = Some(Box::new(move |times: &[u64]| {
        movie_frames_at_with(decode, &movie, times, offset, 0)
    }));
    Ok(build_bundle_from_parts(
        out,
        old.target,
        old.duration_ms,
        frames,
        old.strokes,
        old.events,
        transcript,
        Some(video),
        old.note,
        SourceExtras {
            windows: old.windows,
            cursor: old.cursor,
            full_frames,
            video_offset_ms: offset,
            autocrop,
            timings: StageTimings {
                started_at: Some(started_at),
                candidates_ms: Some(candidates_ms),
                transcribe_ms: Some(transcribe_ms),
                ..Default::default()
            },
            ..SourceExtras::default()
        },
        &crate::handoff::build_options(),
    )?
    .2)
}

/// A bundle without its transcript yet (the speech model was still downloading) gets it later.
fn mark_transcript_pending(bundle_dir: &Path, bundle: &ScrawlBundle) {
    if bundle.transcript.missing.as_ref().is_some_and(|m| m.retry) {
        speech_model::mark_pending(bundle_dir);
    }
}

/// `scrawl_core::build::build_bundle_from_parts`, then the app's marker for a transcript that
/// arrives later.
#[allow(clippy::too_many_arguments)]
pub fn build_bundle_from_parts(
    bundle_dir: &Path,
    target: TargetInfo,
    duration_ms: u64,
    frames: Vec<RgbaFrame>,
    strokes: Vec<Stroke>,
    events: Vec<InteractionEvent>,
    transcript: TranscriptData,
    video_path: Option<PathBuf>,
    note: Option<String>,
    extras: SourceExtras,
    options: &BuildOptions,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    let built = scrawl_core::build::build_bundle_from_parts(
        bundle_dir,
        target,
        duration_ms,
        frames,
        strokes,
        events,
        transcript,
        video_path,
        note,
        extras,
        options,
    )?;
    mark_transcript_pending(bundle_dir, &built.1);
    Ok(built)
}
