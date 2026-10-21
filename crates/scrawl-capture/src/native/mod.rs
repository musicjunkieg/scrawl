//! The macOS and Windows sources behind the traits (the default `native` feature): what the
//! Scrawl app records with, for other apps to record with too. Without the feature, or on
//! another system, the crate has no OS bindings.

pub mod mic;
#[cfg(target_os = "macos")]
pub mod sck;
#[cfg(target_os = "windows")]
pub mod win;
pub mod window_server;
