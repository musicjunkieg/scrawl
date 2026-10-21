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

/// Reads the on-screen window list (front to back) from the window server.
#[cfg(target_os = "macos")]
pub fn raw_windows() -> Vec<RawWindow> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionOnScreenOnly,
    };

    let Some(list) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let key = CFString::from_static_string;
    let (k_number, k_pid, k_owner, k_name, k_layer, k_alpha, k_bounds) = (
        key("kCGWindowNumber"),
        key("kCGWindowOwnerPID"),
        key("kCGWindowOwnerName"),
        key("kCGWindowName"),
        key("kCGWindowLayer"),
        key("kCGWindowAlpha"),
        key("kCGWindowBounds"),
    );
    let (k_x, k_y, k_w, k_h) = (key("X"), key("Y"), key("Width"), key("Height"));
    let number = |dict: &CFDictionary<CFString, CFType>, k: &CFString| -> Option<f64> {
        dict.find(k)?.downcast::<CFNumber>()?.to_f64()
    };
    let string = |dict: &CFDictionary<CFString, CFType>, k: &CFString| -> String {
        dict.find(k)
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for item in list.iter() {
        let ptr = *item as CFDictionaryRef;
        if ptr.is_null() {
            continue;
        }
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(ptr) };
        let Some(bounds) = dict
            .find(&k_bounds)
            .and_then(|v| v.downcast::<CFDictionary>())
        else {
            continue;
        };
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef()) };
        out.push(RawWindow {
            id: number(&dict, &k_number).unwrap_or(0.0) as u32,
            pid: number(&dict, &k_pid).unwrap_or(0.0) as i32,
            owner: string(&dict, &k_owner),
            title: string(&dict, &k_name),
            bounds: [
                number(&bounds, &k_x).unwrap_or(0.0),
                number(&bounds, &k_y).unwrap_or(0.0),
                number(&bounds, &k_w).unwrap_or(0.0),
                number(&bounds, &k_h).unwrap_or(0.0),
            ],
            layer: number(&dict, &k_layer).unwrap_or(0.0) as i32,
            alpha: number(&dict, &k_alpha).unwrap_or(1.0),
        });
    }
    out
}

