use super::*;
use crate::zip::tests::zip;

/// Runs a future that never waits on anything outside it: every store here answers at once.
fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("the store made the receiver wait"),
    }
}

const TOKEN: &str = "s3cret-token";

fn config() -> Config {
    Config {
        token: TOKEN.into(),
        max_upload: 1 << 20,
    }
}

fn request(method: &str, path: &str, body: Vec<u8>) -> Request {
    Request {
        method: method.into(),
        path: path.into(),
        authorization: Some(format!("Bearer {TOKEN}")),
        body,
        now_ms: 1_800_000_000_000,
    }
}

fn scrawl_json(id: &str, created_at_ms: u64) -> Vec<u8> {
    format!(
        r#"{{"version":1,"id":"{id}","createdAtMs":{created_at_ms},"durationMs":18000,
        "target":{{"mode":"device","app":"ScrawlDemo","displayId":0,
        "frame":{{"x":0,"y":0,"width":800,"height":600,"scale":2}}}},
        "device":{{"name":"Mac","capture":"scrawlkit","width":1600,"height":1200}},
        "transcript":{{"source":"none","text":"","segments":[]}},
        "strokes":[],"marks":[],"events":[],"keyframes":[],"contactSheet":"contact-sheet.png"}}"#
    )
    .into_bytes()
}

/// A bundle zip as `scrawl-web`'s `zip()` writes it: `<id>.scrawl/` and its files.
fn bundle(id: &str, created_at_ms: u64) -> Vec<u8> {
    let json = scrawl_json(id, created_at_ms);
    let root = format!("{id}.scrawl");
    zip(
        &[
            (&format!("{root}/scrawl.json"), &json),
            (&format!("{root}/summary.md"), b"[scrawl 0:18]"),
            (&format!("{root}/contact-sheet.png"), b"\x89PNG sheet"),
            (&format!("{root}/frames/frame-01.png"), b"\x89PNG one"),
        ],
        false,
    )
}

fn reply(response: &Response) -> serde_json::Value {
    serde_json::from_slice(&response.body).unwrap()
}

#[test]
fn an_upload_is_kept_listed_and_served() {
    let store = MemoryStore::default();
    let sent = run(handle(
        &config(),
        request("POST", "/bundles", bundle("web-1", 1_000)),
        &store,
    ));
    assert_eq!(sent.status, 200);
    // ScrawlKit's `IngestReply` reads this as `.saved`.
    assert_eq!(
        reply(&sent),
        serde_json::json!({"ok": true, "delivered": false, "state": "saved", "id": "web-1"})
    );

    let listed = run(handle(
        &config(),
        request("GET", "/bundles", vec![]),
        &store,
    ));
    let bundles = &reply(&listed)["bundles"];
    assert_eq!(bundles.as_array().unwrap().len(), 1);
    assert_eq!(bundles[0]["id"], "web-1");
    assert_eq!(bundles[0]["app"], "ScrawlDemo");
    assert_eq!(bundles[0]["device"], "Mac");
    assert_eq!(bundles[0]["receivedAtMs"], 1_800_000_000_000u64);
    assert_eq!(bundles[0]["files"].as_array().unwrap().len(), 4);

    let summary = run(handle(
        &config(),
        request("GET", "/bundles/web-1/summary.md", vec![]),
        &store,
    ));
    assert_eq!(
        (summary.status, summary.body.as_slice()),
        (200, &b"[scrawl 0:18]"[..])
    );
    assert_eq!(summary.content_type, "text/markdown; charset=utf-8");
    let frame = run(handle(
        &config(),
        request("GET", "/bundles/web-1/frames/frame-01.png", vec![]),
        &store,
    ));
    assert_eq!((frame.status, frame.content_type), (200, "image/png"));
    let one = run(handle(
        &config(),
        request("GET", "/bundles/web-1", vec![]),
        &store,
    ));
    assert_eq!(reply(&one)["id"], "web-1");
}

