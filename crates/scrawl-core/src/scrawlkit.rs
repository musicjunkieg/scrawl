//! ScrawlKit's recordings: the `.json` timeline the SDK writes beside each in-app movie, and the
//! [`Capture`] it becomes. The Mac app's `scrawl import` and ingest build their bundles from
//! [`Timeline::capture`], and so does ScrawlKit when it compiles on the phone (`scrawl-ffi`).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::capture::Capture;
use crate::model::{
    CaptureMode, ContextChange, DeviceInfo, InteractionEvent, ScreenSpan, Stroke, TargetFrame,
    TargetInfo, TranscriptData,
};

/// `app` is the display name and `bundle_id` the identifier; an old timeline without a bundle
/// ID falls back to the name for both.
pub fn phone_target(
    name: &str,
    app: Option<&str>,
    bundle_id: Option<&str>,
    width: u32,
    height: u32,
) -> TargetInfo {
    let bundle_id = bundle_id.filter(|id| !id.is_empty()).or(app);
    TargetInfo {
        mode: CaptureMode::Device,
        app: Some(app.or(bundle_id).unwrap_or(name).to_owned()),
        bundle_id: bundle_id.map(str::to_owned),
        window_title: None,
        window_id: None,
        display_id: 0,
        frame: TargetFrame {
            x: 0.0,
            y: 0.0,
            width: f64::from(width),
            height: f64::from(height),
            scale: 1.0,
        },
    }
}

