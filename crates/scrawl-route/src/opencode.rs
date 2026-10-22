//! OpenCode's session API, read through an [`ApiGet`]: one session by ID, a newest-first search
//! of open root sessions, and the pick of the session a capture goes to. The caller supplies the
//! transport (the Scrawl app runs `opencode2 api get`, locally or over SSH), so this module makes
//! no requests of its own.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpenCodeSession {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "parentID")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub time: OpenCodeSessionTime,
    /// V2 puts the session's working directory here; V1 used a top-level `directory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<OpenCodeSessionLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

impl OpenCodeSession {
    pub fn directory(&self) -> Option<&str> {
        self.location
            .as_ref()
            .and_then(|l| l.directory.as_deref())
            .or(self.directory.as_deref())
            .filter(|d| !d.is_empty())
    }

    pub fn is_open_root(&self) -> bool {
        self.parent_id.is_none() && self.time.archived.is_none()
    }

    /// The title, or the ID when it has none.
    pub fn label(&self) -> &str {
        if self.title.is_empty() {
            &self.id
        } else {
            &self.title
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OpenCodeSessionLocation {
    #[serde(default)]
    pub directory: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OpenCodeSessionTime {
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub archived: Option<u64>,
}

/// Percent-encodes everything but unreserved characters, for a URL query or path segment.
pub fn query_escape(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

/// One `GET` against OpenCode's API, returning the body. Swapped for a fake in tests.
pub type ApiGet<'a> = dyn FnMut(&str) -> Result<Vec<u8>> + 'a;

/// How many pages of root sessions (100 each, newest first) a search reads before giving up.
/// Each page is one request (in the Scrawl app, an `opencode2 api` process).
pub const ROOT_SEARCH_PAGES: usize = 5;
const ROOT_PAGE: usize = 100;

/// One session by ID (`GET /api/session/{id}`).
pub fn get_session(get: &mut ApiGet, id: &str) -> Result<OpenCodeSession> {
    let response: serde_json::Value =
        serde_json::from_slice(&get(&format!("/api/session/{}", query_escape(id)))?)
            .context("Invalid OpenCode session response")?;
    let session = response.get("data").unwrap_or(&response).clone();
    Ok(serde_json::from_value(session)?)
}

/// What a root-session search does with each open root session it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    Skip,
    Take,
    Stop,
}

/// Reads open root sessions newest first (`query` adds filters, e.g. `&directory=…`) and returns
/// the first that `scan` takes, reading at most `max_pages` pages.
pub fn find_root_session(
    get: &mut ApiGet,
    query: &str,
    max_pages: usize,
    mut scan: impl FnMut(&OpenCodeSession) -> Scan,
) -> Result<Option<OpenCodeSession>> {
    let mut path = format!("/api/session?parentID=null&limit={ROOT_PAGE}{query}");
    let mut cursors = std::collections::HashSet::new();
    for _ in 0..max_pages {
        let response: serde_json::Value =
            serde_json::from_slice(&get(&path)?).context("Invalid OpenCode session response")?;
        let data = response
            .as_array()
            .or_else(|| response.get("data").and_then(|data| data.as_array()))
            .context("Missing OpenCode session list")?;
        for session in data {
            let session = serde_json::from_value::<OpenCodeSession>(session.clone())?;
            // The API has no archived filter.
            if !session.is_open_root() {
                continue;
            }
            match scan(&session) {
                Scan::Skip => {}
                Scan::Take => return Ok(Some(session)),
                Scan::Stop => return Ok(None),
            }
        }
        let Some(next) = response.pointer("/cursor/next").and_then(|v| v.as_str()) else {
            break;
        };
        if data.is_empty() || !cursors.insert(next.to_owned()) {
            break;
        }
        // The cursor carries the filters.
        path = format!(
            "/api/session?cursor={}&limit={ROOT_PAGE}",
            query_escape(next)
        );
    }
    Ok(None)
}

fn normalize_slug(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Picks the session a capture goes to, and its title: an explicit ID, else the root session
/// whose title matches the app ("UpDown" → "UP DOWN APP"), else `env_session` (the Scrawl app
/// passes `OPENCODE_SESSION_ID`), else the most recently updated root session. An explicit ID
/// costs one request; an app title match reads the newest [`ROOT_SEARCH_PAGES`] pages of root
/// sessions at most.
pub fn resolve_opencode_session_with(
    get: &mut ApiGet,
    session_id: Option<&str>,
    app_name: Option<&str>,
    env_session: Option<&str>,
) -> Result<(String, String)> {
    if let Some(id) = session_id.filter(|s| !s.trim().is_empty()) {
        let title = get_session(get, id)
            .ok()
            .map(|s| s.title)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| id.to_owned());
        return Ok((id.to_owned(), title));
    }

    // Match against the last component if a bundle identifier was passed (com.kitlangton.UpDown -> updown).
    let short = app_name
        .and_then(|a| a.rsplit('.').next())
        .map(normalize_slug)
        .filter(|s| !s.is_empty())
        .or_else(|| app_name.map(normalize_slug).filter(|s| !s.is_empty()));
    let mut newest = None;
    let matched = find_root_session(get, "", ROOT_SEARCH_PAGES, |s| {
        if newest.is_none() {
            newest = Some(s.clone());
        }
        let Some(short) = short.as_deref() else {
            return Scan::Stop;
        };
        let title_slug = normalize_slug(&s.title);
        if !title_slug.is_empty() && (title_slug.contains(short) || short.contains(&title_slug)) {
            Scan::Take
        } else {
            Scan::Skip
        }
    })?;
    let matched = matched
        .or_else(|| {
            let id = env_session.filter(|id| !id.trim().is_empty())?;
            get_session(get, id)
                .ok()
                .filter(OpenCodeSession::is_open_root)
        })
        .or(newest)
        .context("No root OpenCode session found; pass a session ID")?;
    let title = matched.label().to_owned();
    Ok((matched.id, title))
}

/// A fake OpenCode API for tests (feature `fake-api`).
#[cfg(any(test, feature = "fake-api"))]
pub mod fake_api {
    use anyhow::{Context, Result};

    use super::*;

    pub fn session(id: &str, title: &str, dir: &str, updated: u64) -> OpenCodeSession {
        OpenCodeSession {
            id: id.into(),
            title: title.into(),
            parent_id: None,
            time: OpenCodeSessionTime {
                updated,
                archived: None,
            },
            location: Some(OpenCodeSessionLocation {
                directory: Some(dir.into()),
            }),
            directory: None,
        }
    }

    fn decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    /// Answers `GET /api/session/{id}` and `GET /api/session?…` like OpenCode 2: newest first,
    /// `parentID=null`, exact `directory`, `limit` (default 50), and a cursor carrying the filters.
    pub fn serve(sessions: &[OpenCodeSession], path: &str) -> Result<Vec<u8>> {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        if let Some(id) = route.strip_prefix("/api/session/") {
            let session = sessions
                .iter()
                .find(|s| s.id == decode(id))
                .context("404 Not Found")?;
            return Ok(serde_json::to_vec(&serde_json::json!({ "data": session }))?);
        }
        assert_eq!(route, "/api/session");
        let mut params: std::collections::HashMap<String, String> = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.to_owned(), decode(v)))
            .collect();
        let mut offset = 0;
        if let Some(cursor) = params.remove("cursor") {
            let cursor: serde_json::Value = serde_json::from_str(&cursor)?;
            offset = cursor["offset"].as_u64().unwrap() as usize;
            for key in ["parentID", "directory"] {
                if let Some(v) = cursor[key].as_str() {
                    params.insert(key.into(), v.into());
                }
            }
        }
        let limit: usize = params.get("limit").map_or(50, |l| l.parse().unwrap());
        let mut matching: Vec<_> = sessions
            .iter()
            .filter(|s| params.get("parentID").is_none_or(|_| s.parent_id.is_none()))
            .filter(|s| {
                params
                    .get("directory")
                    .is_none_or(|d| s.directory() == Some(d.as_str()))
            })
            .collect();
        matching.sort_by(|a, b| b.time.updated.cmp(&a.time.updated));
        let page: Vec<_> = matching.iter().skip(offset).take(limit).collect();
        let next = (offset + limit < matching.len()).then(|| {
            serde_json::json!({
                "offset": offset + limit,
                "parentID": params.get("parentID"),
                "directory": params.get("directory"),
            })
            .to_string()
        });
        Ok(serde_json::to_vec(
            &serde_json::json!({ "data": page, "cursor": { "next": next } }),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::fake_api::{serve, session};
    use super::*;

    #[test]
    fn sessions_read_their_directory_from_v2_location_or_v1_field() {
        let v2: OpenCodeSession = serde_json::from_str(
            r#"{"id":"ses_a","title":"A","time":{"updated":1},"location":{"directory":"/p/a"}}"#,
        )
        .unwrap();
        assert_eq!(v2.directory(), Some("/p/a"));
        assert!(v2.is_open_root());
        let v1: OpenCodeSession =
            serde_json::from_str(r#"{"id":"ses_b","parentID":"ses_a","directory":"/p/b"}"#)
                .unwrap();
        assert_eq!(v1.directory(), Some("/p/b"));
        assert!(!v1.is_open_root());
        assert_eq!(v1.label(), "ses_b");
    }

    fn many(count: usize) -> Vec<OpenCodeSession> {
        (0..count)
            .map(|i| {
                session(
                    &format!("ses_{i:05}"),
                    &format!("Chat {i}"),
                    "/p/x",
                    100_000 - i as u64,
                )
            })
            .collect()
    }

    /// Runs `f` against `sessions` and returns its result with the paths it requested.
    fn counting<T>(
        sessions: &[OpenCodeSession],
        f: impl FnOnce(&mut ApiGet) -> T,
    ) -> (T, Vec<String>) {
        let mut paths = Vec::new();
        let result = f(&mut |path: &str| {
            paths.push(path.to_owned());
            serve(sessions, path)
        });
        (result, paths)
    }

    #[test]
    fn an_explicit_id_is_one_request() {
        let mut sessions = many(5_000);
        sessions[4_000].title = "THE ONE".into();
        let (result, paths) = counting(&sessions, |get| {
            resolve_opencode_session_with(get, Some("ses_04000"), None, None).unwrap()
        });
        assert_eq!(result, ("ses_04000".into(), "THE ONE".into()));
        assert_eq!(paths, ["/api/session/ses_04000"]);
        // Unknown to the server: still sent, titled by its ID.
        let (result, _) = counting(&sessions, |get| {
            resolve_opencode_session_with(get, Some("ses_gone"), None, None).unwrap()
        });
        assert_eq!(result, ("ses_gone".into(), "ses_gone".into()));
    }

    #[test]
    fn the_newest_open_root_is_one_page() {
        let mut sessions = many(5_000);
        sessions[0].time.archived = Some(1);
        sessions[1].parent_id = Some("ses_00002".into());
        let (result, paths) = counting(&sessions, |get| {
            resolve_opencode_session_with(get, None, None, None).unwrap()
        });
        assert_eq!(result.0, "ses_00002");
        assert_eq!(paths, ["/api/session?parentID=null&limit=100"]);
    }

    #[test]
    fn an_app_match_stops_at_the_first_hit_and_the_search_is_capped() {
        let mut sessions = many(5_000);
        sessions[150].title = "UP DOWN APP".into();
        sessions[300].title = "updown older".into();
        let (result, paths) = counting(&sessions, |get| {
            resolve_opencode_session_with(get, None, Some("com.kitlangton.UpDown"), None).unwrap()
        });
        assert_eq!(result, ("ses_00150".into(), "UP DOWN APP".into()));
        assert_eq!(paths.len(), 2);

        // No match: OPENCODE_SESSION_ID if it's an open root, else the newest; never every page.
        let (result, paths) = counting(&sessions, |get| {
            resolve_opencode_session_with(get, None, Some("Nothing"), Some("ses_04321")).unwrap()
        });
        assert_eq!(result.0, "ses_04321");
        assert_eq!(paths.len(), ROOT_SEARCH_PAGES + 1);
        let (result, _) = counting(&sessions, |get| {
            resolve_opencode_session_with(get, None, Some("Nothing"), None).unwrap()
        });
        assert_eq!(result.0, "ses_00000");
    }
}
