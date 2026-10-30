//! Scrawl's receiver, without the HTTP server or the disk: `handle` turns a request into a reply
//! and reads and writes through a [`Store`]. `workers/scrawl-server` runs it on Cloudflare Workers
//! with R2 as the store; tests run it with [`MemoryStore`]. See `docs/receiver.md`.
//!
//! - `POST /bundles`: a zip of one `<id>.scrawl/` folder. Checked (token, size, paths,
//!   `scrawl.json`), kept under `bundles/<id>.scrawl/`, answered as the Mac app's ScrawlKit ingest
//!   answers (`{ok, delivered, state, …}`), so ScrawlKit's `IngestReply` reads it unchanged. A
//!   bundle already here gets the same answer and isn't written again.
//! - `GET /bundles`: the newest bundles first (`?limit=`, 20 by default, 100 at most).
//! - `GET /bundles/<id>`: one bundle's entry; `GET /bundles/<id>/<file>`: one of its files.
//! - `POST /mcp`: the bundles for agents, over MCP (`mcp.rs`).
//! - `GET /health`: whether the token is right (ScrawlKit asks before it sends).
//! - `GET /`: says the receiver is up, without a token.
//!
//! Every other request needs `Authorization: Bearer <token>`, or `X-Scrawl-Token: <token>` (see
//! [`authorization`]). The token reads and sends everything; the upload token, the one an app
//! carries (anyone can pull it out of the app), only sends bundles and checks itself at `/health`.

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

mod mcp;
pub mod zip;

/// 40 MB. A Worker has 128 MB of memory, and an upload is held twice while it's unpacked (the
/// zip, then its files), so this leaves room; Workers itself takes bodies up to 100 MB.
pub const DEFAULT_MAX_UPLOAD: usize = 40 * 1024 * 1024;

pub struct Config {
    /// The bearer token every request but `GET /` needs. Empty refuses everything.
    pub token: String,
    /// A second token that can only send bundles (`POST /bundles`, `GET /health`), for apps to
    /// carry. Empty: none.
    pub upload_token: String,
    /// The largest upload accepted, in bytes, zipped or unpacked.
    pub max_upload: usize,
}

pub struct Request {
    pub method: String,
    /// The path and query, `/bundles?limit=5`.
    pub path: String,
    pub authorization: Option<String>,
    pub body: Vec<u8>,
    /// Now, in milliseconds since 1970: when an upload arrived.
    pub now_ms: u64,
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// What went wrong, for the server's own log; never sent, since a storage error can name
    /// the provider's internals.
    pub log: Option<String>,
}

impl Response {
    fn json(status: u16, value: &impl Serialize) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(value).unwrap_or_default(),
            log: None,
        }
    }

    /// A 500 that says only that storage failed, with the reason kept for the log.
    fn storage_failed(error: String) -> Self {
        Self {
            log: Some(format!("storage: {error}")),
            ..Self::error(500, "storage failed")
        }
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(
            status,
            &serde_json::json!({ "ok": false, "error": message }),
        )
    }
}

/// Where bundles are kept: Cloudflare R2, a folder, or memory. Keys are `/`-separated paths.
#[allow(async_fn_in_trait)]
pub trait Store {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), String>;
    /// Up to `limit` keys starting with `prefix`, in byte order.
    async fn list(&self, prefix: &str, limit: usize) -> Result<Vec<String>, String>;
}

/// A [`Store`] in memory, for tests and trying the receiver out.
#[derive(Default)]
pub struct MemoryStore {
    pub objects: RefCell<BTreeMap<String, (Vec<u8>, String)>>,
}

impl Store for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .objects
            .borrow()
            .get(key)
            .map(|(bytes, _)| bytes.clone()))
    }

    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), String> {
        self.objects
            .borrow_mut()
            .insert(key.to_owned(), (bytes, content_type.to_owned()));
        Ok(())
    }

    async fn list(&self, prefix: &str, limit: usize) -> Result<Vec<String>, String> {
        Ok(self
            .objects
            .borrow()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .take(limit)
            .cloned()
            .collect())
    }
}

/// What the list shows of one bundle, kept beside it at `index/…`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub created_at_ms: u64,
    pub received_at_ms: u64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Its files, relative to the bundle folder.
    pub files: Vec<String>,
    pub bytes: u64,
}

