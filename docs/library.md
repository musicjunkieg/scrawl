# Scrawl as a library: plan

Status: proposal, no code changed yet. Written 2026-10-19.

Goal: any app can add Scrawl's capture-and-compile loop as a dependency, in three shapes:

1. **Rust crate** for Rust apps and tools.
2. **Swift package** for iOS (exists: ScrawlKit) and macOS (new).
3. **npm package** for web apps.

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
