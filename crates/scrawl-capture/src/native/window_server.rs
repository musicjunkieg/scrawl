//! The window server's list of on-screen windows, the Dock's (or taskbar's) strip and each
//! app's details: what `WindowSource::windows` reads on macOS (CoreGraphics and AppKit) and
//! Windows (`native::win`).

use std::collections::HashMap;

use crate::activity::{AppMeta, RawWindow, filter_windows};
use crate::model::{TargetFrame, WindowEntry};

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
    crate::native::win::top_level_windows()
        .into_iter()
        .map(|w| {
            let taskbar = matches!(w.class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");
            let owner = if taskbar {
                "Dock".to_owned()
            } else {
                names
                    .entry(w.pid)
                    .or_insert_with(|| {
                        crate::native::win::app_identity(w.pid)
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
pub fn app_meta_of(pid: i32) -> AppMeta {
    let id = crate::native::win::app_identity(pid as u32).map(|(_, id)| id);
    let regular = crate::native::win::top_level_windows()
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
    let monitor = crate::native::win::monitors().into_iter().find(|m| {
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

/// One timed snapshot of the windows overlapping `frame`, with `front` (the frontmost app other
/// than this process) marked; `cache` keeps each app's details between snapshots.
#[cfg(target_os = "windows")]
pub fn snapshot(
    frame: TargetFrame,
    dock: Option<[f64; 4]>,
    front: Option<i32>,
    cache: &mut HashMap<i32, AppMeta>,
) -> Vec<WindowEntry> {
    let raw = raw_windows();
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
pub fn app_meta_of(pid: i32) -> AppMeta {
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

/// One timed snapshot of the windows overlapping `frame`, with `front` (the frontmost app other
/// than this process) marked; `cache` keeps each app's details between snapshots.
#[cfg(target_os = "macos")]
pub fn snapshot(
    frame: TargetFrame,
    dock: Option<[f64; 4]>,
    front: Option<i32>,
    cache: &mut HashMap<i32, AppMeta>,
) -> Vec<WindowEntry> {
    objc2::rc::autoreleasepool(|_| {
        let raw = raw_windows();
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