/// The parts of `scrawl.json` the receiver reads. Everything else is kept as uploaded.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    version: u32,
    #[serde(default)]
    created_at_ms: u64,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    target: Option<MetaTarget>,
    #[serde(default)]
    device: Option<MetaDevice>,
}

#[derive(Deserialize)]
struct MetaTarget {
    #[serde(default)]
    app: Option<String>,
}

#[derive(Deserialize)]
struct MetaDevice {
    #[serde(default)]
    name: Option<String>,
}

/// What can be answered before the body is read, from the method, path, `Authorization` and
/// `Content-Length` headers: a missing token, an upload that says it's too big, or one that
/// doesn't say how big it is (browsers, curl and URLSession all do). A front end calls this
/// first, so no body is read that the limit couldn't bound; `handle` checks the size again.
pub fn answer_before_body(
    config: &Config,
    method: &str,
    path: &str,
    authorization: Option<&str>,
    content_length: Option<u64>,
) -> Option<Response> {
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let limit = match parts.first() {
        Some(&"bundles") => config.max_upload,
        Some(&"mcp") => mcp::MAX_MESSAGE,
        _ => return None,
    };
    if let Err(response) = authorize(config, authorization, method, &parts) {
        return Some(response);
    }
    if method != "POST" {
        return None;
    }
    match content_length {
        None => Some(Response::error(
            411,
            "send the upload with its Content-Length",
        )),
        Some(n) if n > limit as u64 => Some(too_big()),
        Some(_) => None,
    }
}

/// The `Authorization` value to check, from the request's `Authorization` and `X-Scrawl-Token`
/// headers. ScrawlKit sends both: iOS may drop or replace `Authorization` in a background upload,
/// since Apple reserves it for the URL loading system, so either one carrying a token is enough.
pub fn authorization(
    config: &Config,
    header: Option<String>,
    scrawl_token: Option<String>,
) -> Option<String> {
    let from_header = scrawl_token.map(|token| format!("Bearer {}", token.trim()));
    match (header, from_header) {
        (Some(header), Some(other)) if access(config, Some(&header)).is_err() => Some(other),
        (header, other) => header.or(other),
    }
}

/// Whether this request sends a bundle with the upload token, the one an app carries: what a
/// front end with a rate limiter counts (`workers/scrawl-server` allows 20 a minute from one
/// address), so a token pulled out of an app can't fill the bucket quickly. The owner's token
/// isn't counted.
pub fn counts_toward_upload_limit(
    config: &Config,
    method: &str,
    path: &str,
    authorization: Option<&str>,
) -> bool {
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    method == "POST"
        && parts == ["bundles"]
        && matches!(access(config, authorization), Ok(Access::Upload))
}

/// The answer once an address has sent more than its share. ScrawlKit's outbox waits and tries
/// again on a 429.
pub fn too_many_uploads() -> Response {
    Response::error(429, "too many uploads from here; try again in a minute")
}

fn too_big() -> Response {
    Response::error(413, "the bundle is larger than this receiver accepts")
}

pub async fn handle(config: &Config, mut request: Request, store: &impl Store) -> Response {
    let body = std::mem::take(&mut request.body);
    let (path, query) = request
        .path
        .split_once('?')
        .unwrap_or((request.path.as_str(), ""));
    let parts: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    let method = request.method.as_str();
    if parts.is_empty() {
        return match method {
            "GET" | "HEAD" => Response {
                status: 200,
                content_type: "text/plain; charset=utf-8",
                body: b"scrawl-server\n".to_vec(),
                log: None,
            },
            _ => Response::error(405, "not allowed here"),
        };
    }
    if parts[0] != "bundles" && parts != ["mcp"] && parts != ["health"] {
        return Response::error(404, "not found");
    }
    if let Err(response) = authorize(config, request.authorization.as_deref(), method, &parts) {
        return response;
    }
    if parts == ["health"] {
        return match method {
            "GET" | "HEAD" => Response::json(200, &serde_json::json!({ "ok": true })),
            _ => Response::error(405, "not allowed here"),
        };
    }
    if parts == ["mcp"] {
        return mcp::handle(method, &body, store)
            .await
            .unwrap_or_else(Response::storage_failed);
    }
    let result = match (method, &parts[1..]) {
        ("POST", []) => upload(config, body, request.now_ms, store).await,
        ("GET", []) => list(query, store).await,
        ("GET", [id]) => entry(id, store).await,
        ("GET", [id, file @ ..]) => file_of(id, &file.join("/"), store).await,
        _ => Ok(Response::error(405, "not allowed here")),
    };
    result.unwrap_or_else(Response::storage_failed)
}

