//! The ink overlay on Windows: a click-through GPUI window over the display being recorded
//! that paints what `laser_overlay.metal` paints on macOS, from the same telemetry: the
//! picker's dim and cutout, the target's inset border, the start glow, Shift ink with its fade,
//! a ⇧-click pin's brief pulse and the Shift tip dot. It is left out of the recording
//! (`WDA_EXCLUDEFROMCAPTURE`).
//!
//! Also the Win32 window helpers Scrawl's other popups use.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, Corners, Edges, Hsla, IntoElement, PathBuilder, PathStyle, Pixels, Point,
    Render, StrokeOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, canvas, point, prelude::*, px, size,
};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, LWA_ALPHA,
    PostMessageW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WM_CLOSE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};
use windows::core::{PCSTR, w};

use crate::capture::SharedCaptureTelemetry;
use crate::model::RgbaFrame;
use crate::sketch::{pin_pulse, slice_laser_window, smooth_ink_points, smooth_laser_points};

const TAIL_MS: u64 = 420;

/// Leaves a window out of every screen capture (Windows 10 2004 and later).
/// `SCRAWL_SHOW_OWN_WINDOWS=1` keeps them in, for screenshots of Scrawl itself.
pub fn exclude_from_capture(hwnd: HWND) {
    if std::env::var_os("SCRAWL_SHOW_OWN_WINDOWS").is_some() {
        return;
    }
    let _ = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) };
}

/// Turns off what Windows adds around a transparent popup: Windows 11's rounded corners,
/// border and shadow (a ghost box), and GPUI's transparent-gradient accent (a faint veil;
/// DirectComposition already gives per-pixel alpha).
pub fn strip_chrome(hwnd: HWND) {
    let round = DWMWCP_DONOTROUND;
    let policy = DWMNCRP_DISABLED;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const round).cast(),
            4,
        );
        let none = 0xFFFF_FFFE_u32; // DWMWA_COLOR_NONE
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&raw const none).cast(), 4);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            (&raw const policy).cast(),
            4,
        );
    }
    disable_accent(hwnd);
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient: u32,
    animation_id: u32,
}

#[repr(C)]
struct CompositionData {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

type SetWindowCompositionAttribute = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;

/// `SetWindowCompositionAttribute(WCA_ACCENT_POLICY, ACCENT_DISABLED)`: user32 exports it
/// without a header.
fn disable_accent(hwnd: HWND) {
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
            return;
        };
        let Some(proc) = GetProcAddress(
            user32,
            PCSTR(c"SetWindowCompositionAttribute".as_ptr().cast()),
        ) else {
            return;
        };
        let set: SetWindowCompositionAttribute = std::mem::transmute(proc);
        let mut policy = AccentPolicy {
            state: 0,
            flags: 0,
            gradient: 0,
            animation_id: 0,
        };
        let mut data = CompositionData {
            attribute: 19,
            data: (&raw mut policy).cast(),
            size: std::mem::size_of::<AccentPolicy>(),
        };
        set(hwnd, &mut data);
    }
}

/// A window's pixels, even when covered (`PrintWindow` with full content).
pub fn capture_window(id: isize) -> anyhow::Result<RgbaFrame> {
    let hwnd = super::hwnd(id);
    let mut rect = Default::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }?;
    let (width, height) = (
        (rect.right - rect.left).max(1),
        (rect.bottom - rect.top).max(1),
    );
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(dc, bitmap.into());
        // PW_RENDERFULLCONTENT captures DirectComposition content.
        let _ = PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(2));
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0_u8; (width * height * 4) as usize];
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok(RgbaFrame::new(0, width as u32, height as u32, bgra))
    }
}

/// The display an overlay covers: its rect in global pixels and its scale.
#[derive(Clone, Copy)]
struct Placement {
    rect: [f64; 4],
    scale: f32,
}

/// The overlay for one display. Dropping it closes the window.
pub struct InkOverlay {
    hwnd: isize,
    placement: Arc<Mutex<Placement>>,
}

