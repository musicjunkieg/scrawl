//! The receiver as an MCP server, so any agent (Claude Code, Cursor, OpenCode) can look at the
//! bundles people send it: `POST /mcp`, the Streamable HTTP transport answered with plain JSON (no
//! event stream, no sessions), behind the same bearer token as the rest of the receiver.
//!
//! - `list_scrawls`: the newest bundles.
//! - `get_scrawl`: one bundle (the newest unless named): `summary.md` and the contact sheet.
//! - `get_scrawl_file`: one of its files, a frame as an image or a text file as text.

use base64::Engine;
use serde_json::{Value, json};

use crate::{Response, Store, entry_of, newest, safe_part, safe_path};

/// The largest MCP message accepted: a tool call is a few hundred bytes.
pub const MAX_MESSAGE: usize = 1024 * 1024;

/// The protocol versions this server speaks, newest first.
const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Images bigger than this aren't sent inline (agents' limits are around 5 MB).
const MAX_IMAGE: usize = 5 * 1024 * 1024;

/// Text bigger than this isn't sent inline: a summary is a few KB, and a reply is built in memory
/// a few times over, which a Worker's 128 MB couldn't hold for a file near the upload limit.
const MAX_TEXT: usize = 1024 * 1024;

const INSTRUCTIONS: &str = "Scrawl recordings are how people show you something: a screen \
recording compiled into summary.md (what they said, clicked and drew, time-aligned, with \
numbered keyframes) and images of those keyframes. When someone says \"look at my scrawl\" or \
\"the last scrawl\", call get_scrawl (the newest unless they name one), and read the summary \
together with the contact sheet: the red ink and click rings mark what they meant. Open one \
frame at full size with get_scrawl_file.";

pub(crate) async fn handle(
    method: &str,
    body: &[u8],
    store: &impl Store,
) -> Result<Response, String> {
    if method != "POST" {
        // No event stream to open: everything is answered in the POST's reply.
        return Ok(Response::error(405, "MCP here is POST only"));
    }
    if body.len() > MAX_MESSAGE {
        return Ok(Response::error(413, "the message is too large"));
    }
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return Ok(rpc_error(Value::Null, -32700, "Parse error"));
    };
    let Some(message) = message.as_object() else {
        return Ok(rpc_error(Value::Null, -32600, "Invalid request"));
    };
    let id = message.get("id").cloned();
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Ok(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid request: not JSON-RPC 2.0",
        ));
    }
    let Some(name) = message.get("method").and_then(Value::as_str) else {
        // A reply to something this server never asks, or nothing at all.
        return Ok(match id {
            Some(_) if message.contains_key("result") || message.contains_key("error") => {
                accepted()
            }
            _ => rpc_error(id.unwrap_or(Value::Null), -32600, "Invalid request"),
        });
    };
    let Some(id) = id else {
        // A notification (`notifications/initialized`, a cancel): nothing to answer.
        return Ok(accepted());
    };
    // A whole number written `1.0` is an integer too, as MCP's schema reads it.
    let integral = id.is_i64() || id.is_u64() || id.as_f64().is_some_and(|n| n.fract() == 0.0);
    if !id.is_string() && !integral {
        return Ok(rpc_error(
            Value::Null,
            -32600,
            "Invalid request: id must be a string or integer",
        ));
    }
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match name {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        // A storage failure is still an answer to this request, with its id.
        "tools/call" => call(&params, store)
            .await
            .unwrap_or_else(|error| Err((-32603, format!("Storage error: {error}")))),
        _ => Err((-32601, format!("Method not found: {name}"))),
    };
    Ok(match result {
        Ok(result) => Response::json(
            200,
            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        ),
        Err((code, text)) => rpc_error(id, code, &text),
    })
}

fn accepted() -> Response {
    Response {
        status: 202,
        content_type: "application/json",
        body: Vec::new(),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    Response::json(
        200,
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    )
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| VERSIONS.contains(v))
        .unwrap_or(VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "scrawl-server", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "list_scrawls",
            "title": "List Scrawl recordings",
            "description": "The newest Scrawl recordings people have sent, newest first: each one's id, when it was made, how long it is, the app and device it was recorded on, and its files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "How many (10 if left out)." }
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_scrawl",
            "title": "Look at a Scrawl recording",
            "description": "One Scrawl recording: its summary.md (the person's words, clicks and marks, time-aligned, with numbered keyframes) and its contact sheet (the keyframes in one image, numbered, with their red ink and click rings). The newest recording unless an id is given.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "A recording's id from list_scrawls; the newest if left out." }
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_scrawl_file",
            "title": "Open a file of a Scrawl recording",
            "description": "One file of a Scrawl recording: a keyframe (frames/frame-01.png, …) as an image at full size, or a text file (scrawl.json, summary.md) as text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The recording's id." },
                    "path": { "type": "string", "description": "The file inside it, such as frames/frame-02.png." }
                },
                "required": ["id", "path"]
            },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

/// A tool's result, `Err` for a call that isn't valid at all (an unknown tool).
type Called = Result<Value, (i64, String)>;

