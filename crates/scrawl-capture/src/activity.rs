//! The window and cursor log of a Screen-mode capture: which windows were on screen (front to
//! back) and where the cursor was, so autocrop can work out which window the narration and ink
//! were about. Snapshots are stored only when the window list changes and cursor samples only
//! when the cursor moves.

use std::time::{Duration, Instant};

use crate::model::{CursorSample, TargetFrame, WindowEntry, WindowSnapshot};

/// Cursor samples closer than this to the previous one (in capture points) are dropped.
const CURSOR_MIN_MOVE_PT: f64 = 2.0;

/// What the worker collected over one capture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityLog {
    pub windows: Vec<WindowSnapshot>,
    pub cursor: Vec<CursorSample>,
    /// Calls to the window list and the time they took, for the log line at the end.
    pub snapshot_calls: u32,
    pub snapshot_time: Duration,
    /// When the finish sequence began and how long stopping capture streams took.
    pub stop_started: Option<Instant>,
    pub stop_ms: Option<u64>,
}

impl ActivityLog {
    /// Stores a snapshot unless the window list is identical to the last stored one.
    pub fn push_windows(&mut self, t: u64, windows: Vec<WindowEntry>) -> bool {
        if self
            .windows
            .last()
            .is_some_and(|last| last.windows == windows)
        {
            return false;
        }
        self.windows.push(WindowSnapshot { t, windows });
        true
    }

    /// Stores a cursor sample (capture points) unless it is within `CURSOR_MIN_MOVE_PT` of the
    /// last one or outside the capture.
    pub fn push_cursor(&mut self, t: u64, x_pt: f64, y_pt: f64, frame: TargetFrame) -> bool {
        let (w, h) = (frame.width.max(1.0), frame.height.max(1.0));
        if !(0.0..=w).contains(&x_pt) || !(0.0..=h).contains(&y_pt) {
            return false;
        }
        let nx = round4(x_pt / w);
        let ny = round4(y_pt / h);
        if let Some(CursorSample(_, px, py)) = self.cursor.last() {
            let dx = (nx - px) as f64 * w;
            let dy = (ny - py) as f64 * h;
            if dx.hypot(dy) < CURSOR_MIN_MOVE_PT {
                return false;
            }
        }
        self.cursor.push(CursorSample(t, nx, ny));
        true
    }
}

fn round4(v: f64) -> f32 {
    ((v * 10_000.0).round() / 10_000.0) as f32
}

/// One window and cursor log in progress. The app's `WindowLogger` reads the window server;
/// another source only has to hand back an `ActivityLog` when stopped.
pub trait ActivityCapture: Send {
    fn stop(self: Box<Self>) -> ActivityLog;
}

#[cfg(test)]
mod tests {
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

    fn entry(id: u32, x: i32) -> WindowEntry {
        WindowEntry {
            id,
            app: format!("App {id}"),
            bundle_id: None,
            regular: true,
            title: String::new(),
            bounds: [x, 0, 400, 300],
            layer: 0,
            frontmost: id == 1,
        }
    }
    #[test]
    fn window_snapshots_are_stored_only_when_the_list_changes() {
        let mut log = ActivityLog::default();
        assert!(log.push_windows(0, vec![entry(1, 0), entry(2, 100)]));
        assert!(!log.push_windows(500, vec![entry(1, 0), entry(2, 100)]));
        // A moved window, a reorder, and a focus change are all new snapshots.
        assert!(log.push_windows(1_000, vec![entry(1, 10), entry(2, 100)]));
        assert!(log.push_windows(1_500, vec![entry(2, 100), entry(1, 10)]));
        let mut unfocused = vec![entry(2, 100), entry(1, 10)];
        unfocused[1].frontmost = false;
        assert!(log.push_windows(2_000, unfocused.clone()));
        assert!(!log.push_windows(2_500, unfocused));
        assert_eq!(
            log.windows.iter().map(|s| s.t).collect::<Vec<_>>(),
            vec![0, 1_000, 1_500, 2_000]
        );
    }

    #[test]
    fn cursor_samples_skip_tiny_moves_and_points_outside_the_capture() {
        let mut log = ActivityLog::default();
        assert!(log.push_cursor(0, 500.0, 250.0, frame()));
        assert!(!log.push_cursor(66, 501.0, 250.5, frame()));
        assert!(log.push_cursor(132, 520.0, 250.0, frame()));
        assert!(!log.push_cursor(198, -5.0, 250.0, frame()));
        assert!(!log.push_cursor(264, 500.0, 900.0, frame()));
        assert_eq!(
            log.cursor,
            vec![CursorSample(0, 0.5, 0.5), CursorSample(132, 0.52, 0.5)]
        );
        assert_eq!(
            serde_json::to_string(&log.cursor[1]).unwrap(),
            "[132,0.52,0.5]"
        );
    }
}
