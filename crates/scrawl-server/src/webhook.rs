//! The webhook: when a bundle is new here, a POST to an address the owner chose (a chat app, a
//! ticket tracker, the courier), saying which bundle came in. The front end sends it after the
//! upload is answered and retries a few times; this builds the message and signs it.
//!
//! The body is JSON: `event` (`"bundle.saved"`), `id`, `url` (the bundle's entry here, which needs
//! the token), `receivedAtMs`, `createdAtMs`, `durationMs`, `app`, `device`, `summary`
//! (`summary.md`, cut at [`MAX_SUMMARY`] bytes, with `summaryCut: true`), and `text` and
//! `content`, one line for Slack and Discord, which show those. Headers: `X-Scrawl-Event`,
//! `X-Scrawl-Delivery` (the bundle's id, the same on every try, so a receiver can drop repeats),
//! `X-Scrawl-Timestamp` (seconds) and `X-Scrawl-Signature: sha256=<hex>`, an HMAC-SHA256 with the
//! webhook secret of `<timestamp>.<body>` (see [`verify`]).

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{Entry, Store};

pub const EVENT: &str = "bundle.saved";

/// The most of `summary.md` a message carries (16 KB), so a long narration can't make one too
/// big for a chat app.
pub const MAX_SUMMARY: usize = 16 * 1024;

/// How far a timestamp may be from now (5 minutes) before [`verify`] calls it a replay.
pub const TOLERANCE_S: u64 = 300;

/// An HTTPS address with a host, so the message, which holds the narration, isn't sent in the
/// clear.
pub fn allowed(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_some_and(|host| !host.is_empty() && !host.contains('@'))
}

/// The message for `entry`, read with its `summary.md` from `store`. `base` is this receiver's
/// own address, `https://scrawl-server.you.workers.dev`.
pub async fn message(entry: &Entry, base: &str, store: &impl Store) -> Result<Vec<u8>, String> {
    let summary = store
        .get(&format!("bundles/{}.scrawl/summary.md", entry.id))
        .await?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    Ok(body(entry, base, &summary))
}

/// The message's JSON, from the entry and its summary.
pub fn body(entry: &Entry, base: &str, summary: &str) -> Vec<u8> {
    let cut = summary.len() > MAX_SUMMARY;
    let mut end = summary.len().min(MAX_SUMMARY);
    while !summary.is_char_boundary(end) {
        end -= 1;
    }
    let seconds = entry.duration_ms / 1000;
    let line = format!(
        "New Scrawl recording{} ({}:{:02}): {}",
        entry
            .app
            .as_deref()
            .map(|app| format!(" from {app}"))
            .unwrap_or_default(),
        seconds / 60,
        seconds % 60,
        entry.id
    );
    let mut value = serde_json::json!({
        "event": EVENT,
        "id": entry.id,
        "url": format!("{}/bundles/{}", base.trim_end_matches('/'), entry.id),
        "receivedAtMs": entry.received_at_ms,
        "createdAtMs": entry.created_at_ms,
        "durationMs": entry.duration_ms,
        "app": entry.app,
        "device": entry.device,
        "summary": &summary[..end],
        "text": line,
        "content": line,
    });
    if cut {
        value["summaryCut"] = serde_json::Value::Bool(true);
    }
    serde_json::to_vec(&value).unwrap_or_default()
}

/// `sha256=<hex>`: the HMAC-SHA256, keyed with `secret`, of `<timestamp>.<body>`.
pub fn signature(secret: &str, timestamp: u64, body: &[u8]) -> String {
    let digest = keyed(secret, timestamp, body).finalize().into_bytes();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

/// Whether `signature` is this receiver's for `body` at `timestamp`, and `timestamp` is within
/// [`TOLERANCE_S`] of `now` (seconds). Compared in constant time.
pub fn verify(secret: &str, timestamp: u64, body: &[u8], signature: &str, now: u64) -> bool {
    if now.abs_diff(timestamp) > TOLERANCE_S {
        return false;
    }
    let Some(hex) = signature.strip_prefix("sha256=") else {
        return false;
    };
    if hex.len() != 64 || !hex.is_ascii() {
        return false;
    }
    let Ok(bytes) = (0..32)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
    else {
        return false;
    };
    keyed(secret, timestamp, body).verify_slice(&bytes).is_ok()
}

fn keyed(secret: &str, timestamp: u64, body: &[u8]) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(format!("{timestamp}.").as_bytes());
    mac.update(body);
    mac
}