async fn call(params: &Value, store: &impl Store) -> Result<Called, String> {
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    match params.get("name").and_then(Value::as_str) {
        Some("list_scrawls") => {
            let limit = arguments
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(10)
                .clamp(1, 50) as usize;
            list_scrawls(limit, store).await.map(Ok)
        }
        Some("get_scrawl") => get_scrawl(text("id"), store).await.map(Ok),
        Some("get_scrawl_file") => match (text("id"), text("path")) {
            (Some(id), Some(path)) => get_scrawl_file(&id, &path, store).await.map(Ok),
            _ => Ok(Ok(tool_error("get_scrawl_file needs an id and a path"))),
        },
        Some(other) => Ok(Err((-32602, format!("Unknown tool: {other}")))),
        None => Ok(Err((-32602, "tools/call needs a tool name".to_owned()))),
    }
}

fn tool_error(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

async fn list_scrawls(limit: usize, store: &impl Store) -> Result<Value, String> {
    let bundles = newest(store, limit).await?;
    if bundles.is_empty() {
        return Ok(
            json!({ "content": [{ "type": "text", "text": "No Scrawl recordings have been sent here yet." }] }),
        );
    }
    let lines: Vec<String> = bundles
        .iter()
        .map(|b| {
            let from = match (&b.app, &b.device) {
                (Some(app), Some(device)) => format!(" · {app} on {device}"),
                (Some(app), None) => format!(" · {app}"),
                (None, Some(device)) => format!(" · {device}"),
                (None, None) => String::new(),
            };
            format!(
                "- {} · {} · {}{} · {} files",
                b.id,
                utc(b.created_at_ms),
                clock(b.duration_ms),
                from,
                b.files.len()
            )
        })
        .collect();
    Ok(json!({
        "content": [{ "type": "text", "text": lines.join("\n") }],
        "structuredContent": { "scrawls": bundles },
    }))
}

async fn get_scrawl(id: Option<String>, store: &impl Store) -> Result<Value, String> {
    let entry = match id {
        Some(id) => entry_of(&id, store).await?,
        None => newest(store, 1).await?.into_iter().next(),
    };
    let Some(entry) = entry else {
        return Ok(tool_error(
            "There's no such Scrawl recording here; list_scrawls shows what there is.",
        ));
    };
    let folder = format!("bundles/{}.scrawl", entry.id);
    let mut content = Vec::new();
    match store.get(&format!("{folder}/summary.md")).await? {
        Some(summary) if summary.len() <= MAX_TEXT => {
            content.push(json!({ "type": "text", "text": String::from_utf8_lossy(&summary) }));
        }
        Some(_) => content.push(json!({ "type": "text", "text": format!(
            "Scrawl recording {}'s summary.md is too large to send here; it can be downloaded from GET /bundles/{}/summary.md.",
            entry.id, entry.id
        ) })),
        None => content.push(json!({ "type": "text", "text": format!("Scrawl recording {} has no summary.md.", entry.id) })),
    }
    if let Some(sheet) = store.get(&format!("{folder}/contact-sheet.png")).await?
        && sheet.len() <= MAX_IMAGE
    {
        content.push(image(&sheet, "image/png"));
    }
    let frames: Vec<&str> = entry
        .files
        .iter()
        .map(String::as_str)
        .filter(|f| f.starts_with("frames/"))
        .collect();
    content.push(json!({
        "type": "text",
        "text": format!(
            "Paths in the summary are where the recording was made. Here it is Scrawl recording {}: open a frame at full size with get_scrawl_file (id \"{}\", path such as \"{}\"). Its files: {}.",
            entry.id,
            entry.id,
            frames.first().copied().unwrap_or("frames/frame-01.png"),
            entry.files.join(", ")
        ),
    }));
    Ok(json!({ "content": content }))
}

async fn get_scrawl_file(id: &str, path: &str, store: &impl Store) -> Result<Value, String> {
    if !safe_part(id) || !safe_path(path) {
        return Ok(tool_error("There's no such file."));
    }
    let Some(bytes) = store.get(&format!("bundles/{id}.scrawl/{path}")).await? else {
        return Ok(tool_error(
            "There's no such file; get_scrawl lists the recording's files.",
        ));
    };
    let kind = crate::content_type(path);
    Ok(match kind {
        "image/png" | "image/jpeg" if bytes.len() <= MAX_IMAGE => {
            json!({ "content": [image(&bytes, kind)] })
        }
        "image/png" | "image/jpeg" => tool_error("That image is too large to send here."),
        _ if (kind.starts_with("text/") || kind == "application/json")
            && bytes.len() <= MAX_TEXT =>
        {
            json!({ "content": [{ "type": "text", "text": String::from_utf8_lossy(&bytes) }] })
        }
        _ if kind.starts_with("text/") || kind == "application/json" => tool_error(&format!(
            "{path} is too large to send here; it can be downloaded from GET /bundles/{id}/{path}."
        )),
        _ => tool_error(&format!(
            "{path} isn't an image or text, so it can't be shown here; it can be downloaded from GET /bundles/{id}/{path}."
        )),
    })
}

fn image(bytes: &[u8], kind: &str) -> Value {
    json!({
        "type": "image",
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        "mimeType": kind,
    })
}

/// `M:SS` for a duration in milliseconds.
fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A time in milliseconds since 1970 as `YYYY-MM-DD HH:MM UTC`.
fn utc(ms: u64) -> String {
    let seconds = ms / 1000;
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_761_777_000_000), "2025-10-29 22:30 UTC");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00 UTC");
        assert_eq!(clock(18_400), "0:18");
        assert_eq!(clock(125_000), "2:05");
    }
}
