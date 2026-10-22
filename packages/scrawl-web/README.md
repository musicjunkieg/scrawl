# scrawl-web

Record your own web page and hand an agent a `.scrawl` bundle: the page as it looked, where
you clicked, what you circled with ⇧ ink, and what you said. The bundle is built in the
browser by Scrawl's compiler (Rust, in wasm, in a Worker). Nothing leaves the page unless
your page sends it.

```js
import { createScrawl } from "scrawl-web";

const scrawl = createScrawl({
  // Your transcriber: the recorded audio in, text with times (ms) out.
  transcribe: async (audio) => {
    const res = await fetch("/api/transcribe", { method: "POST", body: audio });
    return res.json(); // { text, segments: [{ t0, t1, text }] }
  },
});

await scrawl.start(); // the browser asks once to share this tab (and the microphone)
// … click, hold ⇧ and move the pointer to draw, talk …
const bundle = await scrawl.stop();
bundle.summary;  // summary.md: hand this to the agent
bundle.zip();    // a Blob: <id>.scrawl/ with contact-sheet.png, frames/, scrawl.json, summary.md
```

## Transcription

- **Your `transcribe` function** (the default path): Scrawl records the microphone and calls
  it with the audio when you stop. Use your own server (Whisper, Deepgram, OpenAI) or run
  Whisper in the page. Segment times put each sentence next to the frame it was said over.
- **`transcribe: "web-speech"`**: the browser's own recognition, live while recording.
  Chrome and Edge send the audio to their speech service, Safari needs Siri or Dictation on,
  and Firefox has none. It gives no word times, so the times are when each phrase was heard
  and the transcript is marked `estimated`.
- **Nothing**: no microphone is used, and the bundle says there's no transcript.

## What it records

- **Frames:** the tab's own video (`getDisplayMedia` with `preferCurrentTab`), every
  `interval` ms (500) and right after each click, skipping frames where nothing changed. The
  long edge is at most `maxEdge` px (1600).
- **Clicks** with a short name for what was clicked (an `aria-label`, title or the first 60
  characters of text; form fields give only a placeholder or name, never what was typed).
- **⇧ ink:** hold ⇧ and move the pointer. The ink shows while it's drawn and fades out, and no
  frame is taken while it's on screen, so Scrawl draws it once, from the data.
- **Scrolls**, and the page's URL, title and `context`.
- **With `agentContext: true`, what only the page knows**, as Scrawl's `sdk.js` gives the Mac
  app:
  - the element under each click and ⇧ mark, with its id, two classes, and the component and
    source `file:line` from React's dev build or a locator attribute (`data-source-loc`,
    `data-inspector-relative-path`, `data-sentry-source-file`): `clicked <button#save> “Save” in
    Toolbar (src/Toolbar.tsx:42)`;
  - `console.error`, uncaught errors and unhandled rejections, with the first frame in the
    page's own code;
  - `fetch` and XHR requests that fail or answer 400+: method, path and status, never the query,
    body, headers or cookies;
  - `<video>`/`<audio>` seeks, plays and pauses.

  It wraps `console.error`, `fetch` and XHR while recording and puts them back on `stop()`.

## Building and testing

`npm run build:wasm` builds `crates/scrawl-wasm` into `wasm/` (it needs the
`wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the lockfile's `wasm-bindgen`
version). `npm test` runs the Node tests. `node test/browser/run.mjs [out-dir]` records
`test/browser/index.html` in Chromium through Playwright (tab capture, a click, ⇧ ink, a fake
transcriber) and writes the bundle.