/// What a token lets its holder do.
#[derive(Debug, PartialEq)]
enum Access {
    Everything,
    /// Send bundles and check the token, nothing that reads them back.
    Upload,
}

fn access(config: &Config, header: Option<&str>) -> Result<Access, Response> {
    if config.token.is_empty() {
        return Err(Response::error(500, "the receiver has no token set"));
    }
    let given = header.and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    if same(given, &config.token) {
        Ok(Access::Everything)
    } else if !config.upload_token.is_empty() && same(given, &config.upload_token) {
        Ok(Access::Upload)
    } else {
        Err(Response::error(401, "wrong or missing token"))
    }
}

/// Compares every byte, so how long it takes doesn't tell how much of a guess was right.
fn same(given: &str, token: &str) -> bool {
    let (a, b) = (given.as_bytes(), token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether the token in `header` may make this request (`parts` is the path, split on `/`).
fn authorize(
    config: &Config,
    header: Option<&str>,
    method: &str,
    parts: &[&str],
) -> Result<(), Response> {
    let uploading = matches!(
        (method, parts),
        ("POST", ["bundles"]) | ("GET" | "HEAD", ["health"])
    );
    match access(config, header)? {
        Access::Everything => Ok(()),
        Access::Upload if uploading => Ok(()),
        Access::Upload => Err(Response::error(403, "this token can only send bundles")),
    }
}

/// A bundle id or one part of a path inside it: letters, digits, `.`, `-`, `_`, not starting
/// with `.` (so no `..` and no hidden files).
fn safe_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('.')
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn safe_path(path: &str) -> bool {
    path.split('/').all(safe_part)
}

fn index_key(entry: &Entry) -> String {
    // Newest first in byte order: the time counted down from the largest 20-digit number.
    format!("index/{:020}-{}", u64::MAX - entry.created_at_ms, entry.id)
}

fn saved_reply(id: &str) -> Response {
    Response::json(
        200,
        &serde_json::json!({ "ok": true, "delivered": false, "state": "saved", "id": id }),
    )
}

async fn upload(
    config: &Config,
    body: Vec<u8>,
    now_ms: u64,
    store: &impl Store,
) -> Result<Response, String> {
    if body.len() > config.max_upload {
        return Ok(too_big());
    }
    let files = match zip::unzip(&body, config.max_upload as u64) {
        Ok(files) => files,
        Err(zip::ZipError::TooBig) => return Ok(too_big()),
        Err(zip::ZipError::Bad(why)) => return Ok(Response::error(400, &why)),
    };
    // The files are out; the zip needn't sit in memory beside them.
    drop(body);

    // One `<id>.scrawl/` folder, every path inside it. What Finder and the Mac app add on their
    // own (`__MACOSX/`, `.DS_Store`, `.deliver.json`) is left out.
    let mut id: Option<String> = None;
    let mut inside: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
    for (name, bytes) in files {
        if name.split('/').any(|part| part == "." || part == "..") {
            return Ok(Response::error(
                400,
                &format!("a file name isn't allowed: {name}"),
            ));
        }
        if name.starts_with("__MACOSX/") || name.split('/').any(|part| part.starts_with('.')) {
            continue;
        }
        let Some((root, rest)) = name.split_once('/') else {
            return Ok(Response::error(
                400,
                "the zip should hold one <id>.scrawl folder",
            ));
        };
        let Some(this) = root.strip_suffix(".scrawl").filter(|i| safe_part(i)) else {
            return Ok(Response::error(
                400,
                "the zip should hold one <id>.scrawl folder",
            ));
        };
        if id.as_deref().is_some_and(|i| i != this) {
            return Ok(Response::error(400, "the zip holds more than one bundle"));
        }
        if !safe_path(rest) {
            return Ok(Response::error(
                400,
                &format!("a file name isn't allowed: {name}"),
            ));
        }
        id = Some(this.to_owned());
        inside.push((rest.to_owned(), bytes));
    }
    let Some(id) = id else {
        return Ok(Response::error(400, "the zip is empty"));
    };
    let Some(at) = inside.iter().position(|(path, _)| path == "scrawl.json") else {
        return Ok(Response::error(400, "the bundle has no scrawl.json"));
    };
    let (_, scrawl_json) = inside.remove(at);
    let meta: Meta = match serde_json::from_slice(&scrawl_json) {
        Ok(meta) => meta,
        Err(error) => {
            return Ok(Response::error(
                400,
                &format!("scrawl.json doesn't parse: {error}"),
            ));
        }
    };
    if meta.version != 1 {
        return Ok(Response::error(400, "scrawl.json isn't version 1"));
    }

    let folder = format!("bundles/{id}.scrawl");
    // `<folder>.json` is written last: a bundle counts as here only once it is, so an upload cut
    // off partway is simply written again when it's sent again.
    if store.get(&format!("{folder}.json")).await?.is_some() {
        return Ok(saved_reply(&id));
    }
    let entry = Entry {
        id: id.clone(),
        created_at_ms: meta.created_at_ms,
        received_at_ms: now_ms,
        duration_ms: meta.duration_ms,
        app: meta.target.and_then(|t| t.app).filter(|a| !a.is_empty()),
        device: meta.device.and_then(|d| d.name).filter(|d| !d.is_empty()),
        files: std::iter::once("scrawl.json".to_owned())
            .chain(inside.iter().map(|(path, _)| path.clone()))
            .collect(),
        bytes: (scrawl_json.len() + inside.iter().map(|(_, b)| b.len()).sum::<usize>()) as u64,
    };
    // Each file is handed over, not copied, so only one bundle's worth stays in memory.
    for (path, bytes) in inside {
        let kind = content_type(&path);
        store.put(&format!("{folder}/{path}"), bytes, kind).await?;
    }
    store
        .put(
            &format!("{folder}/scrawl.json"),
            scrawl_json,
            "application/json",
        )
        .await?;
    let entry_json = serde_json::to_vec(&entry).unwrap_or_default();
    store
        .put(&index_key(&entry), entry_json.clone(), "application/json")
        .await?;
    store
        .put(&format!("{folder}.json"), entry_json, "application/json")
        .await?;
    Ok(saved_reply(&id))
}

async fn list(query: &str, store: &impl Store) -> Result<Response, String> {
    let limit = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("limit="))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 100);
    let bundles = newest(store, limit).await?;
    Ok(Response::json(
        200,
        &serde_json::json!({ "bundles": bundles }),
    ))
}