#[test]
fn the_newest_bundle_is_listed_first_and_the_limit_holds() {
    let store = MemoryStore::default();
    for (id, at) in [("old", 1_000), ("new", 3_000), ("middle", 2_000)] {
        run(handle(
            &config(),
            request("POST", "/bundles", bundle(id, at)),
            &store,
        ));
    }
    let listed = run(handle(
        &config(),
        request("GET", "/bundles?limit=2", vec![]),
        &store,
    ));
    let ids: Vec<_> = reply(&listed)["bundles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["new", "middle"]);
}

#[test]
fn a_bundle_sent_twice_is_kept_once() {
    let store = MemoryStore::default();
    let first = run(handle(
        &config(),
        request("POST", "/bundles", bundle("web-1", 1_000)),
        &store,
    ));
    let objects = store.objects.borrow().len();
    let again = run(handle(
        &config(),
        request("POST", "/bundles", bundle("web-1", 1_000)),
        &store,
    ));
    assert_eq!((first.status, again.status), (200, 200));
    assert_eq!(reply(&first), reply(&again));
    assert_eq!(store.objects.borrow().len(), objects);
    // Cut off before its entry was written, it's written again rather than taken as here.
    store
        .objects
        .borrow_mut()
        .retain(|key, _| !key.starts_with("index/") && !key.ends_with(".scrawl.json"));
    run(handle(
        &config(),
        request("POST", "/bundles", bundle("web-1", 1_000)),
        &store,
    ));
    assert_eq!(store.objects.borrow().len(), objects);
}

#[test]
fn every_request_but_the_front_page_needs_the_token() {
    let store = MemoryStore::default();
    for token in [
        None,
        Some("Bearer wrong".to_owned()),
        Some(TOKEN.to_owned()),
    ] {
        let mut sent = request("POST", "/bundles", bundle("web-1", 1_000));
        sent.authorization = token.clone();
        assert_eq!(run(handle(&config(), sent, &store)).status, 401);
        let mut listed = request("GET", "/bundles", vec![]);
        listed.authorization = token;
        assert_eq!(run(handle(&config(), listed, &store)).status, 401);
    }
    assert!(store.objects.borrow().is_empty());
    let mut front = request("GET", "/", vec![]);
    front.authorization = None;
    assert_eq!(run(handle(&config(), front, &store)).status, 200);
    // No token set refuses everything rather than letting anyone in.
    let open = Config {
        token: String::new(),
        max_upload: 1 << 20,
    };
    let mut sent = request("POST", "/bundles", bundle("web-1", 1_000));
    sent.authorization = Some("Bearer ".into());
    assert_eq!(run(handle(&open, sent, &store)).status, 500);
}

#[test]
fn paths_outside_the_bundle_are_refused() {
    let store = MemoryStore::default();
    let json = scrawl_json("web-1", 1_000);
    for (name, why) in [
        ("web-1.scrawl/../escape.png", "a file name isn't allowed"),
        (
            "web-1.scrawl/frames/../../escape.png",
            "a file name isn't allowed",
        ),
        ("/web-1.scrawl/abs.png", "one <id>.scrawl folder"),
        ("loose.png", "one <id>.scrawl folder"),
        ("other.scrawl/x.png", "more than one bundle"),
        ("web-1.scrawl/back\\slash.png", "a file name isn't allowed"),
    ] {
        let archive = zip(&[("web-1.scrawl/scrawl.json", &json), (name, b"x")], false);
        let response = run(handle(
            &config(),
            request("POST", "/bundles", archive),
            &store,
        ));
        assert_eq!(response.status, 400, "{name}");
        assert!(
            reply(&response)["error"].as_str().unwrap().contains(why),
            "{name}: {response:?}"
        );
    }
    assert!(store.objects.borrow().is_empty());
    let traversal = run(handle(
        &config(),
        request("GET", "/bundles/web-1/..%2Fx", vec![]),
        &store,
    ));
    assert_eq!(traversal.status, 404);
}

#[test]
fn what_finder_adds_is_left_out() {
    let store = MemoryStore::default();
    let json = scrawl_json("web-1", 1_000);
    let archive = zip(
        &[
            ("web-1.scrawl/", b""),
            ("web-1.scrawl/scrawl.json", &json),
            ("web-1.scrawl/.DS_Store", b"x"),
            ("web-1.scrawl/.deliver.json", b"{}"),
            ("__MACOSX/web-1.scrawl/._scrawl.json", b"x"),
        ],
        true,
    );
    let response = run(handle(
        &config(),
        request("POST", "/bundles", archive),
        &store,
    ));
    assert_eq!(response.status, 200, "{response:?}");
    let keys: Vec<String> = store.objects.borrow().keys().cloned().collect();
    assert!(
        keys.iter()
            .all(|k| !k.contains("DS_Store") && !k.contains("deliver") && !k.contains("MACOSX"))
    );
}

#[test]
fn a_bundle_without_a_good_scrawl_json_is_refused() {
    let store = MemoryStore::default();
    let cases: [(&[u8], &str); 3] = [
        (b"{not json", "doesn't parse"),
        (br#"{"version":2}"#, "isn't version 1"),
        (b"", "doesn't parse"),
    ];
    for (json, why) in cases {
        let archive = zip(&[("web-1.scrawl/scrawl.json", json)], false);
        let response = run(handle(
            &config(),
            request("POST", "/bundles", archive),
            &store,
        ));
        assert_eq!(response.status, 400);
        assert!(reply(&response)["error"].as_str().unwrap().contains(why));
    }
    let archive = zip(&[("web-1.scrawl/summary.md", b"x")], false);
    let response = run(handle(
        &config(),
        request("POST", "/bundles", archive),
        &store,
    ));
    assert!(
        reply(&response)["error"]
            .as_str()
            .unwrap()
            .contains("no scrawl.json")
    );
    let response = run(handle(
        &config(),
        request("POST", "/bundles", b"junk".to_vec()),
        &store,
    ));
    assert_eq!(response.status, 400);
}

#[test]
fn a_bundle_over_the_limit_is_refused() {
    let store = MemoryStore::default();
    let small = Config {
        token: TOKEN.into(),
        max_upload: 2_000,
    };
    let json = scrawl_json("web-1", 1_000);
    let big = vec![1u8; 3_000];
    let stored = zip(
        &[
            ("web-1.scrawl/scrawl.json", &json),
            ("web-1.scrawl/recording.mp4", &big),
        ],
        false,
    );
    assert_eq!(
        run(handle(&small, request("POST", "/bundles", stored), &store)).status,
        413
    );
    // Compressed under the limit, unpacked over twice it.
    let huge = vec![0u8; 5_000];
    let packed = zip(
        &[
            ("web-1.scrawl/scrawl.json", &json),
            ("web-1.scrawl/recording.mp4", &huge),
        ],
        true,
    );
    assert!(packed.len() < 2_000);
    assert_eq!(
        run(handle(&small, request("POST", "/bundles", packed), &store)).status,
        413
    );
    assert!(store.objects.borrow().is_empty());
}

#[test]
fn unknown_paths_and_methods_are_refused() {
    let store = MemoryStore::default();
    assert_eq!(
        run(handle(&config(), request("GET", "/nope", vec![]), &store)).status,
        404
    );
    assert_eq!(
        run(handle(
            &config(),
            request("DELETE", "/bundles", vec![]),
            &store
        ))
        .status,
        405
    );
    assert_eq!(
        run(handle(
            &config(),
            request("GET", "/bundles/missing", vec![]),
            &store
        ))
        .status,
        404
    );
    assert_eq!(
        run(handle(
            &config(),
            request("GET", "/bundles/missing/summary.md", vec![]),
            &store
        ))
        .status,
        404
    );
}

#[test]
fn a_missing_token_or_a_too_big_upload_is_answered_before_the_body_is_read() {
    let check = |method, path, token: Option<&str>, length| {
        answer_before_body(&config(), method, path, token, length).map(|r| r.status)
    };
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(check("POST", "/bundles", None, Some(10)), Some(401));
    assert_eq!(
        check("GET", "/bundles?limit=5", Some("Bearer nope"), None),
        Some(401)
    );
    assert_eq!(
        check("POST", "/bundles", Some(&bearer), Some(2 << 20)),
        Some(413)
    );
    assert_eq!(check("POST", "/bundles", Some(&bearer), Some(10)), None);
    // No length given, the body couldn't be bounded before it's read.
    assert_eq!(check("POST", "/bundles", Some(&bearer), None), Some(411));
    assert_eq!(check("GET", "/bundles", Some(&bearer), None), None);
    assert_eq!(check("GET", "/", None, None), None);
}