impl InkOverlay {
    /// Covers the display holding the centre of `on` (global pixels), else the primary.
    pub fn open<T: 'static>(
        telemetry: Arc<SharedCaptureTelemetry>,
        on: Option<[f64; 4]>,
        cx: &mut Context<T>,
    ) -> Result<Self, String> {
        let monitor = on
            .and_then(|[x, y, w, h]| super::monitor_at(x + w / 2.0, y + h / 2.0))
            .or_else(super::primary_monitor)
            .ok_or("No display")?;
        let display_id = crate::ui::gpui_display_for(&monitor, cx).map(|d| d.id());
        let scale = monitor.scale as f32;
        let bounds = Bounds {
            origin: point(
                px(monitor.rect[0] as f32 / scale),
                px(monitor.rect[1] as f32 / scale),
            ),
            size: size(
                px(monitor.rect[2] as f32 / scale),
                px(monitor.rect[3] as f32 / scale),
            ),
        };
        let rect = monitor.rect;
        let placement = Arc::new(Mutex::new(Placement { rect, scale }));
        let view_placement = Arc::clone(&placement);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    display_id,
                    titlebar: None,
                    focus: false,
                    show: false,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                move |window, cx| {
                    // Layered before GPUI's first frame, or DirectComposition never shows it.
                    // Clicks fall through only a window that is both layered and transparent;
                    // alpha 255 keeps GPUI's per-pixel alpha as it is.
                    if let Some(hwnd) = crate::ui::hwnd_of(window) {
                        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
                        let extra = WS_EX_TRANSPARENT.0
                            | WS_EX_LAYERED.0
                            | WS_EX_TOOLWINDOW.0
                            | WS_EX_NOACTIVATE.0
                            | WS_EX_TOPMOST.0;
                        unsafe {
                            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style | extra) as isize);
                            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                        }
                        strip_chrome(hwnd);
                        exclude_from_capture(hwnd);
                    }
                    cx.new(|_| OverlayView {
                        telemetry,
                        placement: view_placement,
                    })
                },
            )
            .map_err(|error| format!("{error:#}"))?;
        let hwnd = handle
            .update(cx, |_, window, _| crate::ui::hwnd_of(window))
            .ok()
            .flatten()
            .ok_or("The overlay has no window")?;
        // Resized on the next turn of the event loop: called from inside an update, GPUI's
        // resize callback can't reach the app and the window keeps its default layout size.
        let id = hwnd.0 as isize;
        cx.spawn(async move |_, _| place(id, rect)).detach();
        Ok(Self {
            hwnd: hwnd.0 as isize,
            placement,
        })
    }

    pub fn window_number(&self) -> u32 {
        self.hwnd as usize as u32
    }

    /// The display it covers, global pixels.
    pub fn screen(&self) -> [f64; 4] {
        self.placement
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .rect
    }

    /// Moves to the display holding the centre of `rect` (global pixels). Returns whether it
    /// moved: false when it's on that display already or there's none there.
    pub fn move_to(&mut self, rect: [f64; 4]) -> bool {
        let [x, y, w, h] = rect;
        let Some(monitor) = super::monitor_at(x + w / 2.0, y + h / 2.0) else {
            return false;
        };
        {
            let mut placement = self
                .placement
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if (0..4).all(|i| (placement.rect[i] - monitor.rect[i]).abs() < 1.0) {
                return false;
            }
            *placement = Placement {
                rect: monitor.rect,
                scale: monitor.scale as f32,
            };
        }
        // Off the UI thread: this runs inside a GPUI update, where the resize callback can't
        // reach the app (see `open`); the window's thread handles the move from its loop.
        let id = self.hwnd;
        std::thread::spawn(move || place(id, monitor.rect));
        true
    }
}

