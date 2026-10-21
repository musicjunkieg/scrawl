//! What a Scrawl capture records, as traits: a screen recording (`screen::ScreenCapture`), the
//! narration (`audio::AudioCapture`), the window and cursor log (`activity::ActivityCapture`) and
//! the engines that transcribe it (`transcribe::Transcriber`), with the recordings each hands back
//! when stopped. No OS calls (`docs/library.md`); the app in the repository root implements them
//! with ScreenCaptureKit, Windows.Graphics.Capture, the microphone and the window server, and
//! another app can implement them with its own sources.

pub use scrawl_bundle as model;

pub mod activity;
pub mod audio;
pub mod screen;
pub mod transcribe;
