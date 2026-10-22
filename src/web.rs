//! Dev pages telling Scrawl which OpenCode session they belong to: `scrawl_route::web` (the
//! `/sdk.js` script, its registrations and routes on the ingest port), with Scrawl's ingest token,
//! log and `agentContext` experiment.

use std::net::TcpStream;

use anyhow::Result;

pub use scrawl_route::web::{Registration, fetch_registrations, set_serving};

const HOOKS: scrawl_route::web::Hooks = scrawl_route::web::Hooks {
    log: |line| crate::capture::scrawl_log(line),
    wants_context: || crate::handoff::experiment(crate::handoff::AGENT_CONTEXT),
    context: |origin, body| {
        if let Ok(post) = serde_json::from_slice::<crate::page::Post>(body) {
            crate::page::record(origin, post, crate::now_epoch_ms());
        }
    },
};

/// The live registration for `url`, from this process when it runs the ingest server, else
/// from the one that does.
pub fn lookup(url: &str) -> Option<Registration> {
    scrawl_route::web::lookup(url, || {
        // `SCRAWL_INGEST_PORT` points at a `scrawl serve --port` other than the app's.
        let port = std::env::var("SCRAWL_INGEST_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(crate::phone::DEFAULT_INGEST_PORT);
        fetch_registrations(port, &crate::handoff::ensure_ingest_token()?)
    })
}

/// Answers the SDK's routes; `false` when the request is for something else (the ScrawlKit
/// routes, which need the token).
pub fn handle(stream: &mut TcpStream, header: &str, expected_token: &str) -> Result<bool> {
    scrawl_route::web::handle(stream, header, expected_token, &HOOKS)
}
