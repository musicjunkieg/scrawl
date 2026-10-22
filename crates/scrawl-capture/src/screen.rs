//! The screen: one recording in progress and what it leaves behind.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::model::{RgbaFrame, TargetInfo};

pub const FRAME_SAMPLE_INTERVAL: Duration = Duration::from_millis(220);
/// Candidate frames only feed scene diffs and keyframe selection; the keyframes themselves are
/// decoded from `recording.mp4` at full size afterwards.
pub const CANDIDATE_EDGE: f64 = 480.0;
/// Without a recording to decode from, the candidates are the keyframes, so keep them sharper.
pub const CANDIDATE_EDGE_NO_VIDEO: f64 = 1280.0;
/// The candidates' memory ceiling, however long the capture runs.
pub const CANDIDATE_BUDGET_BYTES: usize = 384 << 20;

/// What a finished screen capture leaves behind.
pub struct ScreenRecording {
    /// Small candidate frames for keyframe selection (`CANDIDATE_EDGE`); none when the sampler
    /// never took one.
    pub frames: Vec<RgbaFrame>,
    pub video: Option<PathBuf>,
    pub duration_ms: u64,
    /// The capture time (on the frames' clock) of the video's start.
    pub video_offset_ms: u64,
}

/// One screen recording in progress. `Session` runs a sampler (the app's ScreenCaptureKit or
/// Windows.Graphics.Capture recorder); another recorder only has to hand back a
/// `ScreenRecording` when stopped.
pub trait ScreenCapture: Send {
    /// Takes a candidate frame now (a click or a mark) instead of at the next interval.
    fn request_immediate_frame(&self);
    fn elapsed_ms(&self) -> u64;
    fn stop(self: Box<Self>) -> ScreenRecording;
}

/// A rect in global points (or pixels on Windows): x, y, width, height.
pub type Rect = [f64; 4];

/// Global point → points from `screen`'s top-left corner.
pub fn to_local(screen: Rect, p: [f64; 2]) -> [f64; 2] {
    [p[0] - screen[0], p[1] - screen[1]]
}

/// The part of `r` on `screen`; `r` itself when they don't meet.
pub fn clamp_to(r: Rect, screen: Rect) -> Rect {
    let x0 = r[0].max(screen[0]);
    let y0 = r[1].max(screen[1]);
    let x1 = (r[0] + r[2]).min(screen[0] + screen[2]);
    let y1 = (r[1] + r[3]).min(screen[1] + screen[3]);
    if x1 > x0 && y1 > y0 {
        [x0, y0, x1 - x0, y1 - y0]
    } else {
        r
    }
}

/// `SCStreamConfiguration.sourceRect` for an area: points from its display's top-left corner,
/// not global ones.
pub fn source_rect(area: Rect, display: Rect) -> Rect {
    let [x, y, w, h] = clamp_to(area, display);
    let [lx, ly] = to_local(display, [x, y]);
    [lx, ly, w, h]
}

/// The capture size for `target` with its long edge at most `long_edge` px, and at most
/// `max_scale` pixels per unit of its frame (a 1x display isn't upscaled; on Windows frames are
/// pixels already, so 1; below 1 or NaN counts as 1). Even, and at least 48 a side.
pub fn capture_size(target: &TargetInfo, long_edge: f64, max_scale: f64) -> (u32, u32) {
    let max_dim = target.frame.width.max(target.frame.height).max(1.0);
    let native = target.frame.scale.clamp(1.0, max_scale.max(1.0));
    let scale = (long_edge / max_dim).min(native);
    (
        ((target.frame.width * scale).round() as u32).max(48) & !1,
        ((target.frame.height * scale).round() as u32).max(48) & !1,
    )
}

/// The candidate frames of a capture, bounded in memory. When full it drops every other frame
/// (keeping the first and the ones taken for a click, stroke or shortcut) and samples half as
/// often from then on, so a long capture keeps its whole timeline, evenly, at a coarser step.
pub struct CandidateBuffer {
    frames: Vec<(RgbaFrame, bool)>,
    cap: usize,
    budget: usize,
    interval: Duration,
}

