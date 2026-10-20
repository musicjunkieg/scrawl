//! The screen: one recording in progress and what it leaves behind.

use std::path::PathBuf;

use crate::model::RgbaFrame;

/// What a finished screen capture leaves behind.
pub struct ScreenRecording {
    /// Small candidate frames for keyframe selection.
    pub frames: Vec<RgbaFrame>,
    pub video: Option<PathBuf>,
    pub duration_ms: u64,
    /// The capture time (on the frames' clock) of the video's start.
    pub video_offset_ms: u64,
}

/// One screen recording in progress. The app's `ScreenCaptureSession` records with the
/// platform's capture API; another recorder only has to hand back a `ScreenRecording` when
/// stopped.
pub trait ScreenCapture: Send {
    /// Takes a candidate frame now (a click or a mark) instead of at the next interval.
    fn request_immediate_frame(&self);
    fn elapsed_ms(&self) -> u64;
    fn stop(self: Box<Self>) -> ScreenRecording;
}
