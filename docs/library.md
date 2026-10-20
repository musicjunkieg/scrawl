# Scrawl as a library: plan

Status: phases 1–6a done; 6b (capture traits) is next. See **Status and handoff** below. Written 2026-10-19.

Goal: any app can add Scrawl's capture-and-compile loop as a dependency, in three shapes:

1. **Rust crate** for Rust apps and tools.
2. **Swift package** for iOS (exists: ScrawlKit) and macOS (new).
3. **npm package** for web apps.


## Status and handoff

Updated 2026-10-20. Start here if you are picking this work up.

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

Another Rust app can already depend on `scrawl-core` and call `build::build_bundle_from_parts` with its own frames, strokes, events and transcript.

### Next: phase 6b, capture traits

Goal: a `scrawl-capture` crate with traits for what the app records, and the macOS and Windows implementations behind them, so another Rust app can record as well as compile.

- Screen: `ScreenCaptureSession` and `ScreenRecording` in `src/capture.rs`, with the platform side in `src/capture/macos.rs` and `src/capture/windows.rs` (`pub use platform::*`).
- Target: `discover_target` / `discover_target_at` (one per platform module).
- Audio: `AudioRecorder` and `RecordedAudio` in `src/audio.rs`, and `src/mic.rs`.
- Window and cursor log: `windows::WindowLogger` and `ActivityLog` in `src/windows.rs`.
- Start by drawing the trait boundary in the app (traits in `src/capture.rs`, the existing code as their implementation) and only then move files into a crate, as phases 1–5 did.
- Suggested order: the `Transcriber` trait first (smallest, `src/transcribe.rs`), then audio, then screen.
- **`Transcriber` (in review, branch `claude/transcriber-trait`):** `transcribe::Transcriber` (`name`, `transcribe(Speech) -> Result<Option<TranscriptData>>`) with `Hex`, `Live` and `Local` as its implementations; `transcribe_with` tries engines in order (`Ok(None)` or an error passes to the next; none answering is a missing transcript). `Speech` carries the WAV path and bytes, so the trait no longer needs `RecordedAudio`; `hex::transcribe_audio` takes WAV bytes. 
- **Audio (in review, branch `claude/audio-trait`, stacked on the transcriber PR):** `audio::AudioCapture` (`stop(self: Box<Self>) -> Result<RecordedAudio>`), implemented by `AudioRecorder`; the GPUI capture holds a `Box<dyn AudioCapture>`. `RecordedAudio.live` is now a `transcribe::Slot` holding any `Box<dyn Transcriber + Send>`, so `RecordedAudio` no longer names `live_transcribe`. Microphone choice (`mic`) and starting a recorder stay concrete for now. 
- **Screen (in review, branch `claude/screen-trait`, stacked on the audio PR):** `capture::ScreenCapture` (`request_immediate_frame`, `elapsed_ms`, `stop(self: Box<Self>) -> ScreenRecording`), implemented by `ScreenCaptureSession`; the GPUI capture holds a `Box<dyn ScreenCapture>`. Starting a session, `discover_target`, and the per-platform sampler (`platform::run_sampler(Sampler)`) stay concrete.
- **Next, once the three trait PRs merge:** decide whether the sampler is the better seam for other apps (they would supply only frame grabbing; `ScreenCaptureSession` keeps the candidate buffer and timing), then the window and cursor log (`WindowLogger`, `ActivityLog`), then move the traits and `RecordedAudio` / `ScreenRecording` into `crates/scrawl-capture`.
- **Linux doesn't type-check the app.** Name-resolution errors stop `cargo check` before type checking, so a type error in app code is invisible on Linux. To test a module that has no OS calls, include it in a scratch crate with `#[path = ".../src/<module>.rs"]` and stub its `crate::` neighbours.

After 6b come phases 7 (`scrawl-route`), 8 (`scrawl-ffi` for ScrawlKit), 9 (ScrawlKit macOS) and 10 (`@scrawl/web`), as in **Phases** below.

### Working notes

- **Linux can't build the app.** Its macOS/Windows code fails `cargo check` on Linux with about 113 error lines. Check a change by comparing the sorted error set (messages, without line numbers) of `cargo check -p scrawl --tests` against `main`: they must be identical. The crates build and test normally: `cargo test -p scrawl-bundle -p scrawl-core`, `cargo build -p scrawl-core --target wasm32-unknown-unknown`.
- **CI** (`.github/workflows/check.yml`) runs macOS, Windows and `Linux core and wasm`, about 5–15 minutes. It is the only real build of the app. Plain `cargo test` covers the crates through `default-members`.
- **Golden files.** `SCRAWL_UPDATE_GOLDEN=1 cargo test -p scrawl-core golden` rewrites them (runs on Linux). If a change must alter them, review the diff. When files can only be made on CI, the test prints each mismatching file between `----- actual … -----` markers; the raw job log (GitHub's job-logs URL) can be downloaded and the blocks extracted with a script. `.gitattributes` keeps `tests/golden/**` LF for Windows.
- **Formatting.** Several files predate rustfmt and have existing diffs; compare `rustfmt --check` hunk counts against `main` rather than reformatting whole files.
- **Review.** CodeRabbit reviews PRs that are not drafts. It skips `docs/**` and auto-reviews only PRs into `staging` (comment `@coderabbitai review` on others). Its plan allows 5 reviews an hour.
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
      ScrawlKit (iOS + macOS)   @scrawl/web (npm)   scrawl app (gpui shell,
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
| 10 | `scrawl-wasm` + `@scrawl/web`: page recorder (below), compile in a Worker, output a zip or POST | Web apps embed Scrawl | 4 days |

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

### npm package (`@scrawl/web`)

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
2. **Crate names on crates.io / npm scope.** `scrawl` and `@scrawl` may be taken.
3. **Is OpenCode delivery part of the library story,** or app-only? This plan makes it optional.
4. **Web transcription default:** none, Web Speech, or a hosted endpoint.
