//! The macOS screen recorder: ScreenCaptureKit records the movie and takes the candidate
//! frames, leaving out the recorder's own overlay windows and, when it can, macOS's own prompts.
//! `run_sampler` is what `screen::Session::start` runs on macOS.
//!
//! ScreenCaptureKit is reached through Swift, so a binary that uses this needs the Swift runtime
//! on its rpath (`-Wl,-rpath,/usr/lib/swift`, as this crate's `build.rs` adds for its tests).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::activity::system_prompts_over;
use crate::model::{CaptureMode, RgbaFrame, TargetInfo};
use crate::native::SamplerHooks;
use crate::native::window_server::raw_windows;
use crate::screen::{
    CANDIDATE_EDGE, CANDIDATE_EDGE_NO_VIDEO, FRAME_SAMPLE_INTERVAL, Sampler, capture_size,
    source_rect,
};

/// A recording is at most the display's backing scale (2 on Retina).
pub const MAX_CAPTURE_SCALE: f64 = 2.0;

/// Whether this process may record the screen (`CGPreflightScreenCaptureAccess`).
pub fn screen_capture_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// Records `sampler`'s target until it stops: the movie (when `video_output_path` is set) and
/// candidate frames, at the buffer's interval or at once when `urgent`. Returns the movie.
pub fn run_sampler(sampler: Sampler, hooks: SamplerHooks) -> Option<PathBuf> {
    let Sampler {
        target: sampler_target,
        video_output_path,
        excluded_window_ids,
        started_at,
        stopping: thread_stopping,
        urgent: thread_urgent,
        frames: thread_frames,
        video_offset_ms: thread_offset,
    } = sampler;
    let initial_excluded = excluded_window_ids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    // Keep SCK initialization, capture and teardown on the owning worker.
    // stop() joins even if it arrives while setup is still in progress.
    let (stream, recording_output, video_path, filter) = setup_sck_capture(
        &sampler_target,
        video_output_path,
        &initial_excluded,
        started_at,
        &thread_offset,
        hooks,
    );
    if let Some(ready) = hooks.ready {
        ready(started_at);
    }
    let shot_edge = if video_path.is_some() {
        CANDIDATE_EDGE
    } else {
        CANDIDATE_EDGE_NO_VIDEO
    };
    let shot_config = sck_config(&sampler_target, shot_edge);
    let mut filter_and_config = filter.map(|f| (f, shot_config));
    let mut last_sample = Instant::now()
        .checked_sub(FRAME_SAMPLE_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut phase = 0_u32;
    let mut last_excluded = initial_excluded.clone();
    let mut prompts = SystemPrompts::new(&sampler_target, hooks);
    while !thread_stopping.load(Ordering::Acquire) {
        let mut cur_excluded = excluded_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if prompts.check(started_at) {
            // Frames since the last look may show the prompt.
            thread_frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drop_since(prompts.clear_until_ms);
        }
        cur_excluded.extend(&prompts.ids);
        if cur_excluded != last_excluded {
            last_excluded.clone_from(&cur_excluded);
            prompts.hidden = sck_lists_windows(&prompts.ids);
            if let Some((new_filter, _)) =
                build_sck_filter_and_config(&sampler_target, &cur_excluded, hooks)
            {
                // The recording must hide the same windows as the candidates, or
                // keyframes decoded from it could show Scrawl's own overlays.
                if let Some(ref stream) = stream
                    && let Err(error) = stream.update_content_filter(&new_filter)
                {
                    hooks.log(&format!("Recording filter update failed: {error:?}"));
                }
                filter_and_config = Some((new_filter, sck_config(&sampler_target, shot_edge)));
            }
        }
        let urgent = thread_urgent.swap(false, Ordering::AcqRel);
        let interval = thread_frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .interval();
        // A system prompt ScreenCaptureKit can't leave out: no candidates until
        // it's answered.
        if prompts.showing() {
            thread::park_timeout(Duration::from_millis(30));
            continue;
        }
        if urgent || last_sample.elapsed() >= interval {
            last_sample = Instant::now();
            let t_ms = started_at.elapsed().as_millis() as u64;
            if urgent {
                phase = phase.saturating_add(1);
            }
            let frame = if let Some((ref filter, ref config)) = filter_and_config {
                capture_sck_frame(filter, config, t_ms)
                    .unwrap_or_else(|| (hooks.placeholder)(t_ms, phase))
            } else {
                (hooks.placeholder)(t_ms, phase)
            };
            thread_frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(frame, urgent);
        }
        thread::park_timeout(Duration::from_millis(30));
    }
    if let Some(stream) = stream {
        let _ = stream.stop_capture();
        if let Some(ref output) = recording_output {
            let _ = stream.remove_recording_output(output);
        }
    }
    video_path
}

fn build_sck_filter_and_config(
    target: &TargetInfo,
    excluded_window_ids: &[u32],
    hooks: SamplerHooks,
) -> Option<(SCContentFilter, SCStreamConfiguration)> {
    let content = match SCShareableContent::get() {
        Ok(c) => c,
        Err(err) => {
            // Screen Recording is asked for in onboarding, never mid-capture.
            hooks.log(&format!(
                "SCShareableContent::get() failed: {err:?} (screen recording granted: {})",
                screen_capture_granted()
            ));
            return None;
        }
    };
    let our_pid = std::process::id() as i32;

    let filter = if let Some(window_id) = target.window_id
        && let Some(window) = content
            .windows()
            .into_iter()
            .find(|w| w.window_id() == window_id)
    {
        SCContentFilter::create().with_window(&window).build().ok()
    } else if let Some(display) = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == target.display_id)
        .or_else(|| content.displays().into_iter().next())
    {
        // Exclude Scrawl's floating HUD pill and Metal overlay windows by explicit window ID
        // (and any non-layer-0 Scrawl panels), while keeping Scrawl's normal Review window visible.
        let excluded_overlays: Vec<SCWindow> = content
            .windows()
            .into_iter()
            .filter(|w| {
                excluded_window_ids.contains(&w.window_id())
                    || (w
                        .owning_application()
                        .as_ref()
                        .is_some_and(|a| a.process_id() == our_pid)
                        && w.window_layer() != 0)
            })
            .collect();
        let excluded_refs: Vec<&SCWindow> = excluded_overlays.iter().collect();
        SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&excluded_refs)
            .build()
            .ok()
    } else {
        None
    }?;

    // Full-screen captures get auto-cropped to a window or region afterwards (`scrawl autocrop`),
    // so they need enough pixels for a crop to a quarter of the screen to stay sharp.
    let long_edge = if target.mode == CaptureMode::Display {
        2560.0
    } else {
        1280.0
    };
    Some((filter, sck_config(target, long_edge)))
}