impl CandidateBuffer {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            frames: Vec::new(),
            cap: 0,
            budget: budget_bytes,
            interval: FRAME_SAMPLE_INTERVAL,
        }
    }

    /// How often to sample now.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn push(&mut self, frame: RgbaFrame, urgent: bool) {
        if self.cap == 0 {
            self.cap = (self.budget / frame.rgba.len().max(1)).clamp(60, 1_200);
        }
        self.frames.push((frame, urgent));
        if self.frames.len() >= self.cap {
            self.thin();
        }
    }

    fn thin(&mut self) {
        let mut regular = 0_usize;
        let mut kept: Vec<(RgbaFrame, bool)> = std::mem::take(&mut self.frames)
            .into_iter()
            .enumerate()
            .filter(|(i, (_, urgent))| {
                let keep = *i == 0 || *urgent || regular.is_multiple_of(2);
                if !urgent {
                    regular += 1;
                }
                keep
            })
            .map(|(_, f)| f)
            .collect();
        // Mostly urgent frames (a storm of clicks): thin those too.
        if kept.len() > self.cap * 3 / 4 {
            kept = kept
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i.is_multiple_of(2))
                .map(|(_, f)| f)
                .collect();
        }
        self.frames = kept;
        self.interval *= 2;
    }

    /// Forgets frames taken after `t_ms`, except the first.
    pub fn drop_since(&mut self, t_ms: u64) {
        let mut index = 0;
        self.frames.retain(|(frame, _)| {
            index += 1;
            index == 1 || frame.t_ms <= t_ms
        });
    }

    pub fn into_frames(self) -> Vec<RgbaFrame> {
        self.frames.into_iter().map(|(f, _)| f).collect()
    }
}

/// What the platform's sampler thread records into. It runs until `stopping`, pushes candidate
/// frames (at once when `urgent`), stores when the movie started into `video_offset_ms`, and
/// returns the movie it wrote.
pub struct Sampler {
    pub target: TargetInfo,
    pub video_output_path: Option<PathBuf>,
    /// Scrawl's own windows to leave out of the recording.
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
    pub started_at: Instant,
    pub stopping: Arc<AtomicBool>,
    pub urgent: Arc<AtomicBool>,
    pub frames: Arc<Mutex<CandidateBuffer>>,
    pub video_offset_ms: Arc<AtomicU64>,
}

/// A screen recording: a sampler (see `Sampler`) on its own thread, the candidate frames it
/// takes, and its timing.
pub struct Session {
    started_at: Instant,
    stopping: Arc<AtomicBool>,
    urgent_snapshot: Arc<AtomicBool>,
    frames: Arc<Mutex<CandidateBuffer>>,
    video_offset_ms: Arc<AtomicU64>,
    sampler_thread: Option<JoinHandle<Option<PathBuf>>>,
}

impl Session {
    /// Runs `sample` on a new thread until the session stops. The first frame is taken at once.
    pub fn start(
        target: TargetInfo,
        video_output_path: Option<PathBuf>,
        excluded_window_ids: Arc<Mutex<Vec<u32>>>,
        sample: impl FnOnce(Sampler) -> Option<PathBuf> + Send + 'static,
    ) -> Self {
        let started_at = Instant::now();
        let stopping = Arc::new(AtomicBool::new(false));
        let urgent_snapshot = Arc::new(AtomicBool::new(true));
        let frames = Arc::new(Mutex::new(CandidateBuffer::new(CANDIDATE_BUDGET_BYTES)));
        let video_offset_ms = Arc::new(AtomicU64::new(0));

        let thread_stopping = Arc::clone(&stopping);
        let thread_urgent = Arc::clone(&urgent_snapshot);
        let thread_frames = Arc::clone(&frames);
        let thread_offset = Arc::clone(&video_offset_ms);
        let sampler_thread = thread::Builder::new()
            .name("scrawl-frame-sampler".into())
            .spawn(move || {
                sample(Sampler {
                    target,
                    video_output_path,
                    excluded_window_ids,
                    started_at,
                    stopping: thread_stopping,
                    urgent: thread_urgent,
                    frames: thread_frames,
                    video_offset_ms: thread_offset,
                })
            })
            .ok();

        Self {
            started_at,
            stopping,
            urgent_snapshot,
            frames,
            video_offset_ms,
            sampler_thread,
        }
    }

