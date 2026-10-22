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

const browser = await chromium.launch({
  args: ["--auto-accept-this-tab-capture", "--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
});
try {
  const page = await browser.newPage({ viewport: { width: 900, height: 560 } });
  page.on("console", (m) => m.type() === "error" && console.error("page:", m.text()));
  await page.goto(`${base}/test/browser/index.html`);
  await page.evaluate(async () => {
    const { createScrawl } = await import("/src/index.js");
    window.scrawl = createScrawl({
      interval: 250,
      context: { route: "/cards" },
      transcribe: async (audio, { durationMs }) => ({
        source: "test",
        segments: [{ t0: 200, t1: Math.min(durationMs, 1800), text: `the card should move (${audio.size > 0 ? "audio" : "no audio"})` }],
      }),
    });
    await window.scrawl.start();
  });
  await page.waitForTimeout(700);
  await page.click("#move");
  await page.waitForTimeout(900);
  // ⇧ ink: hold Shift and move the pointer around the card.
  await page.keyboard.down("Shift");
  for (const [x, y] of [[300, 80], [420, 90], [540, 110], [560, 200], [430, 230]]) {
    await page.mouse.move(x, y, { steps: 6 });
  }
  await page.keyboard.up("Shift");
  await page.waitForTimeout(900);
  const started = Date.now();
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
  assert.match(bundle.summary, /route: \/cards/);
  // A refused microphone: start() fails, and the tab capture and ink overlay are gone.
  const refused = await page.evaluate(async () => {
    const { createScrawl } = await import("/src/index.js");
    const getDisplayMedia = navigator.mediaDevices.getDisplayMedia.bind(navigator.mediaDevices);
    let display;
    navigator.mediaDevices.getDisplayMedia = async (c) => (display = await getDisplayMedia(c));
    navigator.mediaDevices.getUserMedia = async () => {
      throw new DOMException("Permission denied", "NotAllowedError");
    };
    const scrawl = createScrawl({ transcribe: async () => ({ text: "x" }) });
    const error = await scrawl.start().then(() => null, (e) => e.name);
    return {
      error,
      recording: scrawl.recording,
      live: display.getTracks().some((t) => t.readyState === "live"),
      overlay: Boolean(document.querySelector("[data-scrawl-ink]")),
    };
  });
  assert.deepEqual(refused, { error: "NotAllowedError", recording: false, live: false, overlay: false });

  // A second start() while the first is still asking for the tab is refused, and a worker
  // that can't load makes stop() fail instead of hang.
  const fresh = await browser.newPage();
  await fresh.goto(`${base}/test/browser/index.html`);
  const edge = await fresh.evaluate(async () => {
    const { createScrawl } = await import("/src/index.js");
    const scrawl = createScrawl({ worker: "/missing-worker.js", interval: 250 });
    const first = scrawl.start();
    const second = await scrawl.start().then(() => null, (e) => e.message);
    await first;
    await new Promise((r) => setTimeout(r, 400));
    const timeout = new Promise((r) => setTimeout(() => r("hung"), 5000));
    const stopped = await Promise.race([scrawl.stop().then(() => "resolved", (e) => e.message), timeout]);
    return { second, stopped, recording: scrawl.recording };
  });
  assert.equal(edge.second, "Already recording");
  assert.match(edge.stopped, /worker failed/, JSON.stringify(edge));
  assert.equal(edge.recording, false);
  // A recorder that can't start turns the microphone back off.
  const mic = await fresh.evaluate(async () => {
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
  // Editable text is named by its label, never by what was typed in it.
  const named = await fresh.evaluate(async () => {
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

  console.log(`ok: ${bundle.files.length} files, ${m.keyframes.length} keyframes, ${m.marks.length} marks, built in ${ms} ms → ${out}`);
  console.log(bundle.summary);
} finally {
  await browser.close();
  server.close();
}