fn sck_config(target: &TargetInfo, long_edge: f64) -> SCStreamConfiguration {
    let (width, height) = capture_size(target, long_edge, MAX_CAPTURE_SCALE);
    let mut config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_shows_cursor(true);
    if target.mode == CaptureMode::Region {
        // `sourceRect` is in the display's own points, not global ones.
        let b = core_graphics::display::CGDisplay::new(target.display_id).bounds();
        let display = [b.origin.x, b.origin.y, b.size.width, b.size.height];
        let area = [
            target.frame.x,
            target.frame.y,
            target.frame.width,
            target.frame.height,
        ];
        let [x, y, w, h] = if display[2] > 0.0 && display[3] > 0.0 {
            source_rect(area, display)
        } else {
            area
        };
        config.set_source_rect(CGRect::new(x, y, w, h));
    }
    config
}

/// Starts the recording stream; returns it with the filter for candidate screenshots. Stores
/// when the recording started (ms since `started_at`) into `video_offset_ms`.
fn setup_sck_capture(
    target: &TargetInfo,
    video_output_path: Option<PathBuf>,
    excluded_window_ids: &[u32],
    started_at: Instant,
    video_offset_ms: &AtomicU64,
    hooks: SamplerHooks,
) -> (
    Option<SCStream>,
    Option<SCRecordingOutput>,
    Option<PathBuf>,
    Option<SCContentFilter>,
) {
    let Some((filter, config)) = build_sck_filter_and_config(target, excluded_window_ids, hooks)
    else {
        return (None, None, None, None);
    };

    if let Some(video_path) = video_output_path {
        if let Ok(rec_cfg_builder) = SCRecordingOutputConfiguration::new()
            && let Ok(rec_cfg) = rec_cfg_builder.with_output_url(&video_path).map(|c| {
                c.with_video_codec(SCRecordingOutputCodec::H264)
                    .with_output_file_type(SCRecordingOutputFileType::MP4)
            })
            && let Some(rec_output) = SCRecordingOutput::new(&rec_cfg)
            && let Ok(stream) = SCStream::new(&filter, &config)
            && stream.add_recording_output(&rec_output).is_ok()
        {
            // The movie's clock starts while `start_capture` runs; its midpoint is the closest
            // estimate this side of the recording delegate.
            let before = started_at.elapsed();
            if stream.start_capture().is_ok() {
                let after = started_at.elapsed();
                video_offset_ms.store(((before + after) / 2).as_millis() as u64, Ordering::Release);
                return (
                    Some(stream),
                    Some(rec_output),
                    Some(video_path),
                    Some(filter),
                );
            }
        }
    }

    (None, None, None, Some(filter))
}

