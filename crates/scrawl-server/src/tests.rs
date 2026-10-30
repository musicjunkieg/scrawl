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
        upload_token: String::new(),
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
        upload_token: String::new(),
        max_upload: 1 << 20,
    };
    let mut sent = request("POST", "/bundles", bundle("web-1", 1_000));
    sent.authorization = Some("Bearer ".into());
    assert_eq!(run(handle(&open, sent, &store)).status, 500);
}

#[test]
fn the_upload_token_sends_bundles_but_reads_nothing() {
    const UPLOAD: &str = "app-upload-token";
    let both = Config {
        token: TOKEN.into(),
        upload_token: UPLOAD.into(),
        max_upload: 1 << 20,
    };
    let store = MemoryStore::default();
    let with = |method: &str, path: &str, body: Vec<u8>| {
        let mut sent = request(method, path, body);
        sent.authorization = Some(format!("Bearer {UPLOAD}"));
        sent
    };
    let sent = run(handle(
        &both,
        with("POST", "/bundles", bundle("app-1", 1_000)),
        &store,
    ));
    assert_eq!(sent.status, 200);
    assert_eq!(reply(&sent)["state"], "saved");
    assert_eq!(
        run(handle(&both, with("GET", "/health", vec![]), &store)).status,
        200
    );
    // An app's token, pulled out of the app, opens none of what's been sent.
    for (method, path) in [
        ("GET", "/bundles"),
        ("GET", "/bundles/app-1"),
        ("GET", "/bundles/app-1/summary.md"),
        ("POST", "/mcp"),
    ] {
        let answer = run(handle(&both, with(method, path, b"{}".to_vec()), &store));
        assert_eq!(answer.status, 403, "{method} {path}");
    }
    let bearer = format!("Bearer {UPLOAD}");
    let early = answer_before_body(&both, "GET", "/bundles", Some(&bearer), None);
    assert_eq!(early.map(|r| r.status), Some(403));
    let early = answer_before_body(&both, "POST", "/mcp", Some(&bearer), Some(10));
    assert_eq!(early.map(|r| r.status), Some(403));
    assert!(answer_before_body(&both, "POST", "/bundles", Some(&bearer), Some(10)).is_none());
    // The token still reads everything, and X-Scrawl-Token can carry the upload token.
    assert_eq!(
        run(handle(
            &both,
            request("GET", "/bundles/app-1", vec![]),
            &store
        ))
        .status,
        200
    );
    let given = authorization(&both, Some("Bearer replaced".into()), Some(UPLOAD.into()));
    assert_eq!(given, Some(bearer.clone()));
    // Only the upload token's uploads count toward a front end's rate limit.
    assert!(counts_toward_upload_limit(
        &both,
        "POST",
        "/bundles",
        Some(&bearer)
    ));
    let owner = format!("Bearer {TOKEN}");
    assert!(!counts_toward_upload_limit(
        &both,
        "POST",
        "/bundles",
        Some(&owner)
    ));
    assert!(!counts_toward_upload_limit(
        &both,
        "GET",
        "/health",
        Some(&bearer)
    ));
    assert!(!counts_toward_upload_limit(
        &both,
        "POST",
        "/bundles",
        Some("Bearer wrong")
    ));
    assert_eq!(too_many_uploads().status, 429);
    // Without an upload token set, it's just a wrong token.
    let mut sent = with("POST", "/bundles", bundle("app-2", 2_000));
    sent.authorization = Some(format!("Bearer {UPLOAD}"));
    assert_eq!(run(handle(&config(), sent, &store)).status, 401);
}

#[test]
fn scrawlkit_checks_its_token_and_may_send_it_in_its_own_header() {
    let store = MemoryStore::default();
    let health = run(handle(&config(), request("GET", "/health", vec![]), &store));
    assert_eq!(health.status, 200);
    assert_eq!(reply(&health)["ok"], true);
    let mut wrong = request("GET", "/health", vec![]);
    wrong.authorization = Some("Bearer wrong".into());
    assert_eq!(run(handle(&config(), wrong, &store)).status, 401);

    // A background upload whose Authorization iOS dropped still has X-Scrawl-Token.
    let mut sent = request("POST", "/bundles", bundle("phone-1", 1_000));
    sent.authorization = authorization(&config(), None, Some(format!("{TOKEN}\n")));
    assert_eq!(run(handle(&config(), sent, &store)).status, 200);
    // One replaced by something else still gets in with the other, before the body too.
    let bearer = format!("Bearer {TOKEN}");
    for (header, other) in [
        (Some("Bearer replaced".to_owned()), Some(TOKEN.to_owned())),
        (Some(bearer.clone()), Some("wrong".to_owned())),
    ] {
        let given = authorization(&config(), header, other);
        assert_eq!(given.as_deref(), Some(bearer.as_str()));
        let early = answer_before_body(&config(), "POST", "/bundles", given.as_deref(), Some(10));
        assert!(early.is_none());
    }
    // Neither right: refused.
    let given = authorization(&config(), Some("Bearer a".into()), Some("b".into()));
    let early = answer_before_body(&config(), "POST", "/bundles", given.as_deref(), Some(10));
    assert_eq!(early.map(|r| r.status), Some(401));
    assert_eq!(authorization(&config(), None, None), None);
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
        upload_token: String::new(),
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

fn rpc(store: &MemoryStore, message: serde_json::Value) -> Response {
    run(handle(
        &config(),
        request("POST", "/mcp", message.to_string().into_bytes()),
        store,
    ))
}

fn result(response: &Response) -> serde_json::Value {
    assert_eq!(response.status, 200, "{response:?}");
    let body = reply(response);
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}

fn call(store: &MemoryStore, tool: &str, arguments: serde_json::Value) -> serde_json::Value {
    result(&rpc(
        store,
        serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}}),
    ))
}