/// Up to `limit` bundles' entries, newest first.
async fn newest(store: &impl Store, limit: usize) -> Result<Vec<Entry>, String> {
    let mut bundles = Vec::new();
    for key in store.list("index/", limit).await? {
        if let Some(bytes) = store.get(&key).await?
            && let Ok(entry) = serde_json::from_slice::<Entry>(&bytes)
        {
            bundles.push(entry);
        }
    }
    Ok(bundles)
}

/// One bundle's entry, if it's here.
async fn entry_of(id: &str, store: &impl Store) -> Result<Option<Entry>, String> {
    if !safe_part(id) {
        return Ok(None);
    }
    Ok(store
        .get(&format!("bundles/{id}.scrawl.json"))
        .await?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
}

async fn entry(id: &str, store: &impl Store) -> Result<Response, String> {
    if !safe_part(id) {
        return Ok(Response::error(404, "no such bundle"));
    }
    Ok(
        match store.get(&format!("bundles/{id}.scrawl.json")).await? {
            Some(bytes) => Response {
                status: 200,
                content_type: "application/json",
                body: bytes,
                log: None,
            },
            None => Response::error(404, "no such bundle"),
        },
    )
}

async fn file_of(id: &str, path: &str, store: &impl Store) -> Result<Response, String> {
    if !safe_part(id) || !safe_path(path) {
        return Ok(Response::error(404, "no such file"));
    }
    Ok(
        match store.get(&format!("bundles/{id}.scrawl/{path}")).await? {
            Some(bytes) => Response {
                status: 200,
                content_type: content_type(path),
                body: bytes,
                log: None,
            },
            None => Response::error(404, "no such file"),
        },
    )
}

/// The type a bundle's file is served as, from its extension.
pub fn content_type(path: &str) -> &'static str {
    match path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("json") => "application/json",
        Some("md") => "text/markdown; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests;