fn capture_sck_frame(
    filter: &SCContentFilter,
    config: &SCStreamConfiguration,
    t_ms: u64,
) -> Option<RgbaFrame> {
    let image = SCScreenshotManager::capture_image(filter, config).ok()?;
    let width = u32::try_from(image.width()).ok()?;
    let height = u32::try_from(image.height()).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let rgba = image.rgba_data().ok()?;
    if rgba.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    Some(RgbaFrame::new(t_ms, width, height, rgba))
}

/// macOS prompts over a capture (`activity::system_prompts_over`): ScreenCaptureKit's "bypass
/// the system private window picker" question shows a second or two after the first capture
/// starts. They're left out of the recording when ScreenCaptureKit lists them, and frames that
/// could show one are never candidates, so a prompt doesn't become a keyframe.
struct SystemPrompts {
    hooks: SamplerHooks,
    frame: Option<crate::model::TargetFrame>,
    ids: Vec<u32>,
    /// Which of `ids` the content filter leaves out.
    hidden: Vec<u32>,
    checked: Option<Instant>,
    /// The capture time of the last look that found nothing new.
    clear_until_ms: u64,
}

impl SystemPrompts {
    fn new(target: &TargetInfo, hooks: SamplerHooks) -> Self {
        Self {
            hooks,
            // A single window's capture never shows a prompt.
            frame: target.window_id.is_none().then_some(target.frame),
            ids: Vec::new(),
            hidden: Vec::new(),
            checked: None,
            clear_until_ms: 0,
        }
    }

    /// Looks again (four times a second for the first 10 s, then every second). Returns whether
    /// a prompt appeared since the last look.
    fn check(&mut self, started_at: Instant) -> bool {
        let Some(frame) = self.frame else {
            return false;
        };
        let elapsed = started_at.elapsed();
        let every = if elapsed < Duration::from_secs(10) {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        };
        if self.checked.is_some_and(|at| at.elapsed() < every) {
            return false;
        }
        self.checked = Some(Instant::now());
        let raw = objc2::rc::autoreleasepool(|_| raw_windows());
        let ids = system_prompts_over(&raw, frame);
        let appeared = ids.iter().any(|id| !self.ids.contains(id));
        if appeared {
            self.hooks
                .log(&format!("system prompt over the capture: windows {ids:?}"));
        } else {
            self.clear_until_ms = elapsed.as_millis() as u64;
        }
        self.ids = ids;
        appeared
    }

    /// A prompt is on screen that the recording can't leave out.
    fn showing(&self) -> bool {
        self.ids.iter().any(|id| !self.hidden.contains(id))
    }
}

/// Which of `ids` ScreenCaptureKit lists (and so can leave out of a recording).
fn sck_lists_windows(ids: &[u32]) -> Vec<u32> {
    if ids.is_empty() {
        return Vec::new();
    }
    SCShareableContent::get()
        .map(|content| {
            content
                .windows()
                .into_iter()
                .map(|w| w.window_id())
                .filter(|id| ids.contains(id))
                .collect()
        })
        .unwrap_or_default()
}
