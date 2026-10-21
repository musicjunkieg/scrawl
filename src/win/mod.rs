//! Windows platform helpers: displays, top-level windows, processes, Scrawl's own window styles,
//! the clipboard, the shell, and the media pipeline. Global coordinates are physical pixels on
//! the virtual screen (Scrawl is per-monitor DPI aware), with the primary display's top-left at
//! the origin, like CoreGraphics' points on macOS.

pub mod clipboard;
pub mod login;
pub mod media;
pub mod overlay;
pub mod privacy;
pub mod shell;
pub mod speech;
pub mod uia;

use std::path::PathBuf;

// Displays, top-level windows and processes are `scrawl-capture`'s (`native::win`).
pub use scrawl_capture::native::win::*;

/// `%LOCALAPPDATA%\Scrawl`: logs and machine-local state.
pub fn local_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Scrawl")
}
