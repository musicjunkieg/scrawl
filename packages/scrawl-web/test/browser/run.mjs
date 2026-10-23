// Records test/browser/index.html in Chromium through scrawl-web: tab capture, a click,
// ⇧ ink, a fake transcriber. Writes the bundle to the folder given (default: a temp dir) and
// checks it. Needs `npm run build:wasm` and Playwright (`PLAYWRIGHT_BROWSERS_PATH` set).
//   node test/browser/run.mjs [out-dir]
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };
const server = createServer((req, res) => {
  const path = join(root, decodeURIComponent(new URL(req.url, "http://x").pathname));
  let body;
  try {
    body = readFileSync(path);
  } catch {
    return res.writeHead(404).end();
  }
  res.writeHead(200, { "Content-Type": types[extname(path)] || "application/octet-stream" });
  res.end(body);
}).listen(0, "127.0.0.1");
await new Promise((r) => server.once("listening", r));
const base = `http://127.0.0.1:${server.address().port}`;
// Another origin, for a no-cors request that succeeds with an opaque response.
const other = createServer((req, res) => res.end("ok")).listen(0, "127.0.0.1");
await new Promise((r) => other.once("listening", r));
const otherOrigin = `http://127.0.0.1:${other.address().port}`;

// A step that hangs fails the run instead of holding CI until its job limit.
let step = "launching Chromium";
const watchdog = setTimeout(() => {
  console.error(`timed out after 120 s while ${step}`);
  process.exit(1);
}, 120_000);
const at = (name) => {
  step = name;
  console.log(`· ${name}`);
};

