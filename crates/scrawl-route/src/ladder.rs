//! The routing ladder: where a capture of a browser tab goes when smart routing is off.
//!
//! 1. The page registered itself through `/sdk.js` ([`crate::web`]): its session, or the session
//!    for the directory it named.
//! 2. The tab is on a loopback host: the process listening on that port, its working directory,
//!    that directory's git root, and the most recently active root session in it.
//! 3. Otherwise nothing: the capture stays on the clipboard.
//!
//! The steps that reach outside (the page registrations, the port's listener, OpenCode's API)
//! come in through [`Hooks`]; the Scrawl app passes `lsof` and `opencode2 api`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use scrawl_bundle::Destination;

use crate::opencode::{
    ApiGet, OpenCodeSession, ROOT_SEARCH_PAGES, Scan, find_root_session, get_session, query_escape,
};
use crate::smart;
use crate::url::parse_url;
use crate::web::Registration;

// MARK: The browser tab

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Browser {
    /// Chrome, Brave, Arc, Edge, Vivaldi, Chromium: `URL of active tab of front window`.
    Chromium,
    /// Safari: `URL of current tab of front window`.
    Safari,
}

pub fn browser_kind(bundle_id: &str) -> Option<Browser> {
    const CHROMIUM: [&str; 6] = [
        "com.google.chrome",
        "com.brave.browser",
        "company.thebrowser.browser",
        "com.microsoft.edgemac",
        "com.vivaldi.vivaldi",
        "org.chromium.chromium",
    ];
    let id = bundle_id.to_ascii_lowercase();
    if id == "com.apple.safari" || id == "com.apple.safaritechnologypreview" {
        return Some(Browser::Safari);
    }
    // Beta, dev, nightly, and canary channels add a suffix.
    CHROMIUM
        .iter()
        .any(|base| id == *base || id.strip_prefix(base).is_some_and(|s| s.starts_with('.')))
        .then_some(Browser::Chromium)
}

// MARK: Port → project