/// Puts one of Scrawl's GPUI windows (the overlay, picker or pill) over `rect` (global pixels)
/// on top, and shows it without activating it. Call it outside GPUI updates: the moves it makes
/// reach GPUI's resize and DPI handlers at once.
pub(crate) fn place(id: isize, rect: [f64; 4]) {
    let hwnd = super::hwnd(id);
    let [x, y, w, h] = rect.map(|v| v.round() as i32);
    let _ = unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE) };
    // GPUI keeps a resize frame around the client area; grow the window by it so the client
    // area (what GPUI draws) covers `rect` exactly. Measured from where the window is now: a
    // move onto a monitor of another DPI has GPUI resize it to Windows' suggested rect.
    let (mut window, mut client, mut origin) = (RECT::default(), RECT::default(), POINT::default());
    let placed = unsafe {
        GetWindowRect(hwnd, &mut window).is_ok()
            && GetClientRect(hwnd, &mut client).is_ok()
            && ClientToScreen(hwnd, &mut origin).as_bool()
    };
    if placed {
        let [x, y, w, h] = grown_by_frame([x, y, w, h], window, origin, client);
        let _ = unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE) };
    }
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
}

/// The window rect whose client area is `target`, given where the window is now (`window`), its
/// client area's screen origin and its client rect.
fn grown_by_frame(target: [i32; 4], window: RECT, origin: POINT, client: RECT) -> [i32; 4] {
    let [x, y, w, h] = target;
    let (left, top) = (origin.x - window.left, origin.y - window.top);
    let right = window.right - origin.x - client.right;
    let bottom = window.bottom - origin.y - client.bottom;
    [x - left, y - top, w + left + right, h + top + bottom]
}

