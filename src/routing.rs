//! Where a Mac capture of a browser tab goes. When the capture ends on a browser showing a dev
//! site, the bundle is sent to that project's OpenCode session instead of only the clipboard:
//!
//! 1. The page registered itself through Scrawl's `/sdk.js` (`web`): its session, or the session
//!    for the directory it named.
//! 2. The tab is on a loopback host: the process listening on that port, its working directory,
//!    that directory's git root, and the most recently active root session in it.
//! 3. Otherwise nothing: the capture stays on the clipboard.
//!
//! With `"experiments": {"smartRouting": true}`, [`smart`] decides instead, from clues
//! [`look`] gathers (`docs/routing.md`).

pub mod look;
pub mod smart;

/// The experiment that routes Mac captures with [`smart`] (`handoff::experiment`).
pub const SMART_ROUTING: &str = "smartRouting";

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::capture::scrawl_log;
use crate::model::{Destination, ScrawlBundle};

pub use scrawl_route::ladder::{
    Browser, Hooks, RECORD, browser_kind, parse_lsof_cwd, parse_lsof_pids, project_root,
    read_record, route, write_record,
};
pub use scrawl_route::url::{WebUrl, parse_url};

// MARK: The browser tab

fn safe_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
}

/// The front tab's URL in the browser `bundle_id`, through AppleScript. The first time macOS
/// asks whether Scrawl may control the browser; while that prompt is up this waits a few
/// seconds, then gives up for this capture. A denial just returns `None`.
pub fn browser_tab_url(bundle_id: &str) -> Option<String> {
    let kind = browser_kind(bundle_id)?;
    if !safe_bundle_id(bundle_id) {
        return None;
    }
    let tab = match kind {
        Browser::Chromium => "active tab",
        Browser::Safari => "current tab",
    };
    let script = format!(
        "tell application id \"{bundle_id}\"\nif (count of windows) is 0 then return \"\"\nreturn URL of {tab} of front window\nend tell"
    );
    let output = run_with_timeout(
        Command::new("/usr/bin/osascript").args(["-e", &script]),
        Duration::from_secs(6),
    )?;
    if !output.status.success() {
        scrawl_log(format!(
            "routing: couldn't read the {bundle_id} tab ({})",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!url.is_empty()).then_some(url)
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                scrawl_log("routing: gave up waiting on a subprocess");
                return None;
            }
        }
    }
}

/// The URL a capture was of: the browser in front when it finished, else the captured window's
/// browser.
pub fn capture_url(
    front_bundle_id: Option<&str>,
    target_bundle_id: Option<&str>,
) -> Option<String> {
    [front_bundle_id, target_bundle_id]
        .into_iter()
        .flatten()
        .filter(|id| browser_kind(id).is_some())
        .find_map(browser_tab_url)
}

// MARK: Port → project

fn lsof(args: &[&str]) -> Option<String> {
    let output = run_with_timeout(
        Command::new("/usr/sbin/lsof").args(args),
        Duration::from_secs(3),
    )?;
    // lsof exits 1 when some of what it was asked about wasn't found.
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    (!text.trim().is_empty()).then_some(text)
}

