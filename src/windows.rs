//! Window geometry and cursor log for Screen-mode captures.
//!
//! While a full-screen capture records, a low-rate worker writes down which windows were on
//! screen (front to back) and where the cursor was, so `scrawl autocrop` can work out which
//! window the narration and ink were about. Snapshots are deduped (only stored when the window
//! list changes), cursor samples are deduped by movement, and window titles stay in
//! `scrawl.json`: they never go into `summary.md` or the clipboard.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use crate::model::TargetFrame;

// The window and cursor types are bundle format; they live in `model` so the bundle compiler
// doesn't depend on this capture-side module.
pub use crate::model::{CursorSample, WindowEntry, WindowSnapshot};
pub use scrawl_capture::activity::{
    ActivityCapture, ActivityLog, AppMeta, RawWindow, WindowLogger, WindowSource, filter_windows,
    owned_window_at,
};
#[cfg(target_os = "macos")]
pub use scrawl_capture::activity::{macos_is_asking, system_prompt_at, system_prompts_over};
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use scrawl_capture::native::window_server::{dock_rect, raw_windows};

/// One timed snapshot of the windows overlapping `frame`, with the frontmost app other than
/// Scrawl marked (`capture::frontmost_external_pid`).
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn snapshot(
    frame: TargetFrame,
    dock: Option<[f64; 4]>,
    cache: &mut HashMap<i32, AppMeta>,
) -> Vec<WindowEntry> {
    scrawl_capture::native::window_server::snapshot(
        frame,
        dock,
        crate::capture::frontmost_external_pid(),
        cache,
    )
}

/// The window server, as `WindowLogger` reads it: the cursor, the frontmost app other than
/// Scrawl, and `snapshot` with the Dock's strip and the app details cached.
pub struct ServerWindows {
    frame: TargetFrame,
    dock: Option<[f64; 4]>,
    cache: HashMap<i32, AppMeta>,
}

impl ServerWindows {
    /// Call on the main thread, so the Dock's strip can be read from `NSScreen`.
    pub fn new(frame: TargetFrame) -> Self {
        Self {
            frame,
            dock: dock_rect(frame),
            cache: HashMap::new(),
        }
    }
}

impl WindowSource for ServerWindows {
    fn cursor(&mut self) -> Option<[f32; 2]> {
        crate::capture::current_mouse_screen()
    }

    fn frontmost_pid(&mut self) -> Option<i32> {
        crate::capture::frontmost_external_pid()
    }

    fn windows(&mut self) -> Vec<WindowEntry> {
        snapshot(self.frame, self.dock, &mut self.cache)
    }

    fn log(&mut self, line: &str) {
        crate::capture::scrawl_log(line);
    }
}

/// Starts logging the window server for a capture of `frame` (see `WindowLogger`). Call on the
/// main thread.
pub fn start_logger(
    frame: TargetFrame,
    started_at: Instant,
    request: Arc<AtomicBool>,
) -> WindowLogger {
    WindowLogger::start(ServerWindows::new(frame), frame, started_at, request)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn frame() -> TargetFrame {
        TargetFrame {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 500.0,
            scale: 2.0,
        }
    }

    #[test]
    fn logger_snapshots_at_start_and_right_after_a_request() {
        let request = Arc::new(AtomicBool::new(false));
        let logger = start_logger(frame(), Instant::now(), Arc::clone(&request));
        thread::sleep(Duration::from_millis(150));
        request.store(true, Ordering::Release);
        // A snapshot can take well over a tick on a busy CI runner, so wait for the worker to
        // take the request instead of sleeping a fixed time.
        let deadline = Instant::now() + Duration::from_secs(5);
        while request.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!request.load(Ordering::Acquire), "the request was taken");
        let log = logger.stop();
        assert!(log.snapshot_calls >= 2, "{} snapshots", log.snapshot_calls);
        assert!(log.windows.len() <= log.snapshot_calls as usize);
    }
}
