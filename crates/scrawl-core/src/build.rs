//! The bundle pipeline: a capture's candidate frames, ink, events and transcript in, a `.scrawl`
//! bundle on disk out (`build_bundle_from_parts`). Recording, transcription and video decoding
//! stay with the caller (the app's `compile.rs`), which passes the results in.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::keyframes::select_keyframes;
use crate::model::{
    CaptureMode, CursorSample, DeviceInfo, InteractionEvent, Mark, RgbaFrame, ScrawlBundle, Shape,
    Stroke, TargetInfo, TranscriptData, WindowSnapshot, keyframe_budget,
};
use crate::render::render_contact_sheet;
use crate::sketch::{marks_of, summary_markdown_with};
use crate::{autocrop, model, render};

/// What a build would otherwise read from `config.json`, the clock or the app's log. The app
/// makes it with `handoff::build_options`; a library caller fills it in.
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    /// The `agentContext` experiment: numbered marks on shared frames and the extra context in
    /// `summary.md`.
    pub agent_context: bool,
    /// Autocrop for Display captures that don't pass their own (`SourceExtras::autocrop`);
    /// `None` leaves them uncropped.
    pub autocrop: Option<autocrop::Settings>,
    /// `scrawl.json`'s `createdAtMs` (epoch milliseconds).
    pub created_at_ms: u64,
    /// Where progress lines go (`finish timings: …`, keyframe decode problems); `None` drops them.
    pub logger: Option<fn(&str)>,
}

impl BuildOptions {
    fn log(&self, message: impl AsRef<str>) {
        if let Some(logger) = self.logger {
            logger(message.as_ref());
        }
    }
}

/// Matches full-resolution decoded frames back to selected keyframes by timestamp (within 250ms),
/// so a single frame decode failure only leaves that one keyframe at candidate resolution.
pub fn merge_full_keyframes(
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
    windows: &[WindowSnapshot],
    cursor: &[CursorSample],
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
        windows,
        cursor,
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
    pub windows: Vec<WindowSnapshot>,
    pub cursor: Vec<CursorSample>,
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

/// Builds a `.scrawl` bundle in `bundle_dir` from a capture's candidate frames, ink, events and
/// transcript: picks keyframes, annotates and crops them, renders the contact sheet and writes
/// `frames/`, `contact-sheet.png`, `scrawl.json` and `summary.md`.
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
    mut extras: SourceExtras,
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
    // Visuals rendered while transcription ran are used when they were made for the same keyframes.
    let prepared = extras.prepared.take().filter(|prep| {
        prep.keyframes.len() == selected.len()
            && prep.keyframes.iter().zip(&selected).all(|(a, (b, _))| {
                a.index == b.index
                    && a.t == b.t
                    && a.mark_id == b.mark_id
                    && a.cursor == b.cursor
                    && a.file == b.file
            })
    });
    let (keyframes, visuals, selected_at, annotated_at) = match prepared {
        Some(prep) => {
            drop(frames);
            let keyframes = selected.into_iter().map(|(kf, _)| kf).collect();
            let at = std::time::Instant::now();
            (keyframes, Visuals::Prepared(prep), at, at)
        }
        None => {
            // Only the first candidate is needed from here on (auto-crop's baseline); free the
            // rest before the full-size keyframes arrive.
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
                                options.log(format!(
                                    "Full-quality keyframes: decoded {matched} of {total}"
                                ));
                            }
                            merged
                        }
                        Err(error) => {
                            options.log(format!("Full-quality keyframes failed: {error:#}"));
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
            let (annotated, cropped) = autocrop::annotate_and_crop(
                selected,
                baseline.as_ref(),
                crop_settings.as_ref(),
                &target,
                duration_ms,
                &strokes,
                &marks,
                &events,
                &extras.windows,
                &extras.cursor,
                options.agent_context,
            );
            let keyframes = annotated.iter().map(|(kf, _)| kf.clone()).collect();
            let annotated_at = std::time::Instant::now();
            let visuals = Visuals::Rendered { annotated, cropped };
            (keyframes, visuals, selected_at, annotated_at)
        }
    };

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
    let autocrop = match &visuals {
        Visuals::Prepared(prep) => prep.autocrop.clone(),
        Visuals::Rendered { cropped, .. } => cropped.as_ref().map(|(info, _)| info.clone()),
    };
    let bundle = ScrawlBundle {
        version: 1,
        id,
        created_at_ms: options.created_at_ms,
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
        windows: extras.windows,
        cursor: extras.cursor,
        autocrop,
        destination: None,
    };

    let (summary, rendered_at) = match visuals {
        Visuals::Prepared(prep) => {
            let summary = summary_markdown_with(&bundle, bundle_dir, options.agent_context);
            write_prepared(bundle_dir, &bundle, &prep, &summary)?;
            (summary, annotated_at)
        }
        Visuals::Rendered {
            mut annotated,
            mut cropped,
        } => {
            let (full_sheet, contact_sheet) = std::thread::scope(|scope| {
                let full_handle = cropped
                    .as_ref()
                    .map(|(_, full)| scope.spawn(|| render_contact_sheet(&bundle, full)));
                let sheet = render_contact_sheet(&bundle, &annotated);
                (full_handle.map(|h| h.join().unwrap()), sheet)
            });
            render::stamp_keyframe_badges(&mut annotated);
            if let Some((_, full)) = &mut cropped {
                render::stamp_keyframe_badges(full);
            }
            let rendered_at = std::time::Instant::now();
            let summary = summary_markdown_with(&bundle, bundle_dir, options.agent_context);
            std::thread::scope(|scope| -> Result<()> {
                let full_write = match (&cropped, &full_sheet) {
                    (Some((info, full)), Some(sheet)) => Some(
                        scope.spawn(|| autocrop::write_full_frames(bundle_dir, info, full, sheet)),
                    ),
                    _ => None,
                };
                write_bundle_artifacts(bundle_dir, &bundle, &annotated, &contact_sheet, &summary)?;
                if let Some(handle) = full_write {
                    handle
                        .join()
                        .map_err(|_| anyhow::anyhow!("Full-frame writer panicked"))??;
                }
                Ok(())
            })?;
            (summary, rendered_at)
        }
    };
    log_timings(
        options,
        &extras.timings,
        Stages {
            total_started,
            started,
            select_only_at,
            selected_at,
            annotated_at,
            rendered_at,
            finished: std::time::Instant::now(),
        },
    );
    Ok((bundle_dir.to_path_buf(), bundle, summary))
}