/// The `.json` ScrawlKit writes beside each in-app recording (seconds from the start, points in
/// screen coordinates).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub recorded_at: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub screen: Option<Screen>,
    #[serde(default)]
    pub markers: Vec<f64>,
    #[serde(default)]
    pub events: Vec<AppEvent>,
    #[serde(default)]
    pub touches: Vec<Touch>,
    #[serde(default)]
    pub strokes: Vec<TimelineStroke>,
    /// `CFBundleShortVersionString`; `version` is the build.
    #[serde(default)]
    pub short_version: Option<String>,
    /// App-defined values when the recording started.
    #[serde(default)]
    pub context: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub context_changes: Vec<TimelineContextChange>,
    /// Screen changes; the first is at 0 when the app named a screen before recording.
    #[serde(default)]
    pub screens: Vec<TimelineScreen>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TimelineContextChange {
    pub t: f64,
    pub key: String,
    #[serde(default)]
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TimelineScreen {
    pub t: f64,
    /// `null` when the app stopped naming a screen.
    #[serde(default)]
    pub name: Option<String>,
}

/// A context value as text: strings as they are, numbers and booleans spelled out, `null` none.
fn context_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

impl Timeline {
    /// The start times (ms) of touches the app marked `still`: swipes that left their scroll view
    /// where it was.
    pub fn still_swipes(&self) -> Vec<u64> {
        let mut starts: BTreeMap<i64, f64> = BTreeMap::new();
        for touch in &self.touches {
            let start = starts.entry(touch.id).or_insert(touch.t);
            *start = start.min(touch.t);
        }
        let mut still: Vec<u64> = self
            .touches
            .iter()
            .filter(|touch| touch.still == Some(true))
            .filter_map(|touch| starts.get(&touch.id).map(|&t| ms(t)))
            .collect();
        still.sort_unstable();
        still.dedup();
        still
    }

    pub fn context(&self) -> BTreeMap<String, String> {
        self.context
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), context_text(v)?)))
            .collect()
    }

    pub fn context_changes(&self) -> Vec<ContextChange> {
        let mut changes: Vec<ContextChange> = self
            .context_changes
            .iter()
            .map(|c| ContextChange {
                t: ms(c.t),
                key: c.key.clone(),
                value: context_text(&c.value),
            })
            .collect();
        changes.sort_by_key(|c| c.t);
        changes
    }

    pub fn screen_spans(&self, duration_ms: u64) -> Vec<ScreenSpan> {
        let changes: Vec<(u64, Option<String>)> = self
            .screens
            .iter()
            .map(|s| (ms(s.t), s.name.clone()))
            .collect();
        ScreenSpan::from_changes(&changes, duration_ms)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TimelineStroke {
    pub t0: f64,
    pub t1: f64,
    #[serde(default)]
    pub points: Vec<[f64; 3]>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    #[serde(default = "one")]
    pub scale: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct AppEvent {
    pub t: f64,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Touch {
    pub t: f64,
    pub id: i64,
    pub x: f64,
    pub y: f64,
    pub phase: String,
    #[serde(default)]
    pub label: Option<String>,
    /// On a touch's last sample: whether the scroll view it moved came back to rest where it
    /// started, so the swipe changed nothing on screen. Absent when the app couldn't tell.
    #[serde(default)]
    pub still: Option<bool>,
}

/// Seconds on the timeline as milliseconds (0 for negative or non-finite).
pub fn ms(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1_000.0).round() as u64
    } else {
        0
    }
}

/// Finger movement under this many points, held under this long, is a tap.
const TAP_SLOP_PT: f64 = 12.0;
const TAP_MS: u64 = 420;

/// Ink that never moves further than this many points from where it started draws nothing: a tap
/// made while Draw was on.
const DOT_PT: f64 = 6.0;

/// Turns raw touch samples into taps and swipes, normalized to the screen. A touch still down
/// when the recording ends is left out: that's the press that stopped it (ScrawlKit's
/// three-finger trigger), not something done in the app.
pub fn touch_events(touches: &[Touch], screen: Screen) -> Vec<InteractionEvent> {
    let mut sorted = touches.to_vec();
    sorted.sort_by(|a, b| a.t.total_cmp(&b.t));
    let (w, h) = (screen.width.max(1.0), screen.height.max(1.0));
    let norm = |touch: &Touch| {
        [
            (touch.x / w).clamp(0.0, 1.0) as f32,
            (touch.y / h).clamp(0.0, 1.0) as f32,
        ]
    };
    let mut open: Vec<(i64, Vec<Touch>)> = Vec::new();
    let mut out = Vec::new();
    let close = |samples: Vec<Touch>, out: &mut Vec<InteractionEvent>| {
        let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
            return;
        };
        let travel = samples
            .iter()
            .map(|s| (s.x - first.x).hypot(s.y - first.y))
            .fold(0.0, f64::max);
        let (t0, t1) = (ms(first.t), ms(last.t));
        let [x0, y0] = norm(first);
        let hit_label = samples
            .iter()
            .find_map(|s| s.label.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if travel < TAP_SLOP_PT && t1.saturating_sub(t0) < TAP_MS {
            out.push(InteractionEvent::Click {
                t: t0,
                x: x0,
                y: y0,
                button: Some("touch".into()),
                label: hit_label.map(str::to_owned),
                role: None,
            });
        } else {
            let [x1, y1] = norm(last);
            let drag_label = if travel < TAP_SLOP_PT {
                Some(match hit_label {
                    Some(l) => format!("long press: {l}"),
                    None => "long press".into(),
                })
            } else {
                hit_label.map(str::to_owned)
            };
            out.push(InteractionEvent::Drag {
                t: t0,
                t1,
                x0,
                y0,
                x1,
                y1,
                label: drag_label,
                button: Some("touch".into()),
                path: samples.iter().map(norm).collect(),
            });
        }
    };
    for touch in sorted {
        let index = open.iter().position(|(id, _)| *id == touch.id);
        match (touch.phase.as_str(), index) {
            ("down", Some(i)) => {
                let (_, samples) = open.remove(i);
                close(samples, &mut out);
                open.push((touch.id, vec![touch]));
            }
            ("down", None) => open.push((touch.id, vec![touch])),
            ("up" | "cancel", Some(i)) => {
                let (_, mut samples) = open.remove(i);
                samples.push(touch);
                close(samples, &mut out);
            }
            ("up" | "cancel", None) => close(vec![touch], &mut out),
            (_, Some(i)) => open[i].1.push(touch),
            (_, None) => open.push((touch.id, vec![touch])),
        }
    }
    out.sort_by_key(InteractionEvent::timestamp_ms);
    out
}

impl Timeline {
    /// The capture of this timeline's movie, `width`×`height` pixels and `duration_ms` long:
    /// touches become taps and swipes, app events join them, markers become flags, ink moves to
    /// the bundle's coordinates, and the app's context and screens come along. Its `id`,
    /// `createdAtMs`, transcript and file names are left for the caller.
    pub fn capture(&self, width: u32, height: u32, duration_ms: u64) -> Capture {
        let screen = self.screen.unwrap_or(Screen {
            width: f64::from(width.max(1)),
            height: f64::from(height.max(1)),
            scale: 1.0,
        });
        let mut events = touch_events(&self.touches, screen);
        events.extend(self.events.iter().map(|e| InteractionEvent::AppEvent {
            t: ms(e.t),
            label: e.label.clone(),
        }));
        events.sort_by_key(InteractionEvent::timestamp_ms);
        let (sw, sh) = (screen.width.max(1.0), screen.height.max(1.0));
        let strokes: Vec<Stroke> = self
            .strokes
            .iter()
            .filter(|s| {
                let [x0, y0, _] = s.points.first().copied().unwrap_or_default();
                s.points
                    .iter()
                    .any(|&[x, y, _]| (x - x0).hypot(y - y0) >= DOT_PT)
            })
            .filter_map(|s| {
                let t0 = ms(s.t0);
                let t1 = ms(s.t1).max(t0);
                let mut pts = Vec::with_capacity(s.points.len() * 3);
                for &[x, y, pt_t] in &s.points {
                    let nx =
                        ((x / sw).clamp(0.0, 1.0) * f64::from(crate::model::SCALE)).round() as i32;
                    let ny =
                        ((y / sh).clamp(0.0, 1.0) * f64::from(crate::model::SCALE)).round() as i32;
                    let dt = ms(pt_t).saturating_sub(t0) as i32;
                    pts.extend_from_slice(&[nx, ny, dt]);
                }
                (!pts.is_empty()).then_some(Stroke {
                    t0,
                    t1,
                    fw: width,
                    fh: height,
                    on: None,
                    pin: None,
                    laser: false,
                    pts,
                })
            })
            .collect();
        let name = self.device.clone().unwrap_or_else(|| "iPhone".into());
        let app = self.app.clone().filter(|a| !a.is_empty());
        let nonempty = |s: &Option<String>| s.clone().filter(|s| !s.is_empty());
        let device = DeviceInfo {
            name: name.clone(),
            capture: "scrawlkit".into(),
            system: self.system.clone(),
            width,
            height,
            screen: self.screen.map(|s| [s.width, s.height, s.scale]),
            app: app.clone(),
            app_version: nonempty(&self.version),
            app_short_version: nonempty(&self.short_version),
            ..Default::default()
        };
        Capture {
            id: "scrawl".into(),
            directory: None,
            created_at_ms: 0,
            target: phone_target(
                &name,
                app.as_deref(),
                self.bundle_id.as_deref(),
                width,
                height,
            ),
            duration_ms,
            strokes,
            events,
            transcript: TranscriptData {
                source: "none".into(),
                ..TranscriptData::default()
            },
            note: None,
            flags: self.markers.iter().map(|&t| ms(t)).collect(),
            source: Some("iphone".into()),
            context: self.context(),
            context_changes: self.context_changes(),
            screens: self.screen_spans(duration_ms),
            device: Some(device),
            video: None,
            audio: None,
            agent_context: false,
            still_swipes: self.still_swipes(),
            skip_still_swipes: false,
            page: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(t: f64, id: i64, x: f64, y: f64, phase: &str) -> Touch {
        Touch {
            t,
            id,
            x,
            y,
            phase: phase.into(),
            label: None,
            still: None,
        }
    }

    #[test]
    fn swipes_the_app_marked_still_are_listed_by_their_start() {
        let json = r#"{"touches":[
            {"t":1.0,"id":1,"x":200,"y":700,"phase":"down"},
            {"t":1.2,"id":1,"x":200,"y":300,"phase":"move"},
            {"t":1.3,"id":1,"x":200,"y":250,"phase":"up","still":true},
            {"t":2.0,"id":2,"x":200,"y":700,"phase":"down"},
            {"t":2.3,"id":2,"x":200,"y":250,"phase":"up","still":false},
            {"t":3.0,"id":3,"x":200,"y":700,"phase":"down"},
            {"t":3.3,"id":3,"x":200,"y":250,"phase":"up"}]}"#;
        let timeline: Timeline = serde_json::from_str(json).unwrap();
        assert_eq!(timeline.still_swipes(), vec![1_000]);
        let capture = timeline.capture(400, 800, 4_000);
        assert_eq!(capture.still_swipes, vec![1_000]);
        // The listed swipe is the event that starts then.
        assert!(
            capture
                .events
                .iter()
                .any(|e| matches!(e, InteractionEvent::Drag { t: 1_000, .. }))
        );
    }

    #[test]
    fn touches_become_taps_swipes_and_long_presses() {
        let screen = Screen {
            width: 400.0,
            height: 800.0,
            scale: 3.0,
        };
        let events = touch_events(
            &[
                touch(1.0, 1, 200.0, 400.0, "down"),
                touch(1.08, 1, 202.0, 401.0, "up"),
                touch(2.0, 2, 200.0, 700.0, "down"),
                touch(2.1, 2, 200.0, 500.0, "move"),
                touch(2.2, 2, 200.0, 200.0, "up"),
                touch(3.0, 3, 100.0, 100.0, "down"),
                touch(3.9, 3, 101.0, 100.0, "up"),
                // A touch the system took over still ends.
                touch(5.0, 4, 300.0, 400.0, "down"),
                touch(5.1, 4, 380.0, 400.0, "cancel"),
                // The three fingers that stopped the recording never lift before it ends.
                touch(9.0, 5, 100.0, 600.0, "down"),
                touch(9.0, 6, 200.0, 620.0, "down"),
                touch(9.0, 7, 300.0, 610.0, "move"),
            ],
            screen,
        );
        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[0],
            InteractionEvent::Click { t: 1000, x, y, button: Some(b), .. }
                if *x == 0.5 && *y == 0.5 && b == "touch"
        ));
        match &events[1] {
            InteractionEvent::Drag {
                t,
                t1,
                y0,
                y1,
                path,
                ..
            } => {
                assert_eq!((*t, *t1), (2000, 2200));
                assert!(*y0 > 0.85 && *y1 < 0.3);
                assert_eq!(path.len(), 3);
            }
            other => panic!("expected a swipe, got {other:?}"),
        }
        assert!(matches!(
            &events[2],
            InteractionEvent::Drag { label: Some(l), .. } if l == "long press"
        ));
        assert!(matches!(
            &events[3],
            InteractionEvent::Drag { t1: 5100, .. }
        ));
    }

    #[test]
    fn timeline_parses_scrawlkit_output() {
        let json = r#"{"app":"dev.kitlangton.UpDown","version":"42","device":"iPhone","system":"27.0",
            "screen":{"width":440,"height":956,"scale":3},"recordedAt":"2026-10-13T16:40:00Z",
            "duration":12.5,"markers":[3.2],"events":[{"t":4.1,"label":"Done tapped"}],
            "touches":[{"t":4.0,"id":7,"x":220,"y":900,"phase":"down"},{"t":4.05,"id":7,"x":220,"y":900,"phase":"up"}]}"#;
        let timeline: Timeline = serde_json::from_str(json).unwrap();
        assert_eq!(timeline.markers, vec![3.2]);
        assert_eq!(timeline.events[0].label, "Done tapped");
        let events = touch_events(&timeline.touches, timeline.screen.unwrap());
        assert!(matches!(events[0], InteractionEvent::Click { t: 4000, .. }));
        // Timelines from before app context still parse, with none.
        assert!(timeline.short_version.is_none());
        assert!(timeline.context().is_empty() && timeline.context_changes().is_empty());
        assert!(timeline.screen_spans(12_500).is_empty());
    }

    #[test]
    fn timeline_parses_app_context_and_screens() {
        let json = r#"{"app":"OpenCode","bundleId":"ai.opencode.mobilecode","version":"354",
            "shortVersion":"1.4","duration":20,
            "context":{"server":"studio","session":"ses_1","build":7,"gone":null},
            "contextChanges":[{"t":9.5,"key":"session","value":null},{"t":4.25,"key":"session","value":"ses_2"}],
            "screens":[{"t":0,"name":"Sessions"},{"t":4.2,"name":"Settings › Experimental"},
                {"t":12,"name":null},{"t":15,"name":"Sessions"}]}"#;
        let timeline: Timeline = serde_json::from_str(json).unwrap();
        assert_eq!(timeline.short_version.as_deref(), Some("1.4"));
        assert_eq!(
            timeline.context(),
            BTreeMap::from([
                ("build".to_owned(), "7".to_owned()),
                ("server".to_owned(), "studio".to_owned()),
                ("session".to_owned(), "ses_1".to_owned()),
            ])
        );
        assert_eq!(
            timeline.context_changes(),
            vec![
                ContextChange {
                    t: 4_250,
                    key: "session".into(),
                    value: Some("ses_2".into()),
                },
                ContextChange {
                    t: 9_500,
                    key: "session".into(),
                    value: None,
                },
            ]
        );
        let span = |t0, t1, name: &str| ScreenSpan {
            t0,
            t1,
            name: name.into(),
        };
        assert_eq!(
            timeline.screen_spans(20_000),
            vec![
                span(0, 4_200, "Sessions"),
                span(4_200, 12_000, "Settings › Experimental"),
                span(15_000, 20_000, "Sessions"),
            ]
        );
    }

    #[test]
    fn phone_target_keeps_the_bundle_id_apart_from_the_display_name() {
        let target = phone_target(
            "iPhone",
            Some("OpenCode"),
            Some("ai.opencode.mobilecode"),
            10,
            20,
        );
        assert_eq!(target.app.as_deref(), Some("OpenCode"));
        assert_eq!(target.bundle_id.as_deref(), Some("ai.opencode.mobilecode"));
        // Older timelines only had the name.
        let old = phone_target("iPhone", Some("UpDown"), Some(""), 10, 20);
        assert_eq!(old.app.as_deref(), Some("UpDown"));
        assert_eq!(old.bundle_id.as_deref(), Some("UpDown"));
        let mirror = phone_target("Kit's iPhone", None, None, 10, 20);
        assert_eq!(mirror.app.as_deref(), Some("Kit's iPhone"));
        assert_eq!(mirror.bundle_id, None);
    }

    #[test]
    fn a_timeline_becomes_a_capture_the_compiler_builds() {
        let json = r#"{"app":"UpDown","bundleId":"dev.kitlangton.UpDown","version":"42",
            "shortVersion":"1.4","device":"Kit's iPhone","system":"iOS 27.0",
            "screen":{"width":400,"height":800,"scale":3},"duration":3,"markers":[2.5],
            "events":[{"t":1.5,"label":"Done tapped"}],
            "touches":[{"t":1.0,"id":1,"x":200,"y":400,"phase":"down","label":"Done"},
                {"t":1.05,"id":1,"x":200,"y":400,"phase":"up"}],
            "strokes":[{"t0":2.0,"t1":2.2,"points":[[100,200,2.0],[300,600,2.2]]},
                {"t0":2.4,"t1":2.4,"points":[[250,700,2.4]]},
                {"t0":2.6,"t1":2.7,"points":[[50,50,2.6],[52,53,2.7]]}],
            "context":{"session":"ses_1"},"screens":[{"t":0,"name":"Workout"}]}"#;
        let timeline: Timeline = serde_json::from_str(json).unwrap();
        let capture = timeline.capture(1200, 2400, 3_000);
        assert_eq!(capture.source.as_deref(), Some("iphone"));
        assert_eq!(capture.flags, vec![2_500]);
        assert_eq!(capture.target.mode, CaptureMode::Device);
        assert_eq!(
            capture.target.bundle_id.as_deref(),
            Some("dev.kitlangton.UpDown")
        );
        let device = capture.device.as_ref().unwrap();
        assert_eq!((device.width, device.height), (1200, 2400));
        assert_eq!(device.app_short_version.as_deref(), Some("1.4"));
        assert!(matches!(
            &capture.events[0],
            InteractionEvent::Click { t: 1_000, label: Some(l), .. } if l == "Done"
        ));
        assert!(matches!(
            &capture.events[1],
            InteractionEvent::AppEvent { t: 1_500, .. }
        ));
        // Taps made while Draw was on, which never moved, aren't ink.
        assert_eq!(capture.strokes.len(), 1);
        // Points go to the bundle's 0–10 000 coordinates, times relative to the stroke.
        let stroke = &capture.strokes[0];
        assert_eq!((stroke.fw, stroke.fh), (1200, 2400));
        assert_eq!(stroke.pts, vec![2_500, 2_500, 0, 7_500, 7_500, 200]);
        assert_eq!(capture.screens[0].name, "Workout");

        // It survives the trip through JSON (the C interface hands it to the app and back).
        let json = serde_json::to_string(&capture).unwrap();
        let back: Capture = serde_json::from_str(&json).unwrap();
        assert_eq!(back.strokes, capture.strokes);
        assert_eq!(back.device, capture.device);

        let frames = [0_u64, 1_000, 2_200]
            .iter()
            .enumerate()
            .map(|(i, &t)| crate::model::RgbaFrame {
                t_ms: t,
                width: 40,
                height: 80,
                rgba: vec![40 + i as u8 * 60; 40 * 80 * 4],
                badge_top_right: false,
            })
            .collect();
        let built = crate::capture::compile(back, frames).unwrap();
        assert!(built.summary.contains("Done tapped"));
        assert!(built.summary.contains("ses_1"));
    }
}