#[test]
fn an_agent_starts_an_mcp_session_and_finds_three_tools() {
    let store = MemoryStore::default();
    let started = result(&rpc(
        &store,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "claude-code", "version": "2.0"}}}),
    ));
    assert_eq!(started["protocolVersion"], "2025-06-18");
    assert_eq!(started["serverInfo"]["name"], "scrawl-server");
    assert!(
        started["instructions"]
            .as_str()
            .unwrap()
            .contains("get_scrawl")
    );
    // A version it doesn't know gets the newest it does.
    let unknown = result(&rpc(
        &store,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": {"protocolVersion": "1999-01-01"}}),
    ));
    assert_eq!(unknown["protocolVersion"], "2025-11-25");

    let notified = rpc(
        &store,
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    assert_eq!((notified.status, notified.body.len()), (202, 0));

    let tools = result(&rpc(
        &store,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
    ));
    let names: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["list_scrawls", "get_scrawl", "get_scrawl_file"]);
    assert_eq!(
        result(&rpc(
            &store,
            serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "ping"})
        )),
        serde_json::json!({})
    );
}

#[test]
fn an_agent_lists_looks_at_and_opens_a_scrawl() {
    let store = MemoryStore::default();
    assert!(
        call(&store, "list_scrawls", serde_json::json!({}))["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No Scrawl recordings")
    );
    // Asked for the newest before anything came: says so, not that an id is missing.
    let empty = call(&store, "get_scrawl", serde_json::json!({}));
    assert_eq!(empty["isError"], true);
    assert_eq!(
        empty["content"][0]["text"],
        "No Scrawl recordings have been sent here yet."
    );
    for (id, at) in [("old", 1_761_777_000_000u64), ("new", 1_761_777_060_000)] {
        run(handle(
            &config(),
            request("POST", "/bundles", bundle(id, at)),
            &store,
        ));
    }

    let listed = call(&store, "list_scrawls", serde_json::json!({"limit": 1}));
    let text = listed["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        text,
        "- new · 2025-10-29 22:31 UTC · 0:18 · ScrawlDemo on Mac · 4 files"
    );
    assert_eq!(listed["structuredContent"]["scrawls"][0]["id"], "new");

    // With no id, the newest: its summary, its contact sheet, and how to open a frame.
    let looked = call(&store, "get_scrawl", serde_json::json!({}));
    let content = looked["content"].as_array().unwrap();
    assert_eq!(content[0]["text"], "[scrawl 0:18]");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    use base64::Engine;
    let sheet = base64::engine::general_purpose::STANDARD
        .decode(content[1]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(sheet, b"\x89PNG sheet");
    assert!(
        content[2]["text"]
            .as_str()
            .unwrap()
            .contains("frames/frame-01.png")
    );
    let old = call(&store, "get_scrawl", serde_json::json!({"id": "old"}));
    assert_eq!(old["content"][0]["text"], "[scrawl 0:18]");

    let frame = call(
        &store,
        "get_scrawl_file",
        serde_json::json!({"id": "new", "path": "frames/frame-01.png"}),
    );
    assert_eq!(frame["content"][0]["type"], "image");
    let json = call(
        &store,
        "get_scrawl_file",
        serde_json::json!({"id": "new", "path": "scrawl.json"}),
    );
    assert!(
        json["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"version\":1")
    );

    for (tool, arguments) in [
        ("get_scrawl", serde_json::json!({"id": "missing"})),
        ("get_scrawl", serde_json::json!({"id": "../escape"})),
        (
            "get_scrawl_file",
            serde_json::json!({"id": "new", "path": "../../index"}),
        ),
        (
            "get_scrawl_file",
            serde_json::json!({"id": "new", "path": "frames/frame-09.png"}),
        ),
        ("get_scrawl_file", serde_json::json!({"id": "new"})),
    ] {
        let answer = call(&store, tool, arguments.clone());
        assert_eq!(answer["isError"], true, "{tool} {arguments}");
    }
}

#[test]
fn mcp_needs_the_token_and_well_formed_messages() {
    let store = MemoryStore::default();
    let mut anonymous = request("POST", "/mcp", b"{}".to_vec());
    anonymous.authorization = None;
    assert_eq!(run(handle(&config(), anonymous, &store)).status, 401);
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(
        answer_before_body(&config(), "POST", "/mcp", Some(&bearer), Some(2 << 20))
            .map(|r| r.status),
        Some(413)
    );
    assert_eq!(
        answer_before_body(&config(), "POST", "/mcp", None, Some(10)).map(|r| r.status),
        Some(401)
    );
    // No event stream to open.
    assert_eq!(
        run(handle(&config(), request("GET", "/mcp", vec![]), &store)).status,
        405
    );

    let garbled = run(handle(
        &config(),
        request("POST", "/mcp", b"{not json".to_vec()),
        &store,
    ));
    assert_eq!(reply(&garbled)["error"]["code"], -32700);
    let unknown = rpc(
        &store,
        serde_json::json!({"jsonrpc": "2.0", "id": 9, "method": "resources/list"}),
    );
    assert_eq!(reply(&unknown)["error"]["code"], -32601);
    assert_eq!(reply(&unknown)["id"], 9);
    let no_tool = rpc(
        &store,
        serde_json::json!({"jsonrpc": "2.0", "id": 10, "method": "tools/call", "params": {"name": "rm_rf"}}),
    );
    assert_eq!(reply(&no_tool)["error"]["code"], -32602);
}

#[test]
fn messages_must_be_json_rpc_2_0() {
    let store = MemoryStore::default();
    for message in [
        serde_json::json!({"id": 1, "method": "tools/list"}),
        serde_json::json!({"jsonrpc": "1.0", "id": 1, "method": "tools/list"}),
    ] {
        let answer = rpc(&store, message);
        assert_eq!(reply(&answer)["error"]["code"], -32600);
        assert_eq!(reply(&answer)["id"], 1);
    }
    // An id is a string or an integer.
    for id in [
        serde_json::json!(true),
        serde_json::json!(1.5),
        serde_json::Value::Null,
        serde_json::json!([1]),
    ] {
        let answer = rpc(
            &store,
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "ping"}),
        );
        assert_eq!(reply(&answer)["error"]["code"], -32600, "{id}");
    }
    let named = rpc(
        &store,
        serde_json::json!({"jsonrpc": "2.0", "id": "a1", "method": "ping"}),
    );
    assert_eq!(reply(&named)["id"], "a1");
    // A whole number written with a decimal point is an integer too.
    let whole = run(handle(
        &config(),
        request(
            "POST",
            "/mcp",
            br#"{"jsonrpc":"2.0","id":2.0,"method":"ping"}"#.to_vec(),
        ),
        &store,
    ));
    assert!(reply(&whole).get("error").is_none(), "{whole:?}");
    assert_eq!(reply(&whole)["id"], 2.0);
}

