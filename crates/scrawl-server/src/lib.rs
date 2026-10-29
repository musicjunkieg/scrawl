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
//! - `GET /`: says the receiver is up, without a token.
//!
//! Every other request needs `Authorization: Bearer <token>`.

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod zip;

/// 100 MB: what Cloudflare Workers accepts as a request body on the Free and Pro plans.
pub const DEFAULT_MAX_UPLOAD: usize = 100 * 1024 * 1024;

pub struct Config {
    /// The bearer token every request but `GET /` needs. Empty refuses everything.
    pub token: String,
    /// The largest upload accepted, in bytes; unpacked, a bundle may be twice this.
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
}

impl Response {
    fn json(status: u16, value: &impl Serialize) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(value).unwrap_or_default(),
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

pub async fn handle(config: &Config, request: Request, store: &impl Store) -> Response {
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
            },
            _ => Response::error(405, "not allowed here"),
        };
    }
    if parts[0] != "bundles" {
        return Response::error(404, "not found");
    }
    if let Err(response) = authorize(config, request.authorization.as_deref()) {
        return response;
    }
    let result = match (method, &parts[1..]) {
        ("POST", []) => upload(config, &request.body, request.now_ms, store).await,
        ("GET", []) => list(query, store).await,
        ("GET", [id]) => entry(id, store).await,
        ("GET", [id, file @ ..]) => file_of(id, &file.join("/"), store).await,
        _ => Ok(Response::error(405, "not allowed here")),
    };
    result.unwrap_or_else(|error| Response::error(500, &format!("storage: {error}")))
}

fn authorize(config: &Config, header: Option<&str>) -> Result<(), Response> {
    if config.token.is_empty() {
        return Err(Response::error(500, "the receiver has no token set"));
    }
    let given = header.and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    // Compares every byte, so how long it takes doesn't tell how much of a guess was right.
    let (a, b) = (given.as_bytes(), config.token.as_bytes());
    let differ = a.len() != b.len() || a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) != 0;
    if differ {
        Err(Response::error(401, "wrong or missing token"))
    } else {
        Ok(())
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
    body: &[u8],
    now_ms: u64,
    store: &impl Store,
) -> Result<Response, String> {
    if body.len() > config.max_upload {
        return Ok(Response::error(
            413,
            "the bundle is larger than this receiver accepts",
        ));
    }
    let files = match zip::unzip(body, config.max_upload as u64 * 2) {
        Ok(files) => files,
        Err(zip::ZipError::TooBig) => {
            return Ok(Response::error(
                413,
                "the bundle is larger than this receiver accepts",
            ));
        }
        Err(zip::ZipError::Bad(why)) => return Ok(Response::error(400, &why)),
    };

    // One `<id>.scrawl/` folder, every path inside it. What Finder and the Mac app add on their
    // own (`__MACOSX/`, `.DS_Store`, `.deliver.json`) is left out.
    let mut id: Option<&str> = None;
    let mut inside = Vec::with_capacity(files.len());
    for (name, bytes) in &files {
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
        if id.is_some_and(|i| i != this) {
            return Ok(Response::error(400, "the zip holds more than one bundle"));
        }
        id = Some(this);
        if !safe_path(rest) {
            return Ok(Response::error(
                400,
                &format!("a file name isn't allowed: {name}"),
            ));
        }
        inside.push((rest, bytes));
    }
    let Some(id) = id else {
        return Ok(Response::error(400, "the zip is empty"));
    };
    let Some((_, scrawl_json)) = inside.iter().find(|(path, _)| *path == "scrawl.json") else {
        return Ok(Response::error(400, "the bundle has no scrawl.json"));
    };
    let meta: Meta = match serde_json::from_slice(scrawl_json) {
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
        return Ok(saved_reply(id));
    }
    for (path, bytes) in inside.iter().filter(|(path, _)| *path != "scrawl.json") {
        store
            .put(
                &format!("{folder}/{path}"),
                bytes.to_vec(),
                content_type(path),
            )
            .await?;
    }
    store
        .put(
            &format!("{folder}/scrawl.json"),
            scrawl_json.to_vec(),
            "application/json",
        )
        .await?;
    let entry = Entry {
        id: id.to_owned(),
        created_at_ms: meta.created_at_ms,
        received_at_ms: now_ms,
        duration_ms: meta.duration_ms,
        app: meta.target.and_then(|t| t.app).filter(|a| !a.is_empty()),
        device: meta.device.and_then(|d| d.name).filter(|d| !d.is_empty()),
        files: inside.iter().map(|(path, _)| path.to_string()).collect(),
        bytes: inside.iter().map(|(_, bytes)| bytes.len() as u64).sum(),
    };
    store
        .put(
            &index_key(&entry),
            serde_json::to_vec(&entry).unwrap_or_default(),
            "application/json",
        )
        .await?;
    store
        .put(
            &format!("{folder}.json"),
            serde_json::to_vec(&entry).unwrap_or_default(),
            "application/json",
        )
        .await?;
    Ok(saved_reply(id))
}

async fn list(query: &str, store: &impl Store) -> Result<Response, String> {
    let limit = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("limit="))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 100);
    let mut bundles = Vec::new();
    for key in store.list("index/", limit).await? {
        if let Some(bytes) = store.get(&key).await?
            && let Ok(entry) = serde_json::from_slice::<Entry>(&bytes)
        {
            bundles.push(entry);
        }
    }
    Ok(Response::json(
        200,
        &serde_json::json!({ "bundles": bundles }),
    ))
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
