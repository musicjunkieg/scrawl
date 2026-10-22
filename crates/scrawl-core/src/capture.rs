//! The one input every recorder hands the compiler: a [`Capture`] (what was recorded besides the
//! frames, in `scrawl.json`'s own types) and its frames as RGBA pixels. A page (`scrawl-wasm`)
//! and an app (`scrawl-ffi`) both build their bundles through here, in memory.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, ensure};
use serde::Deserialize;

use crate::build::{BuildOptions, BuiltBundle, SourceExtras};
use crate::model::{InteractionEvent, RgbaFrame, Stroke, TargetInfo, TranscriptData};
use crate::page;

/// What a recorder (a page, an app) captured, besides its frames. Field names are `scrawl.json`'s (camelCase), and
/// the nested types are its types.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    /// `scrawl.json`'s `id`; the bundle is `<id>.scrawl`.
    #[serde(default = "default_id")]
    pub id: String,
    /// Where the bundle will be stored, which `summary.md` names its files by. Defaults to
    /// `<id>.scrawl`.
    #[serde(default)]
    pub directory: Option<String>,
    /// `scrawl.json`'s `createdAtMs` (epoch milliseconds).
    #[serde(default)]
    pub created_at_ms: u64,
    pub target: TargetInfo,
    pub duration_ms: u64,
    #[serde(default)]
    pub strokes: Vec<Stroke>,
    #[serde(default)]
    pub events: Vec<InteractionEvent>,
    /// What was said, already transcribed; without it the bundle has no transcript.
    #[serde(default = "no_transcript")]
    pub transcript: TranscriptData,
    #[serde(default)]
    pub note: Option<String>,
    /// Moments flagged without ink (ms); each becomes a mark with keyframe priority.
    #[serde(default)]
    pub flags: Vec<u64>,
    /// Where the capture came from (`"web"`, say).
    #[serde(default)]
    pub source: Option<String>,
    /// What the page said about itself (route, build, user), shown in `summary.md`.
    #[serde(default)]
    pub context: BTreeMap<String, String>,
    /// The names of a movie and an `audio.wav` the caller stores beside the bundle.
    #[serde(default)]
    pub video: Option<String>,
    #[serde(default)]
    pub audio: Option<String>,
    /// The `agentContext` experiment: numbered marks and the extra context in `summary.md`.
    #[serde(default)]
    pub agent_context: bool,
    /// What the page saw, in `sdk.js`'s shape at epoch ms (`page::Event`): the element and
    /// component under clicks and ⇧ marks, console errors, failed requests and video playback.
    /// They name the capture's clicks and strokes and add app events (`page::apply`).
    #[serde(default)]
    pub page: Vec<page::Event>,
}

fn default_id() -> String {
    "scrawl".into()
}

fn no_transcript() -> TranscriptData {
    TranscriptData {
        source: "none".into(),
        ..TranscriptData::default()
    }
}

/// Builds the bundle for `capture` from its candidate `frames` (any order; each is a moment the
/// page was seen).
pub fn compile(capture: Capture, frames: Vec<RgbaFrame>) -> Result<BuiltBundle> {
    compile_with_full(capture, frames, Vec::new())
}

/// The times (ms) of the keyframes `compile` picks from these candidate `frames`. A caller that
/// holds its frames small decodes just these again at full size for `compile_with_full`.
pub fn keyframe_times(capture: &Capture, frames: &[RgbaFrame]) -> Result<Vec<u64>> {
    check_frames(frames)?;
    let (events, strokes) = page_applied(capture);
    Ok(crate::build::keyframe_times(
        frames,
        &strokes,
        &capture.flags,
        &events,
        &capture.transcript,
        capture.duration_ms,
    ))
}

/// `compile`, with `full`: the frames at `keyframe_times` again at full size, which the bundle's
/// images are made from instead of the candidates. Empty `full` uses the candidates.
pub fn compile_with_full(
    capture: Capture,
    frames: Vec<RgbaFrame>,
    full: Vec<RgbaFrame>,
) -> Result<BuiltBundle> {
    check_frames(&frames)?;
    if !full.is_empty() {
        check_frames(&full)?;
    }
    let directory = PathBuf::from(
        capture
            .directory
            .clone()
            .unwrap_or_else(|| format!("{}.scrawl", capture.id)),
    );
    let (events, strokes) = page_applied(&capture);
    let options = BuildOptions {
        agent_context: capture.agent_context,
        autocrop: None,
        created_at_ms: capture.created_at_ms,
        logger: None,
    };
    let full = (!full.is_empty()).then(|| {
        let full = RefCell::new(Some(full));
        Box::new(move |_: &[u64]| Ok(full.borrow_mut().take().unwrap_or_default()))
            as crate::build::FullFrames
    });
    let extras = SourceExtras {
        source: capture.source,
        context: capture.context,
        flags: capture.flags,
        full_frames: full,
        ..SourceExtras::default()
    };
    crate::build::build_bundle(
        &directory,
        capture.id,
        capture.target,
        capture.duration_ms,
        frames,
        strokes,
        events,
        capture.transcript,
        capture.video,
        capture.audio,
        capture.note,
        extras,
        &options,
    )
}

