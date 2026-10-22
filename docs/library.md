# Scrawl as a library: plan

Status: phases 1–7 done; phase 10 (`scrawl-web`) is under way. See **Status and handoff** below. Written 2026-10-19.

Goal: any app can add Scrawl's capture-and-compile loop as a dependency, in three shapes:

1. **Rust crate** for Rust apps and tools.
2. **Swift package** for iOS (exists: ScrawlKit) and macOS (new).
3. **npm package** for web apps.


## Status and handoff

Updated 2026-10-22. Start here if you are picking this work up.

### Done (all merged into `main`)

| Phase | PR | What landed |
|---|---|---|
| Plan | #1 | This document |
| 1 | #2 | `src/lib.rs` is the crate; `src/main.rs` only calls `scrawl::run()` |
| 2 | #3 | The pipeline moved into `src/compile.rs` |
| 3 | #5 | `BuildOptions`: the core takes `agentContext` and autocrop as inputs and reads no config or clock; `WindowSnapshot`, `WindowEntry`, `CursorSample` and `CropKind` live in `model` |
| 4 | #6 | Golden test: the demo bundle's `scrawl.json` and `summary.md` in `tests/golden/` |
| 5 | #7 | Workspace: `crates/scrawl-bundle` (format) and `crates/scrawl-core` (compiler); `system-fonts` feature; Linux and wasm CI job |
| 6a | #8, #9 | `scrawl_core::build` (the whole bundle pipeline) and `scrawl_core::demo`; the golden test runs in the core |
| Fix | #4 | `windows::tests::logger_snapshots_at_start_and_right_after_a_request` polls instead of sleeping |

Another Rust app can already depend on `scrawl-core` and call `build::build_bundle_from_parts` with its own frames, strokes, events and transcript, and implement `scrawl-capture`'s traits for its own sources.

### Done: phase 6b, capture traits

Goal: a `scrawl-capture` crate with traits for what the app records, and the macOS and Windows implementations behind them, so another Rust app can record as well as compile.

