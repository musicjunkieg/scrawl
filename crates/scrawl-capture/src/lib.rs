//! What a Scrawl capture records, as traits: a screen recording (`screen::ScreenCapture`), the
//! narration (`audio::AudioCapture`), the window and cursor log (`activity::ActivityCapture`) and
//! the engines that transcribe it (`transcribe::Transcriber`), with the recordings each hands back
//! when stopped. The engines make no OS calls (`docs/library.md`); `native` (the default
//! feature, macOS and Windows only) holds the window-server sources the Scrawl app records with,
//! and another app can bring its own sources instead.

pub use scrawl_bundle as model;

pub mod activity;
pub mod audio;
#[cfg(all(feature = "native", any(target_os = "macos", target_os = "windows")))]
pub mod native;
pub mod screen;
pub mod transcribe;