const browser = await chromium.launch({
  args: ["--auto-accept-this-tab-capture", "--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
});
try {
  const page = await browser.newPage({ viewport: { width: 900, height: 560 } });
  page.on("console", (m) => {
    if (m.type() === "error") console.error("page:", m.text());
    else if (m.type() === "info" && m.text().startsWith("step:")) console.log(`  ${m.text()}`);
  });
  await page.goto(`${base}/test/browser/index.html`);
  at("starting a recording");
  await page.evaluate(async () => {
    // Where start() waits, in the job log, if it ever hangs again.
    const media = navigator.mediaDevices;
    for (const name of ["getDisplayMedia", "getUserMedia"]) {
      const ask = media[name].bind(media);
      media[name] = async (...args) => {
        console.info(`step: ${name}`);
        const stream = await ask(...args);
        console.info(`step: ${name} answered`);
        return stream;
      };
    }
    const { createScrawl } = await import("/src/index.js");
    window.originals = { fetch: window.fetch, error: console.error };
    window.scrawl = createScrawl({
      interval: 250,
      agentContext: true,
      context: { route: "/cards" },
      transcribe: async (audio, { durationMs }) => ({
        source: "test",
        segments: [{ t0: 200, t1: Math.min(durationMs, 1800), text: `the card should move (${audio.size > 0 ? "audio" : "no audio"})` }],
      }),
    });
    await window.scrawl.start();
    console.info("step: started");
  });
  await page.waitForTimeout(700);
  await page.click("#move");
  // What only the page knows: an error it logs and a request that fails.
  at("logging an error and making requests");
  await page.evaluate(async (otherOrigin) => {
    console.error("card failed to save");
    await fetch("/api/cards?token=secret").catch(() => {});
    // An opaque no-cors response that worked is not a failure.
    await fetch(`${otherOrigin}/ping`, { mode: "no-cors" }).catch(() => {});
    // A live stream reports its duration as Infinity; no real playback, which a CI runner may
    // never start.
    const video = document.createElement("video");
    Object.defineProperty(video, "duration", { value: Infinity });
    document.body.append(video);
    video.dispatchEvent(new Event("pause"));
  }, otherOrigin);
  at("clicking inside a shadow root");
  // A button inside an open shadow root is named itself, not by its host.
  await page.evaluate(() => {
    const host = document.createElement("div");
    host.id = "host";
    document.body.append(host);
    host.attachShadow({ mode: "open" }).innerHTML = `<button aria-label="Inner">x</button>`;
  });
  await page.click("#host >> button");
  await page.waitForTimeout(900);
  // ⇧ ink: hold Shift and move the pointer around the card.
  await page.keyboard.down("Shift");
  for (const [x, y] of [[300, 80], [420, 90], [540, 110], [560, 200], [430, 230]]) {
    await page.mouse.move(x, y, { steps: 6 });
  }
  await page.keyboard.up("Shift");
  await page.waitForTimeout(900);
  const started = Date.now();
  at("stopping and building the bundle");
  const bundle = await page.evaluate(async () => {
    const b = await window.scrawl.stop();
    const toBase64 = (bytes) => btoa(Array.from(bytes, (c) => String.fromCharCode(c)).join(""));
    return { id: b.id, summary: b.summary, manifest: b.manifest, files: b.files.map(([n, bytes]) => [n, toBase64(bytes)]), zip: toBase64(new Uint8Array(await b.zip().arrayBuffer())) };
  });
  const ms = Date.now() - started;
  const out = process.argv[2] ? resolve(process.argv[2]) : mkdtempSync(join(tmpdir(), "scrawl-web-"));
  for (const [name, b64] of bundle.files) {
    mkdirSync(dirname(join(out, name)), { recursive: true });
    writeFileSync(join(out, name), Buffer.from(b64, "base64"));
  }
  writeFileSync(join(out, `${bundle.id}.scrawl.zip`), Buffer.from(bundle.zip, "base64"));

  const m = bundle.manifest;
  assert.equal(m.source, "web");
  assert.ok(m.keyframes.length >= 1, "keyframes");
  assert.ok(m.events.some((e) => e.kind === "click" && /Move the card/.test(e.label || "")), JSON.stringify(m.events));
  assert.equal(m.strokes.length, 1, "one ink stroke");
  assert.match(bundle.summary, /the card should move \(audio\)/);
  // agentContext: the click names its source, and the error and failed request are events.
  const labels = m.events.map((e) => e.label).filter(Boolean);
  assert.ok(labels.includes("<button#move> “Move the card” (src/Card.tsx:12)"), JSON.stringify(labels));
  assert.ok(labels.some((l) => /^console error: card failed to save \(test\/browser\/run|^console error: card failed to save$/.test(l)), JSON.stringify(labels));
  assert.ok(labels.includes("GET /api/cards → 404"), JSON.stringify(labels));
  assert.ok(!labels.some((l) => l.includes("→ failed") || l.includes("→ 0")), JSON.stringify(labels));
  assert.ok(labels.some((l) => /^video .*paused at \d+:\d\d$/.test(l)), JSON.stringify(labels));
  assert.ok(labels.includes("<button> “Inner”"), JSON.stringify(labels));
  assert.ok(!JSON.stringify(m).includes("secret"), "no query strings");
  assert.equal(m.strokes[0].on, "<div#card.card.moved> “A card”", JSON.stringify(m.strokes[0]));
  const restored = await page.evaluate(() => window.fetch === window.originals.fetch && console.error === window.originals.error);
  assert.ok(restored, "fetch and console.error are put back");
  assert.match(bundle.summary, /route: \/cards/);
  // A refused microphone: start() fails, and the tab capture and ink overlay are gone.
  at("checking a refused microphone");
  const refused = await page.evaluate(async () => {
    const { createScrawl } = await import("/src/index.js");
    const media = navigator.mediaDevices;
    const real = { getDisplayMedia: media.getDisplayMedia, getUserMedia: media.getUserMedia };
    let display;
    media.getDisplayMedia = async (c) => (display = await real.getDisplayMedia.call(media, c));
    media.getUserMedia = async () => {
      throw new DOMException("Permission denied", "NotAllowedError");
    };
    const scrawl = createScrawl({ transcribe: async () => ({ text: "x" }) });
    const error = await scrawl.start().then(() => null, (e) => e.name);
    Object.assign(media, real); // the checks below record on this tab too
    return {
      error,
      recording: scrawl.recording,
      live: display.getTracks().some((t) => t.readyState === "live"),
      overlay: Boolean(document.querySelector("[data-scrawl-ink]")),
    };
  });
  assert.deepEqual(refused, { error: "NotAllowedError", recording: false, live: false, overlay: false });

  // A second start() while the first is still asking for the tab is refused, and a worker
  // that can't load makes stop() fail instead of hang. Every check records this same tab: on
  // CI's headless shell a second tab's capture never delivers a frame.
  at("checking a second start() and a missing worker");
  const edge = await page.evaluate(async () => {
    const { createScrawl } = await import("/src/index.js");
    const scrawl = createScrawl({ worker: "/missing-worker.js", interval: 250 });
    const fetchBefore = window.fetch;
    const first = scrawl.start();
    const second = await scrawl.start().then(() => null, (e) => e.message);
    await first;
    // Without agentContext the page's fetch and console are left alone.
    if (window.fetch !== fetchBefore) return { second, stopped: "fetch was wrapped", recording: true };
    await new Promise((r) => setTimeout(r, 400));
    const timeout = new Promise((r) => setTimeout(() => r("hung"), 5000));
    const stopped = await Promise.race([scrawl.stop().then(() => "resolved", (e) => e.message), timeout]);
    return { second, stopped, recording: scrawl.recording };
  });
  assert.equal(edge.second, "Already recording");
  assert.match(edge.stopped, /worker failed/, JSON.stringify(edge));
  assert.equal(edge.recording, false);
  // A recorder that can't start turns the microphone back off.
  at("checking a recorder that cannot start");
  const mic = await page.evaluate(async () => {
    const { createScrawl } = await import("/src/index.js");
    const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    let stream;
    navigator.mediaDevices.getUserMedia = async (c) => (stream = await getUserMedia(c));
    window.MediaRecorder = class {
      start() {
        throw new DOMException("No encoder", "NotSupportedError");
      }
    };
    const scrawl = createScrawl({ transcribe: async () => ({ text: "x" }) });
    const error = await scrawl.start().then(() => null, (e) => e.name);
    return { error, live: stream.getTracks().some((t) => t.readyState === "live") };
  });
  assert.deepEqual(mic, { error: "NotSupportedError", live: false });
  // Two recordings at once give the page its own fetch, console.error and XHR back, whichever
  // stops first.
  at("checking two recordings at once");
  const unwrapped = await page.evaluate(async () => {
    const { watchPage } = await import("/src/context.js");
    const original = [window.fetch, console.error, XMLHttpRequest.prototype.open, XMLHttpRequest.prototype.send];
    const same = () => [window.fetch, console.error, XMLHttpRequest.prototype.open, XMLHttpRequest.prototype.send].every((f, i) => f === original[i]);
    const results = [];
    for (const order of [[0, 1], [1, 0]]) {
      const watchers = [watchPage(Date.now), watchPage(Date.now)];
      const wrapped = !same();
      for (const i of order) watchers[i].stop();
      results.push(wrapped && same());
    }
    return results;
  });
  assert.deepEqual(unwrapped, [true, true], "both stop orders put the originals back");
  // Editable text is named by its label, never by what was typed in it.
  at("checking editable text");
  const named = await page.evaluate(async () => {
    const { describe } = await import("/src/input.js");
    document.body.insertAdjacentHTML(
      "beforeend",
      `<section role="region"><div contenteditable><b id="typed">secret words</b></div></section>
       <div contenteditable aria-label="Notes"><i id="noted">secret</i></div>
       <div id="tb" role="textbox">also secret</div>`,
    );
    return ["typed", "noted", "tb"].map((id) => describe(document.getElementById(id)));
  });
  assert.deepEqual(named, ["<div>", "<div> “Notes”", "<div>"]);

  // A stream with no frame yet (a static tab can be slow to send one, and a headless browser may
  // never) doesn't hold up start(): sampling begins with the first frame that does arrive.
  at("checking a stream that has no frame yet");
  const late = await page.evaluate(async () => {
    const { sampleFrames } = await import("/src/frames.js");
    const canvas = Object.assign(document.createElement("canvas"), { width: 64, height: 40 });
    const stream = canvas.captureStream();
    const t0 = performance.now();
    // A long interval, so only the first frame's own sample and stop()'s can add frames.
    const frames = await sampleFrames(stream, () => Math.round(performance.now() - t0), { interval: 60_000 });
    const waited = Math.round(performance.now() - t0);
    const context = canvas.getContext("2d");
    context.fillStyle = "red";
    context.fillRect(0, 0, 64, 40);
    await new Promise((resolve) => setTimeout(resolve, 600));
    const sampled = await frames.stop();
    stream.getTracks().forEach((track) => track.stop());
    return { started: waited < 5_000, sampled: sampled.length };
  });
  assert.deepEqual(late, { started: true, sampled: 2 }, "the first frame is sampled when it arrives, and again at stop()");
  // A play() refused after that wait is kept, for stop() to report instead of "no frames".
  at("checking a play() refused late");
  const refusedLate = await page.evaluate(async () => {
    const { sampleFrames, FIRST_FRAME_WAIT } = await import("/src/frames.js");
    const play = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = () =>
      new Promise((_, reject) => setTimeout(() => reject(new DOMException("no", "NotAllowedError")), FIRST_FRAME_WAIT + 200));
    try {
      const canvas = Object.assign(document.createElement("canvas"), { width: 64, height: 40 });
      const stream = canvas.captureStream();
      const frames = await sampleFrames(stream, () => 0, { interval: 60_000 });
      await new Promise((resolve) => setTimeout(resolve, 400));
      const sampled = await frames.stop();
      stream.getTracks().forEach((track) => track.stop());
      return { failure: frames.failure?.name, sampled: sampled.length };
    } finally {
      HTMLMediaElement.prototype.play = play;
    }
  });
  assert.deepEqual(refusedLate, { failure: "NotAllowedError", sampled: 0 });
  // Through stop(): it fails with that reason, and leaves no tab capture or Web Speech running.
  at("checking a recording whose play() is refused late");
  const failedLate = await page.evaluate(async () => {
    const { createScrawl } = await import("/src/index.js");
    const { FIRST_FRAME_WAIT } = await import("/src/frames.js");
    let listening = 0;
    window.SpeechRecognition = class {
      start() {
        listening++;
      }
      stop() {
        listening--;
        setTimeout(() => this.onend?.());
      }
    };
    // A tab that never sends a frame, so the refusal is all there is.
    const display = Object.assign(document.createElement("canvas"), { width: 64, height: 40 }).captureStream();
    const media = navigator.mediaDevices;
    const getDisplayMedia = media.getDisplayMedia;
    media.getDisplayMedia = async () => display;
    const play = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = () =>
      new Promise((_, reject) => setTimeout(() => reject(new DOMException("no", "NotAllowedError")), FIRST_FRAME_WAIT + 200));
    try {
      const scrawl = createScrawl({ transcribe: "web-speech" });
      await scrawl.start();
      await new Promise((resolve) => setTimeout(resolve, 400));
      const error = await scrawl.stop().then(() => null, (e) => e.name);
      return { error, listening, live: display.getTracks().some((t) => t.readyState === "live") };
    } finally {
      HTMLMediaElement.prototype.play = play;
      media.getDisplayMedia = getDisplayMedia;
      delete window.SpeechRecognition;
    }
  });
  assert.deepEqual(failedLate, { error: "NotAllowedError", listening: 0, live: false });

  clearTimeout(watchdog);
  console.log(`ok: ${bundle.files.length} files, ${m.keyframes.length} keyframes, ${m.marks.length} marks, built in ${ms} ms → ${out}`);
  console.log(bundle.summary);
} finally {
  await browser.close();
  server.close();
  other.close();
}
