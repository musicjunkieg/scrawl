//! The macOS and Windows sources behind the traits (the default `native` feature): what the
//! Scrawl app records with, for other apps to record with too. Without the feature, or on
//! another system, the crate has no OS bindings.

pub mod mic;
#[cfg(target_os = "macos")]
pub mod sck;
#[cfg(target_os = "windows")]
pub mod wgc;
#[cfg(target_os = "windows")]
pub mod win;
pub mod window_server;

use std::time::Instant;

use crate::model::RgbaFrame;

/// What a screen sampler (`sck` on macOS, `wgc` on Windows) tells its app, and the frame it
/// records when it can't take one.
#[derive(Clone, Copy)]
pub struct SamplerHooks {
    /// Where its log lines go.
    pub log: Option<fn(&str)>,
    /// Called once the recorder is set up, with the capture's start.
    pub ready: Option<fn(Instant)>,
    /// The candidate frame when a screenshot fails or there is no screen to record: its time
    /// (ms) and how many urgent frames came before it.
    pub placeholder: fn(u64, u32) -> RgbaFrame,
}

impl SamplerHooks {
    pub(crate) fn log(&self, line: &str) {
        if let Some(log) = self.log {
            log(line);
        }
    }
}

impl Default for SamplerHooks {
    fn default() -> Self {
        Self {
            log: None,
            ready: None,
            placeholder: blank_frame,
        }
    }
}

/// A plain dark 1280 by 800 frame.
pub fn blank_frame(t_ms: u64, _urgent: u32) -> RgbaFrame {
    RgbaFrame::new(t_ms, 1280, 800, [24, 24, 24, 255].repeat(1280 * 800))
}