impl Drop for InkOverlay {
    fn drop(&mut self) {
        let _ =
            unsafe { PostMessageW(Some(super::hwnd(self.hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

struct OverlayView {
    telemetry: Arc<SharedCaptureTelemetry>,
    /// The display it covers (moved by `InkOverlay::move_to`).
    placement: Arc<Mutex<Placement>>,
}

/// One frame's geometry, in the window's points.
struct Scene {
    target: Option<([f32; 4], f32)>,
    uniform_dim: Option<f32>,
    now_sec: f32,
    picker_fade: f32,
    glow: bool,
    ink: Vec<(Vec<[f32; 2]>, f32)>,
    laser: Vec<Vec<([f32; 2], f32)>>,
    /// Pin pulses: centre, ring radius and opacity.
    pins: Vec<([f32; 2], f32, f32)>,
    tip: Option<[f32; 2]>,
    screen: [f32; 2],
}

fn ink_color(alpha: f32) -> Hsla {
    gpui::Rgba {
        r: 1.0,
        g: 0.23,
        b: 0.19,
        a: alpha,
    }
    .into()
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> Hsla {
    gpui::Rgba { r, g, b, a }.into()
}

impl OverlayView {
    fn scene(&self, screen: [f32; 2]) -> Option<Scene> {
        let t = &self.telemetry;
        let active = t.active.load(Ordering::Acquire);
        let selecting = t.selecting.load(Ordering::Acquire);
        let mode = t.capture_mode.load(Ordering::Acquire);
        if !active && !selecting && mode != 3 {
            return None;
        }
        let now_ms = t.elapsed_ms();
        let now_sec = if selecting {
            0.0
        } else {
            now_ms as f32 / 1000.0
        };
        let picker_fade = if selecting {
            let elapsed = t
                .picker_started_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .elapsed()
                .as_secs_f32();
            1.0 - (1.0 - (elapsed / 0.15).clamp(0.0, 1.0)).powi(3)
        } else {
            1.0
        };
        let placement = *self
            .placement
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let k = 1.0 / placement.scale;
        let [ox, oy] = [placement.rect[0] as f32, placement.rect[1] as f32];
        let local = |x: f32, y: f32| [(x - ox) * k, (y - oy) * k];
        let picker = if selecting {
            *t.picker_rect
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        } else {
            None
        };
        let frame = *t
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (target, uniform_dim) = if selecting {
            match picker {
                Some(([x, y, w, h], kind)) => {
                    let [lx, ly] = local(x, y);
                    (
                        Some(([lx, ly, w.max(1.0) * k, h.max(1.0) * k], f32::from(kind))),
                        None,
                    )
                }
                None => (None, Some(picker_fade)),
            }
        } else {
            let [lx, ly] = local(frame.x as f32, frame.y as f32);
            let constrained = mode > 0 && frame.x > -10_000.0;
            let rect = [
                lx,
                ly,
                (frame.width as f32).max(1.0) * k,
                (frame.height as f32).max(1.0) * k,
            ];
            (constrained.then_some((rect, mode as f32)), None)
        };
        let to_screen = |nx: f32, ny: f32| -> [f32; 2] {
            let [lx, ly] = local(frame.x as f32, frame.y as f32);
            [
                lx + nx * frame.width as f32 * k,
                ly + ny * frame.height as f32 * k,
            ]
        };

        let mut strokes: Vec<(crate::model::Stroke, bool)> = t
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .map(|s| (s, false))
            .collect();
        if let Some(laser) = t
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            strokes.push((laser, true));
        }
        if let Some(ink) = t
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            strokes.push((ink, true));
        }
        let mut ink = Vec::new();
        let mut laser = Vec::new();
        let mut pins = Vec::new();
        if active {
            for (stroke, live) in &strokes {
                if stroke.pin.is_some() {
                    if let Some([nx, ny]) = stroke.normalized_points().first().copied()
                        && let Some((radius, alpha)) = pin_pulse(now_ms.saturating_sub(stroke.t1))
                    {
                        pins.push((to_screen(nx, ny), radius, alpha));
                    }
                    continue;
                }
                if stroke.laser {
                    if stroke.t1 + TAIL_MS < now_ms {
                        continue;
                    }
                    let raw =
                        slice_laser_window(&stroke.timed_points(), now_ms, TAIL_MS, to_screen);
                    laser.push(smooth_laser_points(&raw));
                    continue;
                }
                let alpha = if *live {
                    1.0
                } else {
                    let age = now_ms.saturating_sub(stroke.t1);
                    if age >= 1_300 {
                        continue;
                    } else if age <= 650 {
                        1.0
                    } else {
                        let t = ((age - 650) as f32 / 650.0).clamp(0.0, 1.0);
                        (1.0 - t) * (1.0 - t)
                    }
                };
                let pts: Vec<[f32; 2]> = stroke
                    .normalized_points()
                    .into_iter()
                    .map(|[nx, ny]| to_screen(nx, ny))
                    .collect();
                let smoothed: Vec<[f32; 2]> = smooth_ink_points(&pts)
                    .into_iter()
                    .map(|(p, _)| p)
                    .collect();
                ink.push((smoothed, alpha));
            }
        }
        let tip = (active && t.shift_held.load(Ordering::Acquire)).then(|| {
            let [x, y] = *t
                .pointer_screen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            local(x, y)
        });
        Some(Scene {
            target,
            uniform_dim,
            now_sec,
            picker_fade,
            glow: active && t.start_glow.load(Ordering::Acquire),
            ink,
            laser,
            pins,
            tip,
            screen,
        })
    }
}

fn pt(p: [f32; 2], origin: Point<Pixels>) -> Point<Pixels> {
    point(origin.x + px(p[0]), origin.y + px(p[1]))
}

fn rect_bounds(r: [f32; 4], origin: Point<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: pt([r[0], r[1]], origin),
        size: size(px(r[2].max(0.0)), px(r[3].max(0.0))),
    }
}

/// A rounded rectangle as a closed path (for the dim's even-odd hole).
fn add_rounded_rect(builder: &mut PathBuilder, r: [f32; 4], radius: f32, origin: Point<Pixels>) {
    let [x, y, w, h] = r;
    let rad = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    if rad <= 0.0 {
        builder.add_polygon(
            &[
                pt([x, y], origin),
                pt([x + w, y], origin),
                pt([x + w, y + h], origin),
                pt([x, y + h], origin),
            ],
            true,
        );
        return;
    }
    builder.move_to(pt([x + rad, y], origin));
    builder.line_to(pt([x + w - rad, y], origin));
    builder.curve_to(pt([x + w, y + rad], origin), pt([x + w, y], origin));
    builder.line_to(pt([x + w, y + h - rad], origin));
    builder.curve_to(pt([x + w - rad, y + h], origin), pt([x + w, y + h], origin));
    builder.line_to(pt([x + rad, y + h], origin));
    builder.curve_to(pt([x, y + h - rad], origin), pt([x, y + h], origin));
    builder.line_to(pt([x, y + rad], origin));
    builder.curve_to(pt([x + rad, y], origin), pt([x, y], origin));
    builder.close();
}

fn round_stroke(width: f32) -> PathBuilder {
    let mut options = StrokeOptions::default().with_line_width(width);
    options.line_join = lyon::tessellation::LineJoin::Round;
    options.start_cap = lyon::tessellation::LineCap::Round;
    options.end_cap = lyon::tessellation::LineCap::Round;
    PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options))
}

fn paint_polyline(
    window: &mut Window,
    points: &[[f32; 2]],
    width: f32,
    color: Hsla,
    o: Point<Pixels>,
) {
    if points.len() < 2 {
        if let Some(&p) = points.first() {
            paint_dot(window, p, width / 2.0, color, o);
        }
        return;
    }
    let mut builder = round_stroke(width);
    builder.move_to(pt(points[0], o));
    for &p in &points[1..] {
        builder.line_to(pt(p, o));
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_dot(window: &mut Window, p: [f32; 2], radius: f32, color: Hsla, o: Point<Pixels>) {
    window.paint_quad(gpui::quad(
        rect_bounds(
            [p[0] - radius, p[1] - radius, radius * 2.0, radius * 2.0],
            o,
        ),
        Corners::all(px(radius)),
        color,
        Edges::default(),
        gpui::transparent_black(),
        gpui::BorderStyle::Solid,
    ));
}

fn paint_scene(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window) {
    let o = bounds.origin;
    let [sw, sh] = scene.screen;
    let intro = 1.0 - (1.0 - (scene.now_sec / 0.24).clamp(0.0, 1.0)).powi(3);
    let intro_fade = 1.0 - intro;
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;

    if let Some((rect, kind)) = scene.target {
        let device = kind > 2.5;
        let window_mode = kind > 1.5 && !device;
        let radius = if device {
            rect[2].min(rect[3]) * 0.135
        } else if window_mode {
            10.0
        } else {
            0.0
        };
        // Outside the target: 0.32 dim while picking, easing to 0.22 while recording.
        let dim = mix(0.32, 0.22, intro) * scene.picker_fade;
        let mut hole = PathBuilder::fill();
        add_rounded_rect(&mut hole, [0.0, 0.0, sw, sh], 0.0, o);
        add_rounded_rect(&mut hole, rect, radius, o);
        if let Ok(path) = hole.build() {
            window.paint_path(path, rgba(0.0, 0.0, 0.0, dim));
        }
        // Inside a picked window: the picker's blue tint, dissolving as recording starts.
        if window_mode && intro_fade > 0.002 {
            window.paint_quad(gpui::quad(
                rect_bounds(rect, o),
                Corners::all(px(radius)),
                rgba(0.231, 0.361, 0.965, 0.13 * intro_fade * scene.picker_fade),
                Edges::default(),
                gpui::transparent_black(),
                gpui::BorderStyle::Solid,
            ));
        }
        if !device {
            let width = if window_mode {
                mix(2.0, 1.0, intro)
            } else {
                1.0
            };
            let picker_border = if window_mode {
                [0.357, 0.486, 0.980]
            } else {
                [1.0, 1.0, 1.0]
            };
            let alpha = mix(if window_mode { 0.93 } else { 0.92 }, 0.28, intro) * scene.picker_fade;
            let c = [
                mix(picker_border[0], 1.0, intro),
                mix(picker_border[1], 1.0, intro),
                mix(picker_border[2], 1.0, intro),
            ];
            window.paint_quad(gpui::quad(
                rect_bounds(rect, o),
                Corners::all(px(radius)),
                gpui::transparent_black(),
                Edges::all(px(width)),
                rgba(c[0], c[1], c[2], alpha),
                gpui::BorderStyle::Solid,
            ));
        }
    } else if let Some(fade) = scene.uniform_dim {
        window.paint_quad(gpui::fill(bounds, rgba(0.0, 0.0, 0.0, 0.32 * fade)));
    } else if scene.glow {
        // A shortcut recording: a red glow along the display's edges, in by 0.12 s, gone by 1.1 s.
        let t = scene.now_sec;
        let smooth = |a: f32, b: f32, x: f32| {
            let s = ((x - a) / (b - a)).clamp(0.0, 1.0);
            s * s * (3.0 - 2.0 * s)
        };
        let env = smooth(0.0, 0.12, t) * (1.0 - smooth(0.35, 1.1, t));
        if env > 0.002 {
            for step in 0..24 {
                let d = step as f32 * 3.0;
                let a = ((-d / 22.0).exp() - (-(d + 3.0) / 22.0).exp()) * 0.45 * env;
                window.paint_quad(gpui::quad(
                    rect_bounds([d, d, sw - 2.0 * d, sh - 2.0 * d], o),
                    Corners::default(),
                    gpui::transparent_black(),
                    Edges::all(px(3.0)),
                    ink_color(a.max(0.0) * 1.8),
                    gpui::BorderStyle::Solid,
                ));
            }
        }
    } else if intro_fade > 0.002 {
        window.paint_quad(gpui::fill(bounds, rgba(0.0, 0.0, 0.0, 0.32 * intro_fade)));
    }

    // A pin is a 1.5 pt ring centred on its radius, no fill, as `laser_overlay.metal` draws it.
    for &(p, radius, alpha) in &scene.pins {
        let outer = radius + 0.75;
        window.paint_quad(gpui::quad(
            rect_bounds([p[0] - outer, p[1] - outer, outer * 2.0, outer * 2.0], o),
            Corners::all(px(outer)),
            gpui::transparent_black(),
            Edges::all(px(1.5)),
            ink_color(alpha),
            gpui::BorderStyle::Solid,
        ));
    }
    for (points, alpha) in &scene.ink {
        paint_polyline(window, points, 4.5, ink_color(*alpha), o);
    }
    for points in &scene.laser {
        for pair in points.windows(2) {
            let w = (pair[0].1 + pair[1].1) / 2.0;
            if w <= 0.002 {
                continue;
            }
            let glow = rgba(1.0, 0.16, 0.19, 0.18 * w);
            paint_polyline(
                window,
                &[pair[0].0, pair[1].0],
                12.0 * (0.28 + 0.72 * w),
                glow,
                o,
            );
            paint_polyline(
                window,
                &[pair[0].0, pair[1].0],
                4.9 * w.powf(0.68),
                rgba(1.0, 0.16, 0.19, 0.88 * w.powf(0.4)),
                o,
            );
            paint_polyline(
                window,
                &[pair[0].0, pair[1].0],
                1.8 * w.powf(0.82),
                rgba(1.0, 0.97, 0.97, 0.95 * w.powf(0.72)),
                o,
            );
        }
    }
    if let Some(tip) = scene.tip {
        paint_dot(window, tip, 2.0, ink_color(1.0), o);
    }
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let size = window.viewport_size();
        let scene = self.scene([f32::from(size.width), f32::from(size.height)]);
        // Telemetry changes on other threads; follow it every frame while the overlay is up.
        window.request_animation_frame();
        canvas(
            |_, _, _| {},
            move |bounds, (), window: &mut Window, _: &mut App| {
                if let Some(scene) = scene {
                    paint_scene(&scene, bounds, window);
                }
            },
        )
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_grown_by_its_frame_as_it_is_now() {
        let rect = |left, top, right, bottom| RECT {
            left,
            top,
            right,
            bottom,
        };
        // Asked for 1920×1080 at the origin, then resized by a DPI change to 2880×1620, with a
        // 1 px frame left, right and bottom and 2 px on top.
        let grown = grown_by_frame(
            [0, 0, 1920, 1080],
            rect(0, 0, 2880, 1620),
            POINT { x: 1, y: 2 },
            rect(0, 0, 2878, 1617),
        );
        assert_eq!(grown, [-1, -2, 1922, 1083]);
        // No frame: the target itself.
        let none = grown_by_frame(
            [-1920, 0, 400, 100],
            rect(-1920, 0, -1520, 100),
            POINT { x: -1920, y: 0 },
            rect(0, 0, 400, 100),
        );
        assert_eq!(none, [-1920, 0, 400, 100]);
    }
}