fn check_frames(frames: &[RgbaFrame]) -> Result<()> {
    ensure!(!frames.is_empty(), "A capture needs at least one frame");
    for frame in frames {
        // Checked: on wasm `usize` is 32 bits, and 65536×65536×4 would wrap to 0.
        let expected = (frame.width as usize)
            .checked_mul(frame.height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        ensure!(
            frame.width > 0 && frame.height > 0 && expected == Some(frame.rgba.len()),
            "The frame at {} ms isn't {}×{} RGBA pixels",
            frame.t_ms,
            frame.width,
            frame.height
        );
    }
    Ok(())
}

/// The capture's events and strokes once what the page saw names them (`page::apply`).
fn page_applied(capture: &Capture) -> (Vec<InteractionEvent>, Vec<Stroke>) {
    let mut events = capture.events.clone();
    let mut strokes = capture.strokes.clone();
    if !capture.page.is_empty() {
        let start = capture.created_at_ms;
        let end = start.saturating_add(capture.duration_ms);
        let seen = page::clean(capture.page.clone(), start, end);
        page::apply(&seen, start, &mut events, &mut strokes);
    }
    (events, strokes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Capture {
        serde_json::from_value(serde_json::json!({
            "id": "web-1",
            "createdAtMs": 1_700_000_000_000_u64,
            "target": {
                "mode": "window", "app": "Example", "displayId": 0,
                "frame": {"x": 0.0, "y": 0.0, "width": 64.0, "height": 40.0, "scale": 1.0}
            },
            "durationMs": 2_000,
            "events": [{"kind": "click", "t": 900, "x": 0.5, "y": 0.5}],
            "source": "web",
        }))
        .unwrap()
    }

    fn frames() -> Vec<RgbaFrame> {
        [0_u64, 1_000, 1_900]
            .into_iter()
            .map(|t| RgbaFrame::new(t, 64, 40, vec![(t / 10) as u8; 64 * 40 * 4]))
            .collect()
    }

    #[test]
    fn a_capture_becomes_a_bundle_with_its_files() {
        let built = compile(capture(), frames()).unwrap();
        let names: Vec<&str> = built.files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names[0], "contact-sheet.png");
        assert!(
            names.iter().any(|n| n.starts_with("frames/frame-")),
            "{names:?}"
        );
        assert_eq!(names[names.len() - 2..], ["scrawl.json", "summary.md"]);
        assert_eq!(built.bundle.id, "web-1");
        assert_eq!(built.bundle.source.as_deref(), Some("web"));
        assert_eq!(built.bundle.transcript.source, "none");
        assert!(built.summary.contains("web-1.scrawl"), "{}", built.summary);
    }

    #[test]
    fn what_the_page_saw_names_clicks_and_adds_errors_and_requests() {
        let start = 1_700_000_000_000_u64;
        let mut capture = capture();
        capture.page = serde_json::from_value(serde_json::json!([
            {"t": start + 880, "kind": "pointer", "target": {
                "tag": "button", "id": "move", "text": "Move the card",
                "component": "Card", "source": "src/Card.tsx:12"}},
            {"t": start + 1_200, "kind": "error", "message": "boom", "source": "src/Card.tsx:30"},
            {"t": start + 1_500, "kind": "request", "method": "GET",
             "path": "/api/cards?token=secret", "status": 404},
            {"t": start + 9_000, "kind": "error", "message": "after the capture"},
        ]))
        .unwrap();
        let built = compile(capture, frames()).unwrap();
        let labels: Vec<String> = built
            .bundle
            .events
            .iter()
            .filter_map(|e| match e {
                InteractionEvent::Click { label, .. } => label.clone(),
                InteractionEvent::AppEvent { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "<button#move> “Move the card” in Card (src/Card.tsx:12)",
                "console error: boom (src/Card.tsx:30)",
                "GET /api/cards → 404",
            ]
        );
    }

    /// A PNG's width and height, from its IHDR chunk.
    fn png_size(png: &[u8]) -> (u32, u32) {
        let be = |i: usize| u32::from_be_bytes(png[i..i + 4].try_into().unwrap());
        (be(16), be(20))
    }

    #[test]
    fn small_candidates_pick_the_keyframes_and_full_frames_draw_them() {
        let small = frames();
        let times = keyframe_times(&capture(), &small).unwrap();
        assert!(!times.is_empty());
        let full: Vec<RgbaFrame> = times
            .iter()
            .map(|&t| RgbaFrame::new(t, 128, 80, vec![(t / 10) as u8; 128 * 80 * 4]))
            .collect();
        let built = compile_with_full(capture(), small, full).unwrap();
        let picked: Vec<u64> = built.bundle.keyframes.iter().map(|k| k.t).collect();
        assert_eq!(
            picked, times,
            "compile picks the keyframes keyframe_times named"
        );
        for (name, png) in built.files.iter().filter(|(n, _)| n.starts_with("frames/")) {
            assert_eq!(
                png_size(png),
                (128, 80),
                "{name} is drawn from its full frame"
            );
        }
        // Without full frames, the candidates are the keyframes.
        let built = compile(capture(), frames()).unwrap();
        let (name, png) = built
            .files
            .iter()
            .find(|(n, _)| n.starts_with("frames/"))
            .unwrap();
        assert_eq!(png_size(png), (64, 40), "{name}");
    }

    #[test]
    fn bad_frames_and_json_are_errors_not_panics() {
        assert!(compile(capture(), Vec::new()).is_err());
        let short = vec![RgbaFrame {
            t_ms: 0,
            width: 64,
            height: 40,
            rgba: vec![0; 10],
            badge_top_right: false,
        }];
        assert!(compile(capture(), short).is_err());
        assert!(serde_json::from_str::<Capture>("{}").is_err());
    }
}