/// PIDs in `lsof -F p` output (`p1234` lines), in order, without repeats.
pub fn parse_lsof_pids(output: &str) -> Vec<i32> {
    let mut pids = Vec::new();
    for pid in output
        .lines()
        .filter_map(|line| line.strip_prefix('p')?.trim().parse::<i32>().ok())
    {
        if pid > 0 && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

/// The directory in `lsof -a -p PID -d cwd -Fn` output (the `n` line).
pub fn parse_lsof_cwd(output: &str) -> Option<PathBuf> {
    output
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(str::trim)
        .filter(|path| path.starts_with('/'))
        .map(PathBuf::from)
}

/// The project a directory belongs to: its git root (a `.git` directory or worktree file), or
/// the directory itself with `false` when it isn't in a repository.
pub fn project_root(dir: &Path) -> (PathBuf, bool) {
    let mut current = Some(dir);
    while let Some(path) = current {
        if path.join(".git").exists() {
            return (path.to_path_buf(), true);
        }
        current = path.parent();
    }
    (dir.to_path_buf(), false)
}

/// The most recently active open root session for `root`. Inside a repository a session in any
/// folder of it counts; outside one, only a session in that very folder, so a server started
/// from the home folder doesn't claim every session.
///
/// The server filters by exact directory, so that takes one request. A session in a subfolder
/// only wins if it's newer, so the newest-first scan for one stops at the exact match's age (or
/// after [`ROOT_SEARCH_PAGES`]).
pub fn session_for_directory(
    get: &mut ApiGet,
    root: &Path,
    whole_tree: bool,
) -> Result<Option<OpenCodeSession>> {
    let home = dirs::home_dir();
    if root == Path::new("/") || Some(root) == home.as_deref() {
        return Ok(None);
    }
    let directory = format!("&directory={}", query_escape(&root.to_string_lossy()));
    let exact = find_root_session(get, &directory, ROOT_SEARCH_PAGES, |_| Scan::Take)?;
    if !whole_tree {
        return Ok(exact);
    }
    let floor = exact.as_ref().map(|s| s.time.updated);
    let nested = find_root_session(get, "", ROOT_SEARCH_PAGES, |s| {
        if floor.is_some_and(|floor| s.time.updated <= floor) {
            Scan::Stop
        } else if s
            .directory()
            .is_some_and(|dir| Path::new(dir).starts_with(root))
        {
            Scan::Take
        } else {
            Scan::Skip
        }
    })?;
    Ok(nested.or(exact))
}

// MARK: Deciding

/// The outside steps of a decision, swappable in tests.
#[derive(Clone, Copy)]
pub struct Hooks {
    pub sdk: fn(&str) -> Option<Registration>,
    pub listener_dir: fn(u16) -> Option<PathBuf>,
    /// One `GET` against OpenCode's API.
    pub api: fn(&str) -> Result<Vec<u8>>,
}

/// Where a capture of `url` goes, or why it goes nowhere.
pub fn route(url: &str, hooks: &Hooks) -> std::result::Result<Destination, String> {
    let parsed = parse_url(url).ok_or_else(|| format!("{url} isn't an http(s) URL"))?;
    let destination = |session: &OpenCodeSession, via: &str, dir: Option<&Path>| Destination {
        session: session.id.clone(),
        title: session.label().to_owned(),
        via: via.to_owned(),
        url: Some(url.to_owned()),
        directory: dir.map(|d| d.display().to_string()),
    };
    let mut api = hooks.api;
    let by_directory = |dir: &Path, via: &str| {
        let (root, git) = project_root(dir);
        let mut api = hooks.api;
        session_for_directory(&mut api, &root, git)
            .map_err(|err| format!("couldn't list sessions: {err:#}"))?
            .map(|s| destination(&s, via, Some(&root)))
            .ok_or_else(|| format!("no open OpenCode session in {}", root.display()))
    };

    if let Some(reg) = (hooks.sdk)(url) {
        if let Some(id) = reg.session.as_deref() {
            let title =
                get_session(&mut api, id).map_or_else(|_| id.to_owned(), |s| s.label().to_owned());
            return Ok(Destination {
                session: id.to_owned(),
                title,
                via: "sdk".into(),
                url: Some(url.to_owned()),
                directory: reg.directory,
            });
        }
        if let Some(dir) = reg.directory.as_deref() {
            return by_directory(Path::new(dir), "sdk");
        }
    }
    if !parsed.is_loopback() {
        return Err(format!("{} isn't a local dev server", parsed.origin()));
    }
    let dir = (hooks.listener_dir)(parsed.port)
        .ok_or_else(|| format!("no process found listening on port {}", parsed.port))?;
    by_directory(&dir, "localhost")
}

/// The evidence and decision behind a capture's routing, saved beside it so
/// `scrawl route --explain` replays the same decision.
pub const RECORD: &str = "routing.json";

pub fn write_record(
    dir: &Path,
    evidence: &smart::Evidence,
    judgement: &smart::Judgement,
) -> Result<()> {
    let body = serde_json::json!({ "evidence": evidence, "decision": judgement.decision });
    std::fs::write(dir.join(RECORD), serde_json::to_vec_pretty(&body)?)?;
    Ok(())
}

pub fn read_record(dir: &Path) -> Option<smart::Evidence> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(RECORD)).ok()?).ok()?;
    serde_json::from_value(value.get("evidence")?.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opencode::{OpenCodeSessionLocation, OpenCodeSessionTime, fake_api};

    fn session(id: &str, dir: &str, updated: u64) -> OpenCodeSession {
        OpenCodeSession {
            id: id.into(),
            title: id.to_uppercase(),
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

    #[test]
    fn browsers_are_recognized_by_bundle_id() {
        assert_eq!(browser_kind("com.brave.Browser"), Some(Browser::Chromium));
        assert_eq!(
            browser_kind("com.brave.Browser.beta"),
            Some(Browser::Chromium)
        );
        assert_eq!(
            browser_kind("com.google.Chrome.canary"),
            Some(Browser::Chromium)
        );
        assert_eq!(
            browser_kind("company.thebrowser.Browser"),
            Some(Browser::Chromium)
        );
        assert_eq!(browser_kind("com.apple.Safari"), Some(Browser::Safari));
        assert_eq!(browser_kind("com.mitchellh.ghostty"), None);
        assert_eq!(browser_kind("com.google.Chromecast"), None);
    }

    #[test]
    fn lsof_output_parses() {
        let listen = "p48211\ncnode\np48211\np50002\n";
        assert_eq!(parse_lsof_pids(listen), vec![48211, 50002]);
        assert!(parse_lsof_pids("").is_empty());
        let cwd = "p48211\nfcwd\nn/Users/kit/code/open-source/scrawl/site\n";
        assert_eq!(
            parse_lsof_cwd(cwd),
            Some(PathBuf::from("/Users/kit/code/open-source/scrawl/site"))
        );
        assert_eq!(parse_lsof_cwd("p1\nfcwd\n"), None);
    }

    #[test]
    fn project_root_walks_up_to_git() {
        let base = std::env::temp_dir().join(format!("scrawl-root-{}", std::process::id()));
        let site = base.join("repo/site/src");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(base.join("repo/.git")).unwrap();
        assert_eq!(project_root(&site), (base.join("repo"), true));
        let loose = base.join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        // The temp folder isn't in a repository.
        assert_eq!(project_root(&loose), (loose.clone(), false));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sessions_are_chosen_by_directory() {
        let mut child = session("ses_child", "/p/app", 900);
        child.parent_id = Some("ses_app".into());
        let mut archived = session("ses_old", "/p/app", 800);
        archived.time.archived = Some(1);
        let mut sessions = vec![
            session("ses_app", "/p/app", 100),
            session("ses_site", "/p/app/site", 200),
            session("ses_other", "/p/application", 300),
            child,
            archived,
        ];
        // Thousands of newer sessions elsewhere.
        sessions.extend((0..3_000).map(|i| session(&format!("ses_x{i}"), "/q", 1_000 + i)));
        let pick = |sessions: &[OpenCodeSession], root: &str, tree| {
            let mut requests = 0;
            let found = session_for_directory(
                &mut |path: &str| {
                    requests += 1;
                    fake_api::serve(sessions, path)
                },
                Path::new(root),
                tree,
            )
            .unwrap()
            .map(|s| s.id);
            (found, requests)
        };
        // Exact folder: one filtered request.
        assert_eq!(
            pick(&sessions, "/p/app", false),
            (Some("ses_app".into()), 1)
        );
        // A subfolder needs a newest-first scan, which is capped: past it the exact match stands.
        let (found, requests) = pick(&sessions, "/p/app", true);
        assert_eq!(found.as_deref(), Some("ses_app"));
        assert_eq!(requests, 1 + ROOT_SEARCH_PAGES);
        assert_eq!(pick(&sessions, "/p/none", false), (None, 1));
        assert_eq!(pick(&sessions, "/", true), (None, 0));

        // Recent work in a subfolder beats an older exact match; a sibling with a common prefix
        // doesn't count.
        sessions.truncate(5);
        let (found, requests) = pick(&sessions, "/p/app", true);
        assert_eq!(found.as_deref(), Some("ses_site"));
        assert_eq!(requests, 2);
        // An exact match newer than everything else stops the scan at once.
        sessions[0].time.updated = 1_000;
        assert_eq!(pick(&sessions, "/p/app", true), (Some("ses_app".into()), 2));
    }

    fn no_sdk(_: &str) -> Option<Registration> {
        None
    }
    fn sdk_session(url: &str) -> Option<Registration> {
        url.starts_with("http://localhost:5173")
            .then(|| Registration {
                origin: "http://localhost:5173".into(),
                url: url.into(),
                session: Some("ses_sdk".into()),
                ..Registration::default()
            })
    }
    fn sdk_directory(_: &str) -> Option<Registration> {
        Some(Registration {
            origin: "https://studio.ts.net:5173".into(),
            directory: Some("/p/app".into()),
            ..Registration::default()
        })
    }
    fn listener(port: u16) -> Option<PathBuf> {
        (port == 4321).then(|| PathBuf::from("/p/app"))
    }
    fn api(path: &str) -> Result<Vec<u8>> {
        fake_api::serve(
            &[
                session("ses_sdk", "/elsewhere", 5),
                session("ses_app", "/p/app", 10),
            ],
            path,
        )
    }

    #[test]
    fn route_prefers_sdk_then_localhost_port() {
        let hooks = Hooks {
            sdk: sdk_session,
            listener_dir: listener,
            api,
        };
        let sdk = route("http://localhost:5173/page", &hooks).unwrap();
        assert_eq!((sdk.session.as_str(), sdk.via.as_str()), ("ses_sdk", "sdk"));
        assert_eq!(sdk.title, "SES_SDK");

        let hooks = Hooks {
            sdk: no_sdk,
            ..hooks
        };
        // `/p/app` isn't a repository here, so only a session in exactly that folder matches.
        let local = route("http://127.0.0.1:4321/", &hooks).unwrap();
        assert_eq!(
            (
                local.session.as_str(),
                local.via.as_str(),
                local.directory.as_deref()
            ),
            ("ses_app", "localhost", Some("/p/app"))
        );
        assert!(route("http://localhost:9999/", &hooks).is_err());
        assert!(route("https://example.com/", &hooks).is_err());

        let hooks = Hooks {
            sdk: sdk_directory,
            ..hooks
        };
        let by_dir = route("https://studio.ts.net:5173/", &hooks).unwrap();
        assert_eq!(
            (by_dir.session.as_str(), by_dir.via.as_str()),
            ("ses_app", "sdk")
        );
    }
}
