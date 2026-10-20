//! The capture-to-bundle pipeline: selected frames, strokes, events and a transcript in, a
//! `.scrawl` bundle on disk out (`build_bundle_from_parts`, `finalize_capture_bundle`).
//! Phase 2 of `docs/library.md`; phase 3 cuts its ties to the app so it can become `scrawl-core`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::audio::RecordedAudio;
use crate::capture::write_bundle_artifacts;
use crate::keyframes::select_keyframes;
use crate::model::{
    CaptureMode, DeviceInfo, InteractionEvent, Mark, RgbaFrame, ScrawlBundle, Shape, Stroke,
    TargetInfo, TranscriptData, keyframe_budget,
};
use crate::render::render_contact_sheet;
use crate::sketch::{marks_of, summary_markdown_with};
use crate::{autocrop, capture, helper, model, render, speech_model, transcribe, windows};

/// What a build would otherwise read from `config.json`. The app makes it with
/// `handoff::build_options`; a library caller fills it in.
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    /// The `agentContext` experiment: numbered marks on shared frames and the extra context in
    /// `summary.md`.
    pub agent_context: bool,
    /// Autocrop for Display captures that don't pass their own (`SourceExtras::autocrop`);
    /// `None` leaves them uncropped.
    pub autocrop: Option<autocrop::Settings>,
}

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

/// Matches full-resolution decoded frames back to selected keyframes by timestamp (within 250ms),
/// so a single frame decode failure only leaves that one keyframe at candidate resolution.
fn merge_full_keyframes(
    selected: Vec<(model::Keyframe, RgbaFrame)>,
    full: Vec<RgbaFrame>,
) -> (Vec<(model::Keyframe, RgbaFrame)>, usize) {
    let mut pool: Vec<Option<RgbaFrame>> = full.into_iter().map(Some).collect();
    let mut matched = 0;
    let merged = selected
        .into_iter()
        .map(|(key, fallback)| {
            let best = pool
                .iter()
                .enumerate()
                .filter_map(|(idx, f)| f.as_ref().map(|frame| (idx, frame.t_ms.abs_diff(key.t))))
                .filter(|&(_, diff)| diff <= 250)
                .min_by_key(|&(_, diff)| diff)
                .map(|(idx, _)| idx);
            if let Some(idx) = best
                && let Some(mut full_frame) = pool[idx].take()
            {
                full_frame.t_ms = key.t;
                matched += 1;
                (key, full_frame)
            } else {
                (key, fallback)
            }
        })
        .collect();
    (merged, matched)
}

/// Per-stage timing breakdown logged on every finish (`finish timings: ...`).
#[derive(Clone, Copy, Debug, Default)]
pub struct StageTimings {
    pub started_at: Option<std::time::Instant>,
    pub stop_ms: Option<u64>,
    pub audio_ms: Option<u64>,
    pub candidates_ms: Option<u64>,
    pub transcribe_ms: Option<u64>,
}

