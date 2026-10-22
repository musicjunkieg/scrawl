//! Scrawl's bundle compiler for the browser. A page hands over what it recorded (the
//! [`Capture`] as JSON, and its frames as RGBA pixels) and gets back the `.scrawl` bundle's files
//! (`contact-sheet.png`, `frames/frame-XX.png`, `scrawl.json`, `summary.md`), built by
//! `scrawl_core::build::build_bundle` with no file system, threads or clock.
//!
//! From JavaScript (`wasm-bindgen --target web` or `nodejs`):
//!
//! ```js
//! const compiler = new Compiler();
//! compiler.addFrame(t, imageData.width, imageData.height, imageData.data);
//! const bundle = compiler.compile(JSON.stringify(capture));
//! for (const name of bundle.fileNames()) save(name, bundle.file(name));
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use scrawl_core::build::{BuildOptions, BuiltBundle, SourceExtras};
use scrawl_core::model::{InteractionEvent, RgbaFrame, Stroke, TargetInfo, TranscriptData};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

/// What a page recorded, besides its frames. Field names are `scrawl.json`'s (camelCase), and
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
    ensure!(!frames.is_empty(), "A capture needs at least one frame");
    for frame in &frames {
        ensure!(
            frame.width > 0
                && frame.height > 0
                && frame.rgba.len() == frame.width as usize * frame.height as usize * 4,
            "The frame at {} ms isn't {}×{} RGBA pixels",
            frame.t_ms,
            frame.width,
            frame.height
        );
    }
    let directory = PathBuf::from(
        capture
            .directory
            .clone()
            .unwrap_or_else(|| format!("{}.scrawl", capture.id)),
    );
    let options = BuildOptions {
        agent_context: capture.agent_context,
        autocrop: None,
        created_at_ms: capture.created_at_ms,
        logger: None,
    };
    let extras = SourceExtras {
        source: capture.source,
        context: capture.context,
        flags: capture.flags,
        ..SourceExtras::default()
    };
    scrawl_core::build::build_bundle(
        &directory,
        capture.id,
        capture.target,
        capture.duration_ms,
        frames,
        capture.strokes,
        capture.events,
        capture.transcript,
        capture.video,
        capture.audio,
        capture.note,
        extras,
        &options,
    )
}

/// Collects a capture's frames, then builds its bundle.
#[wasm_bindgen]
#[derive(Default)]
pub struct Compiler {
    frames: Vec<RgbaFrame>,
}

#[wasm_bindgen]
impl Compiler {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the page as it looked `t_ms` into the capture: `rgba` is `ImageData.data`.
    #[wasm_bindgen(js_name = addFrame)]
    pub fn add_frame(&mut self, t_ms: f64, width: u32, height: u32, rgba: Vec<u8>) {
        let frame = RgbaFrame {
            t_ms: t_ms.max(0.0).round() as u64,
            width,
            height,
            rgba,
            badge_top_right: false,
        };
        self.frames.push(frame);
    }

    /// How many frames were added.
    #[wasm_bindgen(getter, js_name = frameCount)]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Builds the bundle from the frames added so far and `capture` (a [`Capture`] as JSON). The
    /// frames are used up.
    pub fn compile(&mut self, capture: &str) -> Result<Bundle, JsError> {
        let capture: Capture = serde_json::from_str(capture)
            .context("The capture isn't valid JSON for a Scrawl capture")
            .map_err(js_error)?;
        let built = compile(capture, std::mem::take(&mut self.frames)).map_err(js_error)?;
        Ok(Bundle { built })
    }
}

fn js_error(error: anyhow::Error) -> JsError {
    JsError::new(&format!("{error:#}"))
}

/// A built bundle's files.
#[wasm_bindgen]
pub struct Bundle {
    built: BuiltBundle,
}

#[wasm_bindgen]
impl Bundle {
    /// Every file's path inside the bundle, in the order to save them (`summary.md` last).
    #[wasm_bindgen(js_name = fileNames)]
    pub fn file_names(&self) -> Vec<String> {
        self.built
            .files
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The bytes of the file at `name`, or `undefined`.
    pub fn file(&self, name: &str) -> Option<Vec<u8>> {
        self.built
            .files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, bytes)| bytes.clone())
    }

    /// `summary.md`: the text to hand an agent.
    #[wasm_bindgen(getter)]
    pub fn summary(&self) -> String {
        self.built.summary.clone()
    }

    /// `scrawl.json`.
    #[wasm_bindgen(getter)]
    pub fn manifest(&self) -> String {
        serde_json::to_string_pretty(&self.built.bundle).unwrap_or_default()
    }
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
