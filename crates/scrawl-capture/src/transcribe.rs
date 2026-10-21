//! Engines that turn recorded speech into a transcript.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::model::TranscriptData;

/// The audio a transcriber hears: 16 kHz mono WAV, both on disk and in memory, so an engine
/// can take whichever it needs.
#[derive(Clone, Copy, Debug)]
pub struct Speech<'a> {
    pub wav_path: &'a Path,
    pub wav_bytes: &'a [u8],
    pub duration_ms: u64,
}

/// One engine that turns recorded speech into a transcript. Engines are tried in order (the
/// app's `transcribe::transcribe_with`); one that returns `Ok(None)` (not connected, no model
/// ready) or an error hands the recording to the next.
pub trait Transcriber {
    /// Names the engine in logs.
    fn name(&self) -> &str;
    fn transcribe(&mut self, speech: Speech<'_>) -> Result<Option<TranscriptData>>;
}

/// The engine that heard a recording while it was made, handed to transcription with it.
/// Copies of the recording share it; the first `take` gets it.
#[derive(Clone, Default)]
pub struct Slot(Arc<Mutex<Option<Box<dyn Transcriber + Send>>>>);

impl Slot {
    pub fn new(engine: Option<Box<dyn Transcriber + Send>>) -> Self {
        Self(Arc::new(Mutex::new(engine)))
    }

    pub fn take(&self) -> Option<Box<dyn Transcriber + Send>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = self.0.lock().map(|slot| slot.is_some()).unwrap_or(false);
        f.debug_tuple("Slot").field(&held).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Silent;

    impl Transcriber for Silent {
        fn name(&self) -> &str {
            "Silent"
        }

        fn transcribe(&mut self, _: Speech<'_>) -> Result<Option<TranscriptData>> {
            Ok(None)
        }
    }

    #[test]
    fn copies_of_a_slot_share_one_engine() {
        let slot = Slot::new(Some(Box::new(Silent)));
        let copy = slot.clone();
        assert_eq!(format!("{slot:?}"), "Slot(true)");
        assert!(copy.take().is_some());
        assert!(slot.take().is_none());
        assert!(Slot::default().take().is_none());
    }
}