/// Pre-rendered keyframe visuals and encoded PNGs computed while transcription is running.
pub struct PreparedVisuals {
    pub keyframes: Vec<model::Keyframe>,
    pub autocrop: Option<model::AutocropInfo>,
    pub contact_sheet_png: Vec<u8>,
    pub frame_pngs: Vec<(String, Vec<u8>)>,
    pub full_sheet_png: Option<Vec<u8>>,
    pub full_frame_pngs: Vec<(String, Vec<u8>)>,
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_visuals(
    selected: Vec<(model::Keyframe, RgbaFrame)>,
    baseline: Option<&RgbaFrame>,
    crop_settings: Option<&autocrop::Settings>,
    target: &TargetInfo,
    duration_ms: u64,
    strokes: &[Stroke],
    marks: &[Mark],
    events: &[InteractionEvent],
    activity: &windows::ActivityLog,
    agent_context: bool,
) -> Option<PreparedVisuals> {
    if selected.is_empty() {
        return None;
    }
    let (mut annotated_frames, mut cropped) = autocrop::annotate_and_crop(
        selected,
        baseline,
        crop_settings,
        target,
        duration_ms,
        strokes,
        marks,
        events,
        &activity.windows,
        &activity.cursor,
        agent_context,
    );
    let keyframes: Vec<model::Keyframe> =
        annotated_frames.iter().map(|(kf, _)| kf.clone()).collect();
    let (full_sheet, contact_sheet) = std::thread::scope(|scope| {
        let full_handle = cropped.as_ref().map(|(_, full)| {
            scope.spawn(|| render::render_contact_sheet_for_duration(duration_ms, full))
        });
        let sheet = render::render_contact_sheet_for_duration(duration_ms, &annotated_frames);
        (full_handle.map(|h| h.join().ok()).flatten(), sheet)
    });
    render::stamp_keyframe_badges(&mut annotated_frames);
    if let Some((_, full)) = &mut cropped {
        render::stamp_keyframe_badges(full);
    }
    let contact_sheet_png = render::encode_png(&contact_sheet).ok()?;
    let frame_pngs: Vec<(String, Vec<u8>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = annotated_frames
            .iter()
            .map(|(kf, frame)| {
                scope.spawn(move || render::encode_png(frame).ok().map(|b| (kf.file.clone(), b)))
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().ok().flatten())
            .collect::<Option<Vec<_>>>()
    })?;
    let (autocrop, full_sheet_png, full_frame_pngs) = match (cropped, full_sheet) {
        (Some((info, full)), Some(sheet)) => {
            let sheet_png = render::encode_png(&sheet).ok()?;
            let full_pngs: Vec<(String, Vec<u8>)> = std::thread::scope(|scope| {
                let workers: Vec<_> = full
                    .iter()
                    .map(|(kf, frame)| {
                        scope.spawn(move || {
                            let name = std::path::Path::new(&kf.file)
                                .file_name()
                                .and_then(|n| n.to_str())?
                                .to_owned();
                            render::encode_png(frame).ok().map(|b| (name, b))
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .map(|w| w.join().ok().flatten())
                    .collect::<Option<Vec<_>>>()
            })?;
            (Some(info), Some(sheet_png), full_pngs)
        }
        _ => (None, None, Vec::new()),
    };
    Some(PreparedVisuals {
        keyframes,
        autocrop,
        contact_sheet_png,
        frame_pngs,
        full_sheet_png,
        full_frame_pngs,
    })
}

/// What a source other than this Mac's screen adds to a bundle.
#[derive(Default)]
pub struct SourceExtras {
    pub source: Option<String>,
    pub device: Option<DeviceInfo>,
    /// What the recorded app said about itself: values at the start, their changes, and screens.
    pub context: std::collections::BTreeMap<String, String>,
    pub context_changes: Vec<model::ContextChange>,
    pub screens: Vec<model::ScreenSpan>,
    /// Screen mode's window and cursor log.
    pub activity: windows::ActivityLog,
    /// Time-only markers (ms); each becomes a `Flag` mark with keyframe priority.
    pub flags: Vec<u64>,
    /// When the video starts on the bundle's clock (ms).
    pub video_offset_ms: u64,
    /// Screen-capture auto-crop: `None` follows the config, `Some(None)` is off.
    pub autocrop: Option<Option<autocrop::Settings>>,
    /// Decodes the chosen keyframes again at full quality, for sources that select on small
    /// candidate frames (phone movies).
    pub full_frames: Option<FullFrames>,
    pub prepared: Option<PreparedVisuals>,
    pub timings: StageTimings,
}

/// Decodes frames at these times at full quality.
pub type FullFrames = Box<dyn Fn(&[u64]) -> Result<Vec<RgbaFrame>>>;

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
    let at: Vec<u64> = times.iter().map(|t| t.saturating_sub(offset_ms)).collect();
    let mut frames = helper::frames(movie, helper::FrameTimes::At(&at), max_edge)?;
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
                    &activity,
                    options.agent_context,
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

    build_bundle_from_parts(
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
            activity,
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
    )
}

/// A bundle without its transcript yet (the speech model was still downloading) gets it later.
fn mark_transcript_pending(bundle_dir: &Path, bundle: &ScrawlBundle) {
    if bundle.transcript.missing.as_ref().is_some_and(|m| m.retry) {
        speech_model::mark_pending(bundle_dir);
    }
}

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
    let total_started = extras
        .timings
        .started_at
        .unwrap_or_else(std::time::Instant::now);
    let mut marks = marks_of(&strokes);
    for (i, &t) in extras.flags.iter().enumerate() {
        marks.push(Mark {
            id: format!("f{}", i + 1),
            strokes: Vec::new(),
            t0: t,
            t1: t,
            box_norm: [0.0, 0.0, 1.0, 1.0],
            shape: Shape::Flag,
            from: None,
            to: None,
            pin: None,
            on: None,
            description: "flagged this moment".into(),
            frame: None,
        });
    }
    let bench = std::env::var_os("SCRAWL_BENCH").is_some();
    let started = std::time::Instant::now();
    let selected = select_keyframes(
        &frames,
        &strokes,
        &mut marks,
        &events,
        &transcript,
        keyframe_budget(duration_ms),
    );
    let select_only_at = std::time::Instant::now();
    let matches_prepared = extras.prepared.as_ref().is_some_and(|prep| {
        prep.keyframes.len() == selected.len()
            && prep.keyframes.iter().zip(&selected).all(|(a, (b, _))| {
                a.index == b.index
                    && a.t == b.t
                    && a.mark_id == b.mark_id
                    && a.cursor == b.cursor
                    && a.file == b.file
            })
    });
    if matches_prepared {
        drop(frames);
        let prep = extras.prepared.unwrap();
        let keyframes: Vec<model::Keyframe> = selected.into_iter().map(|(kf, _)| kf).collect();
        let selected_at = std::time::Instant::now();
        let annotated_at = selected_at;
        let rendered_at = selected_at;
        let id = bundle_dir
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scrawl-bundle")
            .to_string();
        let video_rel = video_path.and_then(|p| {
            p.exists()
                .then(|| p.file_name()?.to_str().map(str::to_owned))
                .flatten()
        });
        let audio_rel = bundle_dir
            .join("audio.wav")
            .exists()
            .then(|| "audio.wav".to_string());
        let bundle = ScrawlBundle {
            version: 1,
            id,
            created_at_ms: now_epoch_ms(),
            duration_ms,
            target,
            transcript,
            note,
            strokes,
            marks,
            events,
            keyframes,
            contact_sheet: "contact-sheet.png".into(),
            video: video_rel,
            video_offset_ms: extras.video_offset_ms,
            audio: audio_rel,
            source: extras.source,
            device: extras.device,
            context: extras.context,
            context_changes: extras.context_changes,
            screens: extras.screens,
            windows: extras.activity.windows,
            cursor: extras.activity.cursor,
            autocrop: prep.autocrop,
            destination: None,
        };
        let summary = summary_markdown_with(&bundle, bundle_dir, options.agent_context);
        std::fs::create_dir_all(bundle_dir.join("frames"))
            .with_context(|| format!("could not create {}", bundle_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(bundle_dir, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::write(
            bundle_dir.join(&bundle.contact_sheet),
            &prep.contact_sheet_png,
        )?;
        for (rel, bytes) in &prep.frame_pngs {
            std::fs::write(bundle_dir.join(rel), bytes)?;
        }
        if let (Some(info), Some(full_sheet_png)) = (&bundle.autocrop, &prep.full_sheet_png) {
            let full_dir = bundle_dir.join(&info.full_frames);
            std::fs::create_dir_all(&full_dir)?;
            std::fs::write(bundle_dir.join(&info.full_contact_sheet), full_sheet_png)?;
            for (name, bytes) in &prep.full_frame_pngs {
                std::fs::write(full_dir.join(name), bytes)?;
            }
        }
        let manifest_bytes = serde_json::to_vec_pretty(&bundle)?;
        std::fs::write(bundle_dir.join("scrawl.json"), manifest_bytes)?;
        std::fs::write(bundle_dir.join("summary.md"), summary.as_bytes())?;
        let finished = std::time::Instant::now();
        let select_ms = (select_only_at - started).as_millis() as u64;
        let decode_ms = (selected_at - select_only_at).as_millis() as u64;
        let autocrop_ms = (annotated_at - selected_at).as_millis() as u64;
        let sheet_ms = (rendered_at - annotated_at).as_millis() as u64;
        let write_ms = (finished - rendered_at).as_millis() as u64;
        let total_ms = (finished - total_started).as_millis() as u64;
        let mut parts = Vec::with_capacity(10);
        if let Some(ms) = extras.timings.stop_ms {
            parts.push(format!("stop {ms}ms"));
        }
        if let Some(ms) = extras.timings.audio_ms {
            parts.push(format!("audio {ms}ms"));
        }
        if let Some(ms) = extras.timings.candidates_ms {
            parts.push(format!("candidates {ms}ms"));
        }
        if let Some(ms) = extras.timings.transcribe_ms {
            parts.push(format!("transcribe {ms}ms"));
        }
        parts.extend([
            format!("select {select_ms}ms"),
            format!("decode {decode_ms}ms"),
            format!("autocrop {autocrop_ms}ms"),
            format!("sheet {sheet_ms}ms"),
            format!("write {write_ms}ms"),
            format!("total {total_ms}ms"),
        ]);
        capture::scrawl_log(format!("finish timings: {}", parts.join(" · ")));
        mark_transcript_pending(bundle_dir, &bundle);
        return Ok((bundle_dir.to_path_buf(), bundle, summary));
    }
    // Only the first candidate is needed from here on (auto-crop's baseline); free the rest
    // before the full-size keyframes arrive.
    let baseline = {
        let mut frames = frames;
        frames.truncate(1);
        frames.pop()
    };
    let selected = match &extras.full_frames {
        Some(decode) if !selected.is_empty() => {
            let times: Vec<u64> = selected.iter().map(|(k, _)| k.t).collect();
            match decode(&times) {
                Ok(full) => {
                    let total = selected.len();
                    let (merged, matched) = merge_full_keyframes(selected, full);
                    if matched < total {
                        capture::scrawl_log(format!(
                            "Full-quality keyframes: decoded {matched} of {total}"
                        ));
                    }
                    merged
                }
                Err(error) => {
                    capture::scrawl_log(format!("Full-quality keyframes failed: {error:#}"));
                    selected
                }
            }
        }
        _ => selected,
    };
    let selected_at = std::time::Instant::now();

    let crop_settings = match extras.autocrop {
        Some(settings) => settings,
        None if target.mode == CaptureMode::Display => options.autocrop.clone(),
        None => None,
    };
    let (mut annotated_frames, mut cropped) = autocrop::annotate_and_crop(
        selected,
        baseline.as_ref(),
        crop_settings.as_ref(),
        &target,
        duration_ms,
        &strokes,
        &marks,
        &events,
        &extras.activity.windows,
        &extras.activity.cursor,
        options.agent_context,
    );
    let keyframes: Vec<model::Keyframe> =
        annotated_frames.iter().map(|(kf, _)| kf.clone()).collect();
    let annotated_at = std::time::Instant::now();

    let id = bundle_dir
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scrawl-bundle")
        .to_string();

    let video_rel = video_path.and_then(|p| {
        p.exists()
            .then(|| p.file_name()?.to_str().map(str::to_owned))
            .flatten()
    });
    let audio_rel = bundle_dir
        .join("audio.wav")
        .exists()
        .then(|| "audio.wav".to_string());

    let bundle = ScrawlBundle {
        version: 1,
        id,
        created_at_ms: now_epoch_ms(),
        duration_ms,
        target,
        transcript,
        note,
        strokes,
        marks,
        events,
        keyframes,
        contact_sheet: "contact-sheet.png".into(),
        video: video_rel,
        video_offset_ms: extras.video_offset_ms,
        audio: audio_rel,
        source: extras.source,
        device: extras.device,
        context: extras.context,
        context_changes: extras.context_changes,
        screens: extras.screens,
        windows: extras.activity.windows,
        cursor: extras.activity.cursor,
        autocrop: cropped.as_ref().map(|(info, _)| info.clone()),
        destination: None,
    };

    let (full_sheet, contact_sheet) = std::thread::scope(|scope| {
        let full_handle = cropped
            .as_ref()
            .map(|(_, full)| scope.spawn(|| render_contact_sheet(&bundle, full)));
        let sheet = render_contact_sheet(&bundle, &annotated_frames);
        (full_handle.map(|h| h.join().unwrap()), sheet)
    });
    render::stamp_keyframe_badges(&mut annotated_frames);
    if let Some((_, full)) = &mut cropped {
        render::stamp_keyframe_badges(full);
    }
    let rendered_at = std::time::Instant::now();
    let summary = summary_markdown_with(&bundle, bundle_dir, options.agent_context);
    std::thread::scope(|scope| -> Result<()> {
        let full_write = match (&cropped, &full_sheet) {
            (Some((info, full)), Some(sheet)) => {
                Some(scope.spawn(|| autocrop::write_full_frames(bundle_dir, info, full, sheet)))
            }
            _ => None,
        };
        write_bundle_artifacts(
            bundle_dir,
            &bundle,
            &annotated_frames,
            &contact_sheet,
            &summary,
        )?;
        if let Some(handle) = full_write {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Full-frame writer panicked"))??;
        }
        Ok(())
    })?;
    let finished = std::time::Instant::now();
    let select_ms = (select_only_at - started).as_millis() as u64;
    let decode_ms = (selected_at - select_only_at).as_millis() as u64;
    let autocrop_ms = (annotated_at - selected_at).as_millis() as u64;
    let sheet_ms = (rendered_at - annotated_at).as_millis() as u64;
    let write_ms = (finished - rendered_at).as_millis() as u64;
    let total_ms = (finished - total_started).as_millis() as u64;
    let mut parts = Vec::with_capacity(10);
    if let Some(ms) = extras.timings.stop_ms {
        parts.push(format!("stop {ms}ms"));
    }
    if let Some(ms) = extras.timings.audio_ms {
        parts.push(format!("audio {ms}ms"));
    }
    if let Some(ms) = extras.timings.candidates_ms {
        parts.push(format!("candidates {ms}ms"));
    }
    if let Some(ms) = extras.timings.transcribe_ms {
        parts.push(format!("transcribe {ms}ms"));
    }
    parts.extend([
        format!("select {select_ms}ms"),
        format!("decode {decode_ms}ms"),
        format!("autocrop {autocrop_ms}ms"),
        format!("sheet {sheet_ms}ms"),
        format!("write {write_ms}ms"),
        format!("total {total_ms}ms"),
    ]);
    capture::scrawl_log(format!("finish timings: {}", parts.join(" · ")));
    if bench {
        for (name, elapsed) in [
            ("selection_ms", selected_at - started),
            ("annotation_ms", annotated_at - selected_at),
            ("contact_sheet_ms", rendered_at - annotated_at),
            ("artifacts_ms", finished - rendered_at),
            ("pipeline_ms", finished - started),
        ] {
            eprintln!("BENCH {name}={:.3}", elapsed.as_secs_f64() * 1000.0);
        }
    }

    mark_transcript_pending(bundle_dir, &bundle);
    Ok((bundle_dir.to_path_buf(), bundle, summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_keyframes_match_by_time_and_preserve_partial_decodes() {
        let kf = |index: usize, t: u64| model::Keyframe {
            index,
            t,
            reason: model::KeyframeReason::Click,
            file: format!("frames/frame-{index:02}.png"),
            event: "tap".into(),
            said: None,
            mark_id: None,
            cursor: None,
        };
        let small = |t: u64| RgbaFrame::new(t, 32, 32, vec![10; 32 * 32 * 4]);
        let large = |t: u64| RgbaFrame::new(t, 160, 160, vec![200; 160 * 160 * 4]);

        let selected = vec![
            (kf(1, 1_000), small(1_000)),
            (kf(2, 2_000), small(2_000)),
            (kf(3, 3_000), small(3_000)),
        ];
        // Frame at 2_000 failed to decode and frames arrived out of order (3_000 clamped to 2_999).
        let full = vec![large(2_999), large(1_000)];
        let (merged, matched) = merge_full_keyframes(selected, full);
        assert_eq!(matched, 2);
        assert_eq!(merged[0].1.width, 160);
        assert_eq!(merged[0].1.t_ms, 1_000);
        assert_eq!(merged[1].1.width, 32);
        assert_eq!(merged[1].1.t_ms, 2_000);
        assert_eq!(merged[2].1.width, 160);
        assert_eq!(merged[2].1.t_ms, 3_000);
    }
}