#[test]
fn text_too_large_to_send_points_to_its_download() {
    let store = MemoryStore::default();
    let json = scrawl_json("big", 1_000);
    let huge = vec![b'a'; (1 << 20) + 1];
    let archive = zip(
        &[
            ("big.scrawl/scrawl.json", &json),
            ("big.scrawl/summary.md", &huge),
        ],
        true,
    );
    let config = Config {
        token: TOKEN.into(),
        upload_token: String::new(),
        max_upload: 4 << 20,
    };
    let sent = run(handle(
        &config,
        request("POST", "/bundles", archive),
        &store,
    ));
    assert_eq!(sent.status, 200, "{sent:?}");
    let looked = call(&store, "get_scrawl", serde_json::json!({"id": "big"}));
    let text = looked["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("too large") && text.contains("GET /bundles/big/summary.md"),
        "{text}"
    );
    let file = call(
        &store,
        "get_scrawl_file",
        serde_json::json!({"id": "big", "path": "summary.md"}),
    );
    assert_eq!(file["isError"], true);
}

/// A store whose every call fails, as R2 can.
struct BrokenStore;

impl Store for BrokenStore {
    async fn get(&self, _key: &str) -> Result<Option<Vec<u8>>, String> {
        Err("R2 is down".into())
    }
    async fn put(&self, _key: &str, _bytes: Vec<u8>, _kind: &str) -> Result<(), String> {
        Err("R2 is down".into())
    }
    async fn list(&self, _prefix: &str, _limit: usize) -> Result<Vec<String>, String> {
        Err("R2 is down".into())
    }
}

#[test]
fn a_storage_failure_is_a_json_rpc_error_for_its_request() {
    let message = serde_json::json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "list_scrawls", "arguments": {}}});
    let answer = run(handle(
        &config(),
        request("POST", "/mcp", message.to_string().into_bytes()),
        &BrokenStore,
    ));
    assert_eq!(answer.status, 200);
    assert_eq!(reply(&answer)["id"], 5);
    assert_eq!(reply(&answer)["error"]["code"], -32603);
    // The provider's words stay in the log, out of the reply.
    assert_eq!(reply(&answer)["error"]["message"], "Storage error");
    assert_eq!(answer.log.as_deref(), Some("storage: R2 is down"));
    let listed = run(handle(
        &config(),
        request("GET", "/bundles", vec![]),
        &BrokenStore,
    ));
    assert_eq!(listed.status, 500);
    assert!(!String::from_utf8_lossy(&listed.body).contains("R2 is down"));
    assert_eq!(listed.log.as_deref(), Some("storage: R2 is down"));
}