    pub fn request_immediate_frame(&self) {
        self.urgent_snapshot.store(true, Ordering::Release);
        if let Some(ref handle) = self.sampler_thread {
            handle.thread().unpark();
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    /// Stops the sampler and collects what it took: the candidate frames (possibly none) and
    /// the movie, if it wrote a non-empty one.
    pub fn stop(mut self) -> ScreenRecording {
        let duration_ms = self.elapsed_ms();
        self.stopping.store(true, Ordering::Release);
        let video_path = self
            .sampler_thread
            .take()
            .and_then(|handle| {
                handle.thread().unpark();
                handle.join().ok()
            })
            .flatten();
        let buffer = std::mem::replace(
            &mut *self
                .frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            CandidateBuffer::new(0),
        );
        let valid_video =
            video_path.filter(|path| path.exists() && path.metadata().is_ok_and(|m| m.len() > 0));
        ScreenRecording {
            frames: buffer.into_frames(),
            video: valid_video,
            duration_ms: duration_ms.max(100),
            video_offset_ms: self.video_offset_ms.load(Ordering::Acquire),
        }
    }
}

impl ScreenCapture for Session {
    fn request_immediate_frame(&self) {
        Session::request_immediate_frame(self);
    }

    fn elapsed_ms(&self) -> u64 {
        Session::elapsed_ms(self)
    }

    fn stop(self: Box<Self>) -> ScreenRecording {
        Session::stop(*self)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(handle) = self.sampler_thread.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_stop_joins_worker_before_collecting_its_final_media() {
        let stopping = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(CandidateBuffer::new(1 << 20)));
        let worker_stopping = Arc::clone(&stopping);
        let worker_frames = Arc::clone(&frames);
        let path = std::env::temp_dir().join(format!("scrawl-screen-test-{}", std::process::id()));
        let worker_path = path.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !worker_stopping.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::yield_now();
            }
            assert!(worker_stopping.load(Ordering::Acquire));
            worker_frames
                .lock()
                .unwrap()
                .push(RgbaFrame::new(42, 1, 1, vec![1, 2, 3, 255]), false);
            std::fs::write(&worker_path, b"finalized media").unwrap();
            Some(worker_path)
        });
        let session = Session {
            started_at: Instant::now(),
            stopping,
            urgent_snapshot: Arc::new(AtomicBool::new(false)),
            frames,
            video_offset_ms: Arc::new(AtomicU64::new(120)),
            sampler_thread: Some(worker),
        };
        let recording = session.stop();
        let frames = recording.frames;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].t_ms, 42);
        assert_eq!(frames[0].rgba, vec![1, 2, 3, 255]);
        assert_eq!(recording.video.as_ref(), Some(&path));
        assert_eq!(recording.video_offset_ms, 120);
        std::fs::remove_file(path).unwrap();
    }

    /// Kit's Screen capture on the Pro: the old listen-only event tap was off (no Input
    /// Monitoring), so his clicks on OpenCode's composer never reached the bundle. Clicks now
    /// come from the mouse monitor; polled button state stands in if it couldn't be installed.
    #[test]
    fn candidate_buffer_stays_bounded_and_keeps_the_whole_timeline() {
        // A 1 MB budget of 4 KB frames: a 256-frame cap.
        let mut buffer = CandidateBuffer::new(1 << 20);
        let frame = |t| RgbaFrame::new(t, 32, 32, vec![0; 32 * 32 * 4]);
        let mut t = 0;
        let mut peak = 0;
        // Three hours at the sampler's pace, with a click frame every 10 s.
        while t < 3 * 3_600_000 {
            buffer.push(frame(t), t % 10_000 == 0);
            peak = peak.max(buffer.len());
            t += buffer.interval().as_millis() as u64;
        }
        assert!(peak <= 256, "peak {peak}");
        assert!(buffer.interval() >= FRAME_SAMPLE_INTERVAL * 64);
        let frames = buffer.into_frames();
        assert_eq!(frames[0].t_ms, 0, "the start is kept");
        assert!(frames.windows(2).all(|w| w[0].t_ms < w[1].t_ms));
        // Spread over the whole capture, not bunched at the end.
        let first_half = frames.iter().filter(|f| f.t_ms < 3 * 1_800_000).count();
        assert!(
            first_half * 3 > frames.len(),
            "{first_half} of {}",
            frames.len()
        );
    }

    #[test]
    fn candidate_buffer_keeps_urgent_frames_when_thinning() {
        let mut buffer = CandidateBuffer::new(40 * 4096);
        let frame = |t| RgbaFrame::new(t, 32, 32, vec![0; 32 * 32 * 4]);
        for t in 0..60 {
            buffer.push(frame(t * 220), t == 33);
        }
        assert!(buffer.len() < 60);
        assert_eq!(buffer.interval(), FRAME_SAMPLE_INTERVAL * 2);
        assert!(buffer.into_frames().iter().any(|f| f.t_ms == 33 * 220));
    }

    #[test]
    fn frames_that_may_show_a_system_prompt_are_dropped_but_never_the_first() {
        let mut buffer = CandidateBuffer::new(1 << 20);
        for t in [0, 220, 440, 660, 880] {
            buffer.push(RgbaFrame::new(t, 4, 4, vec![0; 64]), false);
        }
        buffer.drop_since(440);
        let times: Vec<u64> = buffer.into_frames().iter().map(|f| f.t_ms).collect();
        assert_eq!(times, [0, 220, 440]);
        let mut buffer = CandidateBuffer::new(1 << 20);
        buffer.push(RgbaFrame::new(500, 4, 4, vec![0; 64]), false);
        buffer.drop_since(0);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn capture_size_treats_a_scale_limit_below_one_as_one() {
        use crate::model::{CaptureMode, TargetFrame};
        let target = TargetInfo {
            mode: CaptureMode::Display,
            app: None,
            bundle_id: None,
            window_title: None,
            window_id: None,
            display_id: 1,
            frame: TargetFrame {
                x: 0.0,
                y: 0.0,
                width: 1728.0,
                height: 1117.0,
                scale: 2.0,
            },
        };
        assert_eq!(capture_size(&target, 2560.0, 2.0), (2560, 1654));
        assert_eq!(capture_size(&target, 2560.0, 0.5), (1728, 1116));
        assert_eq!(capture_size(&target, 2560.0, f64::NAN), (1728, 1116));
    }
}