/// Reads the visible top-level windows (front to back). Like CGWindowList: the taskbar is the
/// `Dock` (layer 20), always-on-top and tool windows sit above layer 0, and cloaked (on another
/// virtual desktop) or minimized windows have no alpha.
#[cfg(target_os = "windows")]
pub fn raw_windows() -> Vec<RawWindow> {
    let mut names: HashMap<u32, String> = HashMap::new();
    crate::win::top_level_windows()
        .into_iter()
        .map(|w| {
            let taskbar = matches!(w.class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");
            let owner = if taskbar {
                "Dock".to_owned()
            } else {
                names
                    .entry(w.pid)
                    .or_insert_with(|| {
                        crate::win::app_identity(w.pid)
                            .map(|(name, _)| name)
                            .unwrap_or_default()
                    })
                    .clone()
            };
            let layer = if taskbar {
                20
            } else if w.is_app_window() && !w.topmost() {
                0
            } else {
                3
            };
            RawWindow {
                id: w.hwnd as u32,
                pid: w.pid as i32,
                owner,
                title: w.title.clone(),
                bounds: w.rect,
                layer,
                alpha: if w.on_screen() { 1.0 } else { 0.0 },
            }
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn app_meta_of(pid: i32) -> AppMeta {
    let id = crate::win::app_identity(pid as u32).map(|(_, id)| id);
    let regular = crate::win::top_level_windows()
        .iter()
        .any(|w| w.pid == pid as u32 && w.is_app_window());
    AppMeta {
        bundle_id: id,
        regular,
    }
}

/// The taskbar's strip on the display at `frame` (global pixels): the difference between the
/// display and its work area.
#[cfg(target_os = "windows")]
pub fn dock_rect(frame: TargetFrame) -> Option<[f64; 4]> {
    let monitor = crate::win::monitors().into_iter().find(|m| {
        let [x, y, w, h] = m.rect;
        (x - frame.x).abs() < 1.0
            && (y - frame.y).abs() < 1.0
            && (w - frame.width).abs() < 1.0
            && (h - frame.height).abs() < 1.0
    })?;
    let [fx, fy, fw, fh] = monitor.rect;
    let [vx, vy, vw, vh] = monitor.work;
    let (left, right, top, bottom) = (
        vx - fx,
        (fx + fw) - (vx + vw),
        vy - fy,
        (fy + fh) - (vy + vh),
    );
    let min = 8.0;
    if bottom >= min {
        Some([fx, vy + vh, fw, bottom])
    } else if top >= min {
        Some([fx, fy, fw, top])
    } else if left >= min {
        Some([fx, vy, left, vh])
    } else if right >= min {
        Some([vx + vw, vy, right, vh])
    } else {
        None
    }
}

/// One timed snapshot of the windows overlapping `frame`.
#[cfg(target_os = "windows")]
pub fn snapshot(
    frame: TargetFrame,
    dock: Option<[f64; 4]>,
    cache: &mut HashMap<i32, AppMeta>,
) -> Vec<WindowEntry> {
    let raw = raw_windows();
    let front = crate::capture::frontmost_external_pid();
    filter_windows(
        &raw,
        frame,
        std::process::id() as i32,
        front,
        dock,
        &mut |pid| cache.entry(pid).or_insert_with(|| app_meta_of(pid)).clone(),
    )
}

#[cfg(target_os = "macos")]
fn app_meta_of(pid: i32) -> AppMeta {
    use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
    match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        Some(app) => AppMeta {
            bundle_id: app.bundleIdentifier().map(|b| b.to_string()),
            regular: app.activationPolicy() == NSApplicationActivationPolicy::Regular,
        },
        None => AppMeta {
            bundle_id: None,
            regular: false,
        },
    }
}

/// The strip the Dock reserves on the display at `frame` (global top-left points), from the
/// difference between the screen's frame and visible frame. `None` when the Dock hides or off
/// the main thread (`NSScreen` is main-thread only).
#[cfg(target_os = "macos")]
pub fn dock_rect(frame: TargetFrame) -> Option<[f64; 4]> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    // AppKit's origin is the primary display's bottom-left corner, y up.
    let primary_h = screens.firstObject()?.frame().size.height;
    let flip = |r: objc2_foundation::NSRect| {
        [
            r.origin.x,
            primary_h - r.origin.y - r.size.height,
            r.size.width,
            r.size.height,
        ]
    };
    let screen = screens.iter().find(|s| {
        let [x, y, w, h] = flip(s.frame());
        (x - frame.x).abs() < 1.0
            && (y - frame.y).abs() < 1.0
            && (w - frame.width).abs() < 1.0
            && (h - frame.height).abs() < 1.0
    })?;
    let [fx, fy, fw, fh] = flip(screen.frame());
    let [vx, vy, vw, vh] = flip(screen.visibleFrame());
    let (left, right, bottom) = (vx - fx, (fx + fw) - (vx + vw), (fy + fh) - (vy + vh));
    // A hidden Dock still leaves a few points at the edge.
    let min = 8.0;
    if bottom >= min {
        Some([fx, vy + vh, fw, bottom])
    } else if left >= min {
        Some([fx, vy, left, vh])
    } else if right >= min {
        Some([vx + vw, vy, right, vh])
    } else {
        None
    }
}

/// One timed snapshot of the windows overlapping `frame`.
#[cfg(target_os = "macos")]
pub fn snapshot(
    frame: TargetFrame,
    dock: Option<[f64; 4]>,
    cache: &mut HashMap<i32, AppMeta>,
) -> Vec<WindowEntry> {
    objc2::rc::autoreleasepool(|_| {
        let raw = raw_windows();
        let front = crate::capture::frontmost_external_pid();
        filter_windows(
            &raw,
            frame,
            std::process::id() as i32,
            front,
            dock,
            &mut |pid| cache.entry(pid).or_insert_with(|| app_meta_of(pid)).clone(),
        )
    })
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