/// The working directory of the process listening on TCP `port` on this Mac.
pub fn listener_directory(port: u16) -> Option<PathBuf> {
    let pids = parse_lsof_pids(&lsof(&[
        "-nP",
        &format!("-iTCP:{port}"),
        "-sTCP:LISTEN",
        "-Fp",
    ])?);
    pids.into_iter().find_map(|pid| {
        parse_lsof_cwd(&lsof(&["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])?)
            .filter(|dir| dir != Path::new("/"))
    })
}

// MARK: Deciding

/// The real outside steps: `/sdk.js` registrations, `lsof`, and `opencode2 api`.
pub fn hooks() -> Hooks {
    Hooks {
        sdk: crate::web::lookup,
        listener_dir: listener_directory,
        api: crate::handoff::api_get,
    }
}

/// Decides where a capture that just finished goes; `None` (logged) keeps it on the clipboard.
pub fn route_capture(
    front_bundle_id: Option<&str>,
    target_bundle_id: Option<&str>,
) -> Option<Destination> {
    if !crate::handoff::auto_send() || std::env::var_os("SCRAWL_NO_OPENCODE").is_some() {
        return None;
    }
    let url = capture_url(front_bundle_id, target_bundle_id)?;
    match route(&url, &hooks()) {
        Ok(dest) => {
            scrawl_log(format!(
                "routing: {url} → {} ({}) via {}",
                dest.title, dest.session, dest.via
            ));
            Some(dest)
        }
        Err(reason) => {
            scrawl_log(format!("routing: {url} stays on the clipboard: {reason}"));
            None
        }
    }
}

/// Smart routing's first half for a capture that just finished: the tab, terminal and sessions,
/// or `None` when routing is off.
pub fn look_capture(
    front_bundle_id: Option<&str>,
    target_bundle_id: Option<&str>,
) -> Option<look::Live> {
    if !crate::handoff::auto_send() || std::env::var_os("SCRAWL_NO_OPENCODE").is_some() {
        return None;
    }
    Some(look::look_live(
        front_bundle_id,
        target_bundle_id,
        &look::Senses::default(),
    ))
}

/// Smart routing's second half, once the bundle is built: decides, saves the evidence beside
/// the bundle (`RECORD`), and logs the outcome.
pub fn judge_capture(
    dir: &Path,
    bundle: &ScrawlBundle,
    live: look::Live,
) -> (smart::Judgement, Option<String>) {
    let url = live.url.clone();
    if let Some(error) = &live.error {
        scrawl_log(format!("routing: {error}"));
    }
    let evidence = look::evidence_for(bundle, dir, live);
    let judgement = smart::decide(&evidence, smart::Policy::Weigh);
    if let Err(err) = write_record(dir, &evidence, &judgement) {
        scrawl_log(format!("routing: couldn't save {RECORD}: {err:#}"));
    }
    let outcome = match &judgement.decision {
        smart::Decision::Send { to } => {
            format!("send to {} ({})", to.candidate.label(), to.candidate.id)
        }
        smart::Decision::Offer { choices } => format!(
            "offer {}",
            choices
                .iter()
                .map(|c| c.candidate.label())
                .collect::<Vec<_>>()
                .join(" or ")
        ),
        smart::Decision::Keep { reason } => format!("clipboard only: {reason}"),
    };
    scrawl_log(format!(
        "routing (smart): {} clues, {} sessions → {outcome}",
        evidence.clues.len(),
        evidence.candidates.len()
    ));
    (judgement, url)
}

/// `scrawl route --explain`: what smart routing sees for a bundle and what it decides, beside
/// what the ladder (smart routing off) does. Read-only.
pub fn explain_bundle(bundle: &Path, url: Option<&str>) -> Result<String> {
    use std::fmt::Write as _;
    let dir = crate::handoff::resolve_bundle_dir(bundle)?;
    let (_, loaded) = crate::handoff::load_bundle(&dir)?;
    let (evidence, error, replayed) =
        look::evidence_for_bundle(&dir, &loaded, url, &look::Senses::default());
    let judgement = smart::decide(&evidence, smart::Policy::Weigh);
    let mut out = String::new();
    let target = &loaded.target;
    let _ = writeln!(
        out,
        "Capture   {} · {}{} · {}:{:02}",
        dir.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        target.app.as_deref().unwrap_or("?"),
        target
            .window_title
            .as_deref()
            .map(|t| format!(" “{t}”"))
            .unwrap_or_default(),
        loaded.duration_ms / 60_000,
        loaded.duration_ms / 1000 % 60
    );
    let tab = url
        .map(str::to_owned)
        .or_else(|| loaded.destination.as_ref().and_then(|d| d.url.clone()));
    if replayed {
        let _ = writeln!(out, "Evidence  saved at capture time ({RECORD})");
    } else {
        let _ = writeln!(
            out,
            "Evidence  gathered now: sessions as they are today{}",
            if tab.is_none() {
                "; no tab URL (pass --url)"
            } else {
                ""
            }
        );
    }
    if let Some(error) = error {
        let _ = writeln!(out, "Warning   {error}");
    }
    out.push_str(&smart::explain(&evidence, &judgement));
    let today = match tab.as_deref() {
        Some(tab) => match route(tab, &hooks()) {
            Ok(dest) => format!("send to {} ({}) via {}", dest.title, dest.session, dest.via),
            Err(reason) => format!("clipboard only: {reason}"),
        },
        None => "clipboard only: not a browser tab".into(),
    };
    let _ = writeln!(out, "Flag off  {today}");
    Ok(out)
}

/// Writes `dest` into the bundle's `scrawl.json` and `summary.md`.
pub fn record_destination(dir: &Path, bundle: &mut ScrawlBundle, dest: Destination) -> Result<()> {
    bundle.destination = Some(dest);
    std::fs::write(dir.join("scrawl.json"), serde_json::to_vec_pretty(bundle)?)?;
    std::fs::write(
        dir.join("summary.md"),
        crate::handoff::summary_markdown(bundle, dir),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_is_recorded_and_preferred_by_send() {
        let dir =
            std::env::temp_dir().join(format!("scrawl-route-{}.scrawl", crate::make_bundle_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut bundle: ScrawlBundle = serde_json::from_value(serde_json::json!({
            "version": 1, "id": "t", "createdAtMs": 0, "durationMs": 1_000,
            "target": {"mode": "display", "displayId": 1,
                "frame": {"x": 0.0, "y": 0.0, "width": 10.0, "height": 10.0, "scale": 1.0}},
            "transcript": {"source": "none", "text": "", "segments": []},
            "strokes": [], "marks": [], "events": [], "keyframes": [], "contactSheet": "c.png"
        }))
        .unwrap();
        assert_eq!(crate::handoff::recorded_destination(&dir), None);
        record_destination(
            &dir,
            &mut bundle,
            Destination {
                session: "ses_app".into(),
                title: "APP".into(),
                via: "localhost".into(),
                url: Some("http://localhost:4321/".into()),
                directory: None,
            },
        )
        .unwrap();
        let summary = std::fs::read_to_string(dir.join("summary.md")).unwrap();
        assert!(
            summary.contains("For session: APP (ses_app) · page http://localhost:4321/"),
            "{summary}"
        );
        let (_, loaded) = crate::handoff::load_bundle(&dir).unwrap();
        assert_eq!(loaded.destination.unwrap().via, "localhost");
        assert_eq!(
            crate::handoff::recorded_destination(&dir).as_deref(),
            Some("ses_app")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
