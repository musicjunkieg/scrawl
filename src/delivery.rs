//! What became of a ScrawlKit recording once the Mac took it in, and the retries that finish the
//! job when OpenCode didn't take the bundle: `scrawl_route::delivery`, logging to Scrawl's log.

use std::path::{Path, PathBuf};

pub use scrawl_route::delivery::{
    Delivery, Forward, Outcome, PENDING, Pending, Report, reply_json,
};

use crate::capture::scrawl_log;

fn log(line: &str) {
    scrawl_log(line);
}

/// Queues the bundle at `dir` into `session` through `forward`. If that fails, leaves a `PENDING`
/// marker so the port owner retries it, and says so.
pub fn deliver(
    dir: &Path,
    session: &str,
    title: &str,
    key: Option<&str>,
    forward: Forward,
) -> Delivery {
    scrawl_route::delivery::deliver(dir, session, title, key, forward, log)
}

/// Tries every pending delivery in `captures` that is due at `now`.
pub fn retry_due(captures: &Path, now: u64, forward: Forward) -> Report {
    scrawl_route::delivery::retry_due(captures, now, forward, log)
}

/// Retries pending deliveries in `captures` every 15 s for as long as the process runs;
/// `on_sent` hears about each that went through.
pub fn spawn_retries(captures: PathBuf, forward: Forward, on_sent: fn(&Path, &Pending)) {
    scrawl_route::delivery::spawn_retries(captures, forward, on_sent, log);
}