- Screen: `ScreenCaptureSession` and `ScreenRecording` in `src/capture.rs`, with the platform side in `src/capture/macos.rs` and `src/capture/windows.rs` (`pub use platform::*`).
- Target: `discover_target` / `discover_target_at` (one per platform module).
- Audio: `AudioRecorder` and `RecordedAudio` in `src/audio.rs`, and `src/mic.rs`.
- Window and cursor log: `windows::WindowLogger` and `ActivityLog` in `src/windows.rs`.
- Start by drawing the trait boundary in the app (traits in `src/capture.rs`, the existing code as their implementation) and only then move files into a crate, as phases 1–5 did.
- Suggested order: the `Transcriber` trait first (smallest, `src/transcribe.rs`), then audio, then screen.
- **`Transcriber` (#11, merged):** `transcribe::Transcriber` (`name`, `transcribe(Speech) -> Result<Option<TranscriptData>>`) with `Hex`, `Live` and `Local` as its implementations; `transcribe_with` tries engines in order (`Ok(None)` or an error passes to the next; none answering is a missing transcript). `Speech` carries the WAV path and bytes, so the trait no longer needs `RecordedAudio`; `hex::transcribe_audio` takes WAV bytes. 
- **Audio (#12, merged):** `audio::AudioCapture` (`stop(self: Box<Self>) -> Result<RecordedAudio>`), implemented by `AudioRecorder`; the GPUI capture holds a `Box<dyn AudioCapture>`. `RecordedAudio.live` is now a `transcribe::Slot` holding any `Box<dyn Transcriber + Send>`, so `RecordedAudio` no longer names `live_transcribe`. Microphone choice (`mic`) and starting a recorder stay concrete for now. 
- **Screen (#13, merged):** `capture::ScreenCapture` (`request_immediate_frame`, `elapsed_ms`, `stop(self: Box<Self>) -> ScreenRecording`), implemented by `ScreenCaptureSession`; the GPUI capture holds a `Box<dyn ScreenCapture>`. Starting a session, `discover_target`, and the per-platform sampler (`platform::run_sampler(Sampler)`) stay concrete.
- **Window and cursor log (#14, merged):** `windows::ActivityCapture` (`stop(self: Box<Self>) -> ActivityLog`), implemented by `WindowLogger`; the GPUI capture holds a `Box<dyn ActivityCapture>`. `ActivityLog` (with its dedupe in `push_windows` / `push_cursor`) has no OS calls and can move as is. `scrawl record` still uses `WindowLogger` directly. `discover_target` stays a per-platform function: it answers once, before recording, so another app passes its own `TargetInfo` to `ScreenCaptureSession::start` instead of implementing a trait.
- **`crates/scrawl-capture` (#15, merged):** the four traits and what they hand back, with no OS calls: `transcribe::{Speech, Transcriber, Slot}`, `audio::{AudioCapture, RecordedAudio}`, `screen::{ScreenCapture, ScreenRecording}` and `activity::{ActivityCapture, ActivityLog}` (with its dedupe and tests). The app re-exports each under its old path (`crate::audio::RecordedAudio` and so on), so callers did not change; `Slot::live` became the app function `transcribe::live_slot`, since an app can't add inherent methods to a crate's type. CI tests the crate on Linux and builds it for wasm.
- **Window list (#16, merged):** the window-server-independent half of `src/windows.rs` moved to `scrawl_capture::activity`: `RawWindow`, `AppMeta`, `filter_windows`, the system-prompt checks (`system_prompts_over`, `system_prompt_at`, `macos_is_asking`) and `owned_window_at`, with their tests. `src/windows.rs` keeps the OS reads (`raw_windows`, `dock_rect`, `snapshot`) and `WindowLogger`. On Linux the app's `cargo check` now also warns that a few re-exports are unused, because their only users are macOS or Windows code.
- **Window logger (#17, merged):** `scrawl_capture::activity::WindowLogger` is the worker (its tick, snapshot triggers and the closing log line) over a `WindowSource` trait (`cursor`, `frontmost_pid`, `windows`, `log`), tested on Linux with a fake source. The app's `windows::ServerWindows` is the window-server source (it reads the Dock's strip on the main thread when made), and `windows::start_logger` starts a logger on it.
- **Audio engine (#18, merged):** `scrawl_capture::audio` holds the recorder: `Recorder` runs an input on its own thread (`start(thread_name, open, log)`, where `open` starts the input feeding a `Sink` and returns what keeps it running) and turns what it heard into `RecordedAudio` on `stop`; `Downmix` is the microphone callback's area-averaging downmix to 16 kHz mono; and the WAV helpers (`encode_wav_16k`, `decode_wav_16k`, `pcm16`, `recorded_audio_from_wav`) and `speech_islands` came with it, with their tests. The app keeps choosing and opening the device with `cpal` (`audio::start_recorder`, `mic`) and live transcription.
- **Screen engine (#19, merged):** `scrawl_capture::screen::Session` runs a sampler (`FnOnce(Sampler) -> Option<PathBuf>`) on its own thread and collects its candidate frames and movie on `stop`; `CandidateBuffer`, `Sampler` and the sampling constants moved with it, with their tests. The app's `ScreenCaptureSession` runs `platform::run_sampler` (ScreenCaptureKit, or Windows.Graphics.Capture and Media Foundation) in one, holds retention off while it records, and adds a placeholder frame when none was taken. `capture_size` stays in the app (it depends on `MAX_CAPTURE_SCALE`, per platform).
- **Native window server (#20, merged):** `scrawl-capture`'s default `native` feature (macOS and Windows only; target-gated dependencies on `core-foundation`, `core-graphics`, `objc2`, `objc2-app-kit`, `objc2-foundation` and `windows`) adds `native::window_server` (`raw_windows`, `app_meta_of`, `dock_rect`, `snapshot(frame, dock, front, cache)`) and `native::win` (displays, top-level windows, processes and `file_description`, from `src/win/mod.rs` and `src/win/shell.rs`). The app re-exports them (`src/win/mod.rs` glob-imports `native::win`) and keeps `windows::snapshot`, which passes `capture::frontmost_external_pid` as `front`.
- **Native window source (#21, merged):** `native::window_server::NativeWindows` is a ready `WindowSource` (`cursor`, `frontmost_pid`, `snapshot`, the Dock's strip read on the main thread when made), with `with_frontmost` and `with_log`. The app's `windows::start_logger` runs one with `capture::frontmost_external_pid` and `scrawl_log`, replacing its `ServerWindows`. Another app: `WindowLogger::start(NativeWindows::new(frame), frame, started_at, request)`.
- **Native microphone (#22, merged):** `native::mic` (with `cpal`, macOS and Windows only, re-exported as `native::mic::cpal`): `open_device(device, sink, log)` opens a device at its default format into a `Sink` through `Downmix`, `device(id)` finds one, `device_name`, and `start_default(log)` records the default input in a `Recorder`. The app's `audio::start_recorder` keeps choosing the device (`mic`, config) and live transcription, and opens it with `native::mic::open_device`.
- **Native macOS screen (#23, merged):** `native::sck::run_sampler(sampler, hooks)` is the ScreenCaptureKit recorder (the movie, candidate screenshots, leaving out the recorder's overlays and, when ScreenCaptureKit lists them, macOS's prompts) that `screen::Session::start` runs on macOS. `native::SamplerHooks` carries the app's log, a ready callback (the app's `sck_ready_ms` metric) and the placeholder frame (`native::blank_frame` by default). `screen::{capture_size, source_rect, clamp_to, to_local, Rect}` moved from `src/capture.rs` and `src/screens.rs`. `DOCS_RS=1 cargo check -p scrawl-capture --target aarch64-apple-darwin` type-checks it: the `screencapturekit` build scripts skip the SDK when `DOCS_RS` is set.
- **Native Windows screen (#24, merged):** `native::wgc::run_sampler(sampler, hooks)` is the Windows.Graphics.Capture recorder (D3D11 video processor crop, scale and NV12; Media Foundation H.264 sink writer; BGRA candidates), moved from `src/win/media.rs` with the same `SamplerHooks`. `media_startup` and `unpack` are public for the app's decoding (`frames`, `probe`, `export_audio`), which stays in `src/win/media.rs` with the helper's types.
- **Result:** every recorder is an engine in the crate (`screen::Session`, `audio::Recorder`, `activity::WindowLogger`) with its macOS and Windows sources behind the default `native` feature. Still in the app: choosing the microphone and the live transcriber, the `Hex` / `Live` / `Local` transcription engines and `transcribe_with` (they lean on `helper` and config), retention, and `discover_target`.
- **Cross-check the crates for macOS and Windows.** `rustup target add aarch64-apple-darwin x86_64-pc-windows-gnu`, then `DOCS_RS=1 cargo check -p scrawl-capture --target aarch64-apple-darwin` and `--target x86_64-pc-windows-gnu` type-check the crate's macOS and Windows code on Linux (no linking, so no SDK). The whole app doesn't cross-check: `transcribe-cpp-sys` (Windows) builds C++ with CMake, and `ring` (macOS) compiles C against the macOS SDK.
- **Linux doesn't type-check the app.** Name-resolution errors stop `cargo check` before type checking, so a type error in app code is invisible on Linux. To test a module that has no OS calls, include it in a scratch crate with `#[path = ".../src/<module>.rs"]` and stub its `crate::` neighbours.

### Done: phase 7, `scrawl-route`

Goal: a `crates/scrawl-route` crate with what happens to a bundle once it is built (`src/delivery.rs`, `src/routing.rs` with `routing/smart.rs` and `routing/look.rs`, `src/web.rs`, and the OpenCode API calls in `src/handoff.rs`), so OpenCode delivery is something an app opts into. Same method as 6b: one module per PR, the app re-exporting each under its old path.

- **Delivery queue (#25, merged):** `scrawl_route::delivery` is `src/delivery.rs` (`Delivery`, `Outcome`, `reply_json`, the `.deliver.json` `Pending` marker, `deliver`, `retry_due`, `spawn_retries`). Each function that logs takes a `scrawl_route::Log` (`fn(&str)`); the app's `src/delivery.rs` keeps the old signatures and passes `scrawl_log`. CI tests the crate on Linux and builds it for wasm.
- **OpenCode lookups (#26, merged):** `scrawl_route::opencode` holds `OpenCodeSession` (with its V1/V2 `directory`), `ApiGet`, `get_session`, `find_root_session` with `Scan` and `ROOT_SEARCH_PAGES`, `query_escape`, and `resolve_opencode_session_with` (an explicit ID, an app title match, `env_session`, the newest root), with their tests. The transport stays in the app (`handoff::api_get` runs `opencode2 api get`, locally or over SSH), as does sending. `opencode::fake_api`, the fake session API the routing and phone tests serve from, is behind the crate's `fake-api` feature, which the app turns on as a dev-dependency.
- **Page registrations (#27, merged):** `scrawl_route::web` is `src/web.rs` with `src/sdk.js` (now `crates/scrawl-route/src/sdk.js`): the script, the `Registry` of live pages, who may register (`may_register`, `constant_time_eq`), the `/sdk.js` and `/web/*` routes (`handle`), and `fetch_registrations(port, token)`. `handle` takes `web::Hooks` (the log, whether pages should send context, and what keeps it); `lookup(url, remote)` asks `remote` when this process isn't serving. `scrawl_route::url` holds `WebUrl`, `parse_url` and `is_loopback_host` from `routing`. The app's `src/web.rs` passes Scrawl's log, the `agentContext` experiment, `page::record`, the ingest port and token.
- **Routing decision (#28, merged):** `scrawl_route::smart` is `src/routing/smart.rs`, moved whole with its tests: the `Evidence`, `Candidate` and `Clue` types, `decide` (the ladder and weighing policies, no clock and no I/O), `destination` and `explain`. The app's `routing::smart` re-exports it. `is_home_or_root` is now `pub`, because `look` (still in the app) calls it. The crate gains `scrawl-bundle` (for `Destination`) and `dirs`.
- **Routing ladder (#29, merged):** `scrawl_route::ladder` holds what `src/routing.rs` decides without reaching outside: `Browser` and `browser_kind`, the `lsof` output parsers, `project_root`, `session_for_directory`, `Hooks` (`sdk`, `listener_dir`, `api`) and `route`, and the `routing.json` record (`RECORD`, `write_record`, `read_record`), with their tests. `Hooks` has no `Default` in the crate; the app's `routing::hooks()` passes `web::lookup`, `lsof` and `opencode2 api`. Still in the app: the OS calls (`browser_tab_url` through `osascript`, `listener_directory` through `lsof`, `run_with_timeout`), `route_capture`, `look_capture`, `judge_capture`, `explain_bundle`, `record_destination` (it writes `summary.md`), and `look`, which is all I/O (the tab, the terminal pane, Tailscale, OpenCode Desktop's state) and feeds `smart::decide`.
- **After phase 7:** sending (`send_bundle_to_session`) runs the `opencode` binary and stays in the app. Another app routes with `scrawl_route::ladder::route` and its own `Hooks`, or builds `smart::Evidence` itself and calls `smart::decide`.

### Next: phase 10, `scrawl-web`

Goal: a web app records its own page and compiles the bundle in the browser (**npm package** below). Phase 8 (`scrawl-ffi`) and 9 (ScrawlKit macOS) need a Mac to build and test, so 10 went first.

- **The core runs in wasm (#30, merged):** `scrawl-core` built for wasm already, but couldn't run there: `std::thread::spawn` and `Instant::now` panic on `wasm32-unknown-unknown`, and the build wrote to disk. Now:
  - `par::map` and `par::join` run its parallel work (keyframe grids and thumbnails, autocrop's content and annotation, contact sheets, PNG encoding) on threads natively and in turn on wasm, results in order.
  - `clock::Instant` is `std::time::Instant` natively; on wasm every instant is the same, so the `finish timings` read 0 ms.
  - `build::build_bundle` builds in memory and hands back a `BuiltBundle` (`bundle`, `summary`, `files`, images first and `summary.md` last); `build_bundle_from_parts` is it plus `build::write_files`. `demo::demo_built` is the demo bundle built that way.
  - Checked: the demo bundle's files are byte for byte the same before and after, and the same in wasm (in Node, about 0.3 s) as native.
- **`crates/scrawl-wasm` (#31, merged):** the compiler for JavaScript through `wasm-bindgen` (0.2.129, already in the lockfile). `new Compiler()`, `addFrame(tMs, width, height, rgba)` (an `ImageData`'s pixels), then `compile(JSON.stringify(capture))` returns a `Bundle` with `fileNames()`, `file(name)` (bytes), `summary` and `manifest`. A capture is `scrawl.json`'s own types in camelCase (`target`, `durationMs`, `strokes`, `events`, `transcript`, `flags`, `context`, `source`, `id`, `createdAtMs`, `agentContext`); without a transcript the bundle's `transcript.source` is `none`. Bad frames or JSON are a thrown `Error`, never a panic. In Rust it's `scrawl_wasm::compile(Capture, frames)`.
  - CI's `Linux core and wasm` job installs `wasm-bindgen-cli` 0.2.129 (it must match the lockfile's `wasm-bindgen`), builds the crate for wasm, and runs `crates/scrawl-wasm/tests/node.mjs`, which compiles a four-frame capture in Node and checks the files, PNGs, `scrawl.json` and `summary.md` (about 60 ms).
- **`packages/scrawl-web` (`scrawl-web` on npm, #32, merged):** the page recorder, plain ES modules with no dependencies. `createScrawl(options)`, `start()`, `stop()` returns the bundle (`files`, `summary`, `manifest`, `audio`, `file(name)`, `zip()`).
  - Frames: the tab's video (`getDisplayMedia` with `preferCurrentTab`), sampled every `interval` ms (500) and 150 ms after each click, at most `maxEdge` px (1600), skipping frames whose 16×16 luma thumbnail didn't change, kept as WebP (PNG on Safari) until the build.
  - Input: clicks with a short label (the rules `sdk.js` uses; never typed values), scrolls, and ⇧ ink (hold ⇧ and move). The page shows the ink while it's drawn and fades it; frame sampling pauses from the stroke's start until it has faded, so frames never show it and Scrawl draws it once, from `strokes`.
  - Transcription (decision 4): the page's own `transcribe(audio, { durationMs })` (the microphone through `MediaRecorder`), or the opt-in `"web-speech"` (live `SpeechRecognition` in Chrome, Edge and Safari; phrase times, `estimated`), or nothing (`transcript.missing`).
  - The build runs in a module Worker (`src/worker.js`), which decodes the frames and calls `scrawl-wasm` built with `wasm-bindgen --target web` (`scripts/build-wasm.sh` into `wasm/`). `zip()` is a stored (uncompressed) zip with a `<id>.scrawl/` folder.
  - Tests: `npm test` (zip checked by Python's `zipfile`, transcripts, strokes, change detection, Web Speech with a fake recognizer, and a compile through the browser wasm in Node), and `test/browser/run.mjs`, which records `test/browser/index.html` in Chromium through Playwright (tab capture auto-accepted, a click, ⇧ ink, a fake transcriber) and checks the bundle. CI's `Web package` job runs both.
- **Names for publishing (decision 2):** the five library crates keep their names (`scrawl-bundle`, `scrawl-capture`, `scrawl-core`, `scrawl-route`, `scrawl-wasm`), all free on crates.io; their path dependencies carry `version = "0.3.5"` and each has `repository`, so `cargo publish --dry-run -p scrawl-bundle -p scrawl-capture -p scrawl-core -p scrawl-route -p scrawl-wasm` packages and verifies all five in order. The npm package is `scrawl-web` (unscoped): the `@scrawl` scope belongs to another account. `npm pack` builds the wasm first (`prepack`). Nothing is published yet; that needs the owner's crates.io and npm logins.
- **Page context (in review, branch `claude/web-page-context`):** with `agentContext: true`, `scrawl-web` records what `sdk.js` tells the Mac app (`src/context.js`): the element, component and source `file:line` under clicks and ⇧ marks, console errors, failed requests and video playback, as `page::Event`s at epoch ms. They go to the compiler as the capture's `page`, and `scrawl-wasm` runs the app's own `page::clean` and `page::apply`, so labels read the same as a Mac capture's (`<button#save> “Save” in Toolbar (src/Toolbar.tsx:42)`, `console error: …`, `GET /api/x → 404`). `page::clean` is `Log`'s trimming without its 3-minute window. The page's `console.error`, `fetch` and XHR are wrapped only while recording and put back on `stop()`.
- **Frame memory (in review, branch `claude/wasm-frame-memory`):** the Worker decodes each sampled frame at 480 px (`CANDIDATE_EDGE`, the Mac app's candidate size) for keyframe selection, asks `Compiler.keyframeTimes(capture)` which it picked (`build::keyframe_times`, the same selection `build_bundle` runs), and decodes only those again at full size for `addFullFrame`; `compile` hands them to `build_bundle` through `SourceExtras::full_frames`, as phone movies do. A 300-frame 1600×900 recording peaks at about 200 MB of wasm memory instead of 1.7 GB, and builds in about 1 s instead of 13 s. Selection on small candidates can pick a slightly different frame in long stretches of steady motion, as on the Mac.
- **Next:** an optional POST of the zip to a Scrawl endpoint with the ingest token (only useful with the Mac app running).

After 10 come phases 8 (`scrawl-ffi` for ScrawlKit) and 9 (ScrawlKit macOS), as in **Phases** below.

### Working notes

- **Linux can't build the app.** Its macOS/Windows code fails `cargo check` on Linux with about 113 error lines. Check a change by comparing the sorted error set (messages, without line numbers) of `cargo check -p scrawl --tests` against `main`: they must be identical. The crates build and test normally: `cargo test -p scrawl-bundle -p scrawl-capture -p scrawl-core -p scrawl-route -p scrawl-wasm`, `cargo build -p scrawl-capture -p scrawl-core -p scrawl-route --target wasm32-unknown-unknown`.
- **CI** (`.github/workflows/check.yml`) runs macOS, Windows and `Linux core and wasm`, about 5–15 minutes. It is the only real build of the app. Plain `cargo test` covers the crates through `default-members`.
- **Golden files.** `SCRAWL_UPDATE_GOLDEN=1 cargo test -p scrawl-core golden` rewrites them (runs on Linux). If a change must alter them, review the diff. When files can only be made on CI, the test prints each mismatching file between `----- actual … -----` markers; the raw job log (GitHub's job-logs URL) can be downloaded and the blocks extracted with a script. `.gitattributes` keeps `tests/golden/**` LF for Windows.
- **Formatting.** Several files predate rustfmt and have existing diffs; compare `rustfmt --check` hunk counts against `main` rather than reformatting whole files.
- **Review.** CodeRabbit reviews PRs that are not drafts, including PRs into `main`, on its own; an extra `@coderabbitai review` comment is skipped once it has seen the commit. It skips `docs/**`. Its plan allows 5 reviews an hour.
- **Stacked PRs.** Deleting a merged PR's branch closes the PRs that target it, so point stacked PRs at `main` before their base merges.

## The system today

Think of Scrawl as a film crew plus an editing room. The crew (capture) records picture, sound and the
director's pointing (cursor, clicks, `⇧` ink). The editing room (bundle compiler) cuts that footage
into a short reel (`contact-sheet.png`, `frames/`, `summary.md`, `scrawl.json`) an agent can read in
one glance. A courier (delivery/routing) hands the reel to the right OpenCode session.

Today the editing room exists in exactly one place: the Mac (or Windows) app. ScrawlKit and `sdk.js`
are film crews with no editing room of their own; they ship raw footage to the app.

```
 ScrawlKit (iOS) ──movie+timeline.json──┐
                                         │  HTTP / USB
 sdk.js (web page) ──clicks, context────┤
                                         ▼
 ┌────────────────────── scrawl binary (one crate, 43k lines) ─────────────────────┐
 │ capture · mic · hotkeys · ui (gpui) · settings · onboarding   ← app shell      │
 │ keyframes · sketch · render · autocrop · page · agent_context ← bundle compiler │
 │ transcribe · hex · helper (Swift: AVFoundation decode, SpeechAnalyzer)          │
 │ routing · delivery · web · phone · inbox                       ← courier        │
 └──────────────────────────────────────────────────────────────────────────────────┘
                                         ▼
                          ~/Library/Application Support/scrawl/captures/*.scrawl
```

So "make it a library" really means: **pull the editing room out of the app so it can run inside
anyone's process**, and give each platform a crew that feeds it.

## Target shape

```
                    ┌──────────────── scrawl-bundle ────────────────┐
                    │ format types (model.rs), serde, version: 1    │
                    └───────────────────────▲───────────────────────┘
                                            │
                    ┌──────────────── scrawl-core ──────────────────┐
                    │ Timeline + frames  ──compile()──▶  Bundle     │
                    │ keyframes, ink geometry, render, autocrop,    │
                    │ summary.md, contact sheet. Pure: no OS calls, │
                    │ no config file, no clock. Builds for wasm32.  │
                    └───▲──────────────▲──────────────▲─────────────┘
                        │              │              │
          ┌─────────────┴───┐  ┌───────┴───────┐  ┌───┴──────────────┐
          │ scrawl-ffi      │  │ scrawl-wasm   │  │ scrawl-capture   │
          │ UniFFI → Swift  │  │ wasm-bindgen  │  │ traits + macOS / │
          │ xcframework     │  │ → npm         │  │ Windows impls    │
          └───────▲─────────┘  └──────▲────────┘  └────▲─────────────┘
                  │                   │                │
      ScrawlKit (iOS + macOS)   scrawl-web (npm)    scrawl app (gpui shell,
                                                    settings, onboarding)
                                                          │
                                              scrawl-route (optional crate:
                                              OpenCode routing + delivery)
```

The app becomes one consumer of the library, not the library itself.

### The seam: one input type

Every crew produces the same thing, so `scrawl-core` takes one input. ScrawlKit's `timeline.json`
already is most of it; `build_bundle_from_parts` (`src/main.rs:932`) already takes it in pieces.

```rust
pub struct Timeline {
    pub target: TargetInfo,              // what was recorded (app, window, URL, display)
    pub duration_ms: u64,
    pub strokes: Vec<Stroke>,            // ⇧ ink
    pub events: Vec<InteractionEvent>,   // clicks, keys, page context
    pub transcript: TranscriptData,      // already transcribed, or `missing`
    pub flags: Vec<u64>,
    pub note: Option<String>,
}

pub trait FrameSource {                  // the platform decodes video; core never does
    fn candidates(&mut self) -> Result<Vec<RgbaFrame>>;            // small, for selection
    fn full(&mut self, times_ms: &[u64]) -> Result<Vec<RgbaFrame>>; // keyframes at full size
}

pub struct Options {                     // replaces global config reads
    pub experiments: BTreeSet<String>,
    pub autocrop: Option<autocrop::Settings>,
    pub created_at_ms: u64,              // replaces now_epoch_ms()
    pub id: String,                      // replaces make_bundle_id()
}

pub fn compile(timeline: Timeline, frames: &mut dyn FrameSource, opts: &Options)
    -> Result<Compiled>;                 // Compiled = ScrawlBundle + PNG bytes + summary.md

impl Compiled { pub fn write_to(&self, dir: &Path) -> Result<PathBuf>; }   // optional fs step
```

Transcription stays outside core (SpeechAnalyzer, Hex, Parakeet, Web Speech are all platform
services). A `Transcriber` trait lives in `scrawl-capture`; the result goes in `Timeline.transcript`.

## Coupling that has to be cut

The bundle compiler is less tangled than the module graph suggests. The edges from the core
candidates (`model`, `sketch`, `keyframes`, `render`, `autocrop`, `page`, `agent_context`) into the
app are few:

| Edge | Where | Fix |
|---|---|---|
| Global experiment reads | `sketch.rs:466`, `render.rs:381` (`handoff::experiment(AGENT_CONTEXT)`) | Read `Options.experiments` |
| Config file read | `autocrop.rs:235` (`handoff::load_config_from`) | Pass `Options.autocrop` |
| Type borrowed from capture | `model.rs:5` re-exports `windows::{CursorSample, WindowSnapshot}` | Move both types into `model` |
| Type borrowed from autocrop | `model.rs:158` `autocrop::CropKind` | Move `CropKind` into `model` |
| `autocrop` → `windows::WindowEntry` | `autocrop.rs` | Move `WindowEntry` into `model` |
| `page` → `crate::now_epoch_ms` | `page.rs` | Take the time as a parameter |
| `sketch` → `ax`, `speech_model` | docs and a test fixture | Point at core types; test builds its own `missing` |
| Pipeline lives in `main.rs` | `prepare_visuals`, `build_bundle_from_parts`, `merge_full_keyframes` | Move to `core::compile` |
| Video decode inside pipeline | `movie_frames_at` → `helper::frames` (Swift) | Becomes the app's `FrameSource` impl |

Everything else (gpui, ScreenCaptureKit, Metal, mic, hotkeys, settings, onboarding, routing) stays
in the app or moves to `scrawl-capture` / `scrawl-route`.

## Phases

Each phase is one PR that keeps both CI jobs (`.github/workflows/check.yml`) green and changes no
bundle output. Estimates assume one person who knows the code.

| # | Change | Output | Est. |
|---|---|---|---|
| 1 | Add `src/lib.rs`; `main.rs` keeps only `fn main` | Same binary, crate is importable | 0.5 day |
| 2 | Move `now_epoch_ms`, `make_bundle_id`, `prepare_visuals`, `build_bundle_from_parts` into `src/compile.rs` | `main.rs` under 300 lines | 1 day |
| 3 | Cut the edges in the table above; add `Options` and `FrameSource` | Core modules import only `model` and each other | 2 days |
| 4 | Golden test: reprocess fixture bundles through `compile()` and byte-compare `scrawl.json` and `summary.md` against the old path | Proof that 1–3 changed nothing | 1 day |
| 5 | Cargo workspace: `crates/scrawl-bundle`, `crates/scrawl-core`, app at root; CI adds a Linux + `wasm32-unknown-unknown` build of core | Publishable `scrawl-core` | 1 day |
| 6 | `scrawl-capture` traits; move `capture/macos.rs`, `capture/windows.rs`, `mic`, `audio` behind them | Rust apps can capture, not only compile | 3 days |
| 7 | `scrawl-route` crate from `routing`, `delivery`, `web` | OpenCode delivery is opt-in | 2 days |
| 8 | `scrawl-ffi` (UniFFI) + xcframework build script; ScrawlKit can compile on device | iOS apps get bundles without a Mac | 3 days |
| 9 | ScrawlKit macOS target: ScreenCaptureKit window recorder, reuse `Ink`, `InkView`, `Overlay` via AppKit hosting | Mac apps embed Scrawl | 4 days |
| 10 | `scrawl-wasm` + `scrawl-web`: page recorder (below), compile in a Worker, output a zip or POST | Web apps embed Scrawl | 4 days |

Total: about 21 working days. Phases 1–5 (about 5.5 days) deliver the Rust crate and unblock the rest.

## Per-target notes

### Rust crate

- Published crates: `scrawl-bundle`, `scrawl-core`, later `scrawl-capture`, `scrawl-route`.
- `scrawl-core` deps: `serde`, `serde_json`, `png`, `anyhow`. No `gpui`, no OS crates.
- `scrawl reprocess` / `scrawl inspect` become thin wrappers over `compile()`, so the CLI and the
  library can't drift.

### Swift package (iOS + macOS)

- Keep the product name `ScrawlKit` and its public API (`.scrawl()`, `Scrawl.shared`, `ScrawlConfig`).
- Add a binary target `ScrawlCore.xcframework` built from `scrawl-ffi`. A new option
  `Scrawl.shared.compiles = .onDevice | .onMac` picks local compile or today's upload.
- Frames come from `AVAssetImageGenerator` (the same thing `helper/Media.swift` does on the Mac), so
  the `FrameSource` impl is Swift, passed across FFI as a callback interface.
- macOS: `#if os(iOS)` guards already isolate UIKit. Add an `#if os(macOS)` recorder on
  ScreenCaptureKit scoped to the host app's own windows, so no Screen Recording prompt beyond the
  app's own.

### npm package (`scrawl-web`)

- Records only the host page, no extension needed:
  - frames: `getDisplayMedia({ preferCurrentTab: true })` into a `<video>`, sampled to `ImageBitmap`
    at the times `compile()` asks for; fallback is DOM snapshots via `html2canvas`-style rendering.
  - voice: `MediaRecorder`; transcript via an injectable `transcribe(blob)` callback (the app's own
    server, Web Speech, or none, which records `transcript.missing`).
  - ink, clicks, element context: reuse `sdk.js` (it already describes elements, source
    `file:line`, console errors and failed requests).
- `compile()` runs in a Web Worker from `scrawl-wasm`. Output: a `.scrawl` zip `Blob`, or POST to a
  Scrawl app/agent endpoint with the existing ingest token.
- `sdk.js` keeps working unchanged for people who run the Mac app.

## Rules this keeps

- Bundle format stays `version: 1`; any change to output goes behind an experiment, off by default
  (`Options.experiments` is how a library caller turns one on).
- Images stay visual, text carries meaning (`docs/keyframes-and-text.md`): the library writes the
  same files, it does not add captions.
- Tests keep swapping outside steps through hooks (`IngestHooks`, `routing::Hooks`); the new traits
  are the same idea one layer down.

## Decisions needed

1. ~~**Workspace vs `Cargo.toml` stability.**~~ Decided 2026-10-20: a multi-crate workspace is
   fine. Phase 5 goes ahead as written.
2. ~~**Crate names on crates.io / npm scope.**~~ Decided 2026-10-22: the crates keep their `scrawl-*` names (all free; the app crate `scrawl` is taken but isn't published), and the npm package is `scrawl-web`, since the `@scrawl` scope belongs to another account.
3. **Is OpenCode delivery part of the library story,** or app-only? This plan makes it optional.
4. ~~**Web transcription default.**~~ Decided 2026-10-22: the page's own `transcribe` function, with `"web-speech"` as an opt-in; without either, no transcript.