/// A build's keyframe images: rendered ahead of time (`SourceExtras::prepared`), or annotated
/// and cropped by this build and rendered at the end.
enum Visuals {
    Prepared(PreparedVisuals),
    Rendered {
        annotated: Vec<(model::Keyframe, RgbaFrame)>,
        cropped: Option<(model::AutocropInfo, Vec<(model::Keyframe, RgbaFrame)>)>,
    },
}

/// Writes a bundle whose PNGs were encoded ahead of time.
fn write_prepared(
    bundle_dir: &Path,
    bundle: &ScrawlBundle,
    prep: &PreparedVisuals,
    summary: &str,
) -> Result<()> {
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
    let manifest_bytes = serde_json::to_vec_pretty(bundle)?;
    std::fs::write(bundle_dir.join("scrawl.json"), manifest_bytes)?;
    std::fs::write(bundle_dir.join("summary.md"), summary.as_bytes())?;
    Ok(())
}

/// When each stage of a build ended.
struct Stages {
    total_started: std::time::Instant,
    started: std::time::Instant,
    select_only_at: std::time::Instant,
    selected_at: std::time::Instant,
    annotated_at: std::time::Instant,
    rendered_at: std::time::Instant,
    finished: std::time::Instant,
}

/// The `finish timings: …` log line, and `BENCH` lines on stderr with `SCRAWL_BENCH` set.
fn log_timings(options: &BuildOptions, timings: &StageTimings, s: Stages) {
    let ms = |d: std::time::Duration| d.as_millis() as u64;
    let mut parts = Vec::with_capacity(10);
    if let Some(ms) = timings.stop_ms {
        parts.push(format!("stop {ms}ms"));
    }
    if let Some(ms) = timings.audio_ms {
        parts.push(format!("audio {ms}ms"));
    }
    if let Some(ms) = timings.candidates_ms {
        parts.push(format!("candidates {ms}ms"));
    }
    if let Some(ms) = timings.transcribe_ms {
        parts.push(format!("transcribe {ms}ms"));
    }
    parts.extend([
        format!("select {}ms", ms(s.select_only_at - s.started)),
        format!("decode {}ms", ms(s.selected_at - s.select_only_at)),
        format!("autocrop {}ms", ms(s.annotated_at - s.selected_at)),
        format!("sheet {}ms", ms(s.rendered_at - s.annotated_at)),
        format!("write {}ms", ms(s.finished - s.rendered_at)),
        format!("total {}ms", ms(s.finished - s.total_started)),
    ]);
    options.log(format!("finish timings: {}", parts.join(" · ")));
    if std::env::var_os("SCRAWL_BENCH").is_some() {
        for (name, elapsed) in [
            ("selection_ms", s.selected_at - s.started),
            ("annotation_ms", s.annotated_at - s.selected_at),
            ("contact_sheet_ms", s.rendered_at - s.annotated_at),
            ("artifacts_ms", s.finished - s.rendered_at),
            ("pipeline_ms", s.finished - s.started),
        ] {
            eprintln!("BENCH {name}={:.3}", elapsed.as_secs_f64() * 1000.0);
        }
    }
}

/// Writes all bundle artifacts (`scrawl.json`, `summary.md`, `contact-sheet.png`, `frames/frame-XX.png`)
/// into a new or specified `.scrawl` bundle directory.
pub fn write_bundle_artifacts(
    out_dir: &Path,
    bundle: &crate::model::ScrawlBundle,
    annotated_frames: &[(crate::model::Keyframe, RgbaFrame)],
    contact_sheet: &RgbaFrame,
    summary_md: &str,
) -> Result<()> {
    std::fs::create_dir_all(out_dir.join("frames"))
        .with_context(|| format!("could not create {}", out_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(out_dir, std::fs::Permissions::from_mode(0o700));
    }

    std::thread::scope(|scope| -> Result<()> {
        let workers: Vec<_> = annotated_frames
            .iter()
            .map(|(kf, frame)| {
                scope.spawn(move || -> Result<()> {
                    crate::render::save_png(frame, &out_dir.join(&kf.file))
                })
            })
            .collect();
        crate::render::save_png(contact_sheet, &out_dir.join(&bundle.contact_sheet))?;
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("PNG encoding worker panicked"))??;
        }
        Ok(())
    })?;

    let manifest_bytes = serde_json::to_vec_pretty(bundle)?;
    std::fs::write(out_dir.join("scrawl.json"), manifest_bytes)?;
    std::fs::write(out_dir.join("summary.md"), summary_md.as_bytes())?;

    Ok(())
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
