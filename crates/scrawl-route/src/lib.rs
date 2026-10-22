//! Where a Scrawl bundle goes once it is built.
//!
//! - [`delivery`]: what became of a bundle sent to an agent session, what a phone is told about
//!   it, and the queue that retries a send that failed.
//! - [`opencode`]: OpenCode's session API (look a session up, search the open root sessions, pick
//!   the one a capture goes to), over a transport the caller passes in.
//!
//! Nothing here sends a bundle itself: callers pass a [`delivery::Forward`] that does (the Scrawl
//! app's queues it into an OpenCode session), and a [`Log`] for the lines worth keeping.

pub mod delivery;
pub mod opencode;

/// Where a crate function writes a line worth keeping (the Scrawl app's log, stderr, or nowhere).
pub type Log = fn(&str);

/// Milliseconds since the Unix epoch.
pub(crate) fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
