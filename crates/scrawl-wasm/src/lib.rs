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
//! // Optionally, with small candidates: the keyframes again at full size.
//! for (const t of compiler.keyframeTimes(JSON.stringify(capture))) compiler.addFullFrame(t, …);
//! const bundle = compiler.compile(JSON.stringify(capture));
//! for (const name of bundle.fileNames()) save(name, bundle.file(name));
//! ```

use anyhow::Context;
use scrawl_core::build::BuiltBundle;
pub use scrawl_core::capture::{Capture, compile, compile_with_full, keyframe_times};
use scrawl_core::model::RgbaFrame;
use wasm_bindgen::prelude::*;

/// Collects a capture's frames, then builds its bundle.
#[wasm_bindgen]
#[derive(Default)]
pub struct Compiler {
    frames: Vec<RgbaFrame>,
    full: Vec<RgbaFrame>,
}

#[wasm_bindgen]
impl Compiler {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the page as it looked `t_ms` into the capture: `rgba` is `ImageData.data`. These
    /// candidates can be small (the Mac app uses 480 px) when the keyframes are added again at
    /// full size with `addFullFrame`.
    #[wasm_bindgen(js_name = addFrame)]
    pub fn add_frame(&mut self, t_ms: f64, width: u32, height: u32, rgba: Vec<u8>) {
        self.frames.push(rgba_frame(t_ms, width, height, rgba));
    }

    /// The times (ms) of the keyframes `compile` will pick from the candidates added so far, for
    /// `capture` (a [`Capture`] as JSON).
    #[wasm_bindgen(js_name = keyframeTimes)]
    pub fn keyframe_times(&self, capture: &str) -> Result<Vec<f64>, JsError> {
        let capture = parse(capture)?;
        let times = keyframe_times(&capture, &self.frames).map_err(js_error)?;
        Ok(times.into_iter().map(|t| t as f64).collect())
    }

    /// Adds a keyframe at full size, at one of `keyframeTimes`; the bundle's images are made from
    /// these instead of the candidates.
    #[wasm_bindgen(js_name = addFullFrame)]
    pub fn add_full_frame(&mut self, t_ms: f64, width: u32, height: u32, rgba: Vec<u8>) {
        self.full.push(rgba_frame(t_ms, width, height, rgba));
    }

    /// How many frames were added.
    #[wasm_bindgen(getter, js_name = frameCount)]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Builds the bundle from the frames added so far and `capture` (a [`Capture`] as JSON). The
    /// frames are used up.
    pub fn compile(&mut self, capture: &str) -> Result<Bundle, JsError> {
        let capture = parse(capture)?;
        let frames = std::mem::take(&mut self.frames);
        let full = std::mem::take(&mut self.full);
        let built = compile_with_full(capture, frames, full).map_err(js_error)?;
        Ok(Bundle { built })
    }
}

fn rgba_frame(t_ms: f64, width: u32, height: u32, rgba: Vec<u8>) -> RgbaFrame {
    RgbaFrame {
        t_ms: t_ms.max(0.0).round() as u64,
        width,
        height,
        rgba,
        badge_top_right: false,
    }
}

fn parse(capture: &str) -> Result<Capture, JsError> {
    serde_json::from_str(capture)
        .context("The capture isn't valid JSON for a Scrawl capture")
        .map_err(js_error)
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
