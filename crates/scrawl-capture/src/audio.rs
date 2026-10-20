//! The narration: one recording in progress and the audio it leaves behind.

use anyhow::Result;

/// A finished recording of the narration: 16 kHz mono WAV.
#[derive(Clone, Debug)]
pub struct RecordedAudio {
    pub sample_count: usize,
    pub duration_ms: u64,
    pub wav_bytes: Vec<u8>,
    /// The engine that transcribed while recording, if one did.
    pub live: crate::transcribe::Slot,
}

/// One recording of the narration in progress. The app's `AudioRecorder` records the
/// microphone; another source only has to hand back 16 kHz mono WAV when stopped.
pub trait AudioCapture: Send {
    fn stop(self: Box<Self>) -> Result<RecordedAudio>;
}
