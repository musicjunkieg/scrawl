// Node tests for scrawl-web's parts that don't need a browser, and an end-to-end compile
// through the browser build of the wasm compiler (`npm run build:wasm` first).
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { changed, sampleSize, thumbnail } from "../src/frames.js";
import { SCALE, fraction, strokeOf } from "../src/input.js";
import { compileFrames, missingTranscript, normalizeTranscript, zip } from "../src/index.js";
import { CANDIDATE_EDGE, compileSampled } from "../src/compile.js";
import { listen } from "../src/webspeech.js";

test("zip holds every file under the bundle's folder", () => {
  const files = [
    ["contact-sheet.png", new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3])],
    ["frames/frame-01.png", new Uint8Array(1000).fill(7)],
    ["summary.md", new TextEncoder().encode("[scrawl] “quotes” ok\n")],
  ];
  const dir = mkdtempSync(join(tmpdir(), "scrawl-web-zip-"));
  const path = join(dir, "b.zip");
  writeFileSync(path, zip(files, "web-1.scrawl"));
  // Python's zipfile checks every CRC and the central directory.
  const out = execFileSync("python3", ["-c", `
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
assert z.testzip() is None
for i in z.infolist(): print(i.filename, i.file_size)
print(z.read("web-1.scrawl/summary.md").decode())
`, path]).toString();
  assert.match(out, /web-1\.scrawl\/contact-sheet\.png 7/);
  assert.match(out, /web-1\.scrawl\/frames\/frame-01\.png 1000/);
  assert.match(out, /“quotes” ok/);
});

test("transcripts are clamped, sorted and marked when they're guesses", () => {
  const t = normalizeTranscript(
    { segments: [{ t0: 900, t1: 5000, text: " second " }, { t0: -5, t1: 400, text: "first" }, { t0: 1, t1: 2, text: " " }] },
    3000,
  );
  assert.deepEqual(t.segments, [
    { t0: 0, t1: 400, text: "first" },
    { t0: 900, t1: 3000, text: "second" },
  ]);
  assert.equal(t.text, "first second");
  assert.equal(t.source, "page");
  assert.equal(t.estimated, undefined);
  const textOnly = normalizeTranscript({ text: "hello there", source: "whisper" }, 2000);
  assert.deepEqual(textOnly.segments, [{ t0: 0, t1: 2000, text: "hello there" }]);
  assert.equal(textOnly.estimated, true);
  assert.equal(textOnly.source, "whisper");
  assert.ok(normalizeTranscript(null, 10).missing);
  assert.ok(normalizeTranscript({ text: "  " }, 10).missing);
  assert.deepEqual(missingTranscript("x"), { source: "none", text: "", segments: [], missing: { reason: "x" } });
});

test("ink becomes a stroke in scrawl.json's units", () => {
  assert.equal(strokeOf([{ x: 1, y: 1, t: 5 }], 100, 100), undefined);
  const s = strokeOf([{ x: 0, y: 50, t: 1000 }, { x: 100, y: 100, t: 1040 }, { x: 150, y: -3, t: 1100 }], 100, 200);
  assert.deepEqual(s, { t0: 1000, t1: 1100, fw: 100, fh: 200, pts: [0, 2500, 0, SCALE, 5000, 40, SCALE, 0, 100] });
  assert.equal(fraction(33, 100), 0.33);
});

test("frames shrink to the long edge and repeat frames are skipped", () => {
  assert.deepEqual(sampleSize(3200, 2000, 1600), { width: 1600, height: 1000 });
  assert.deepEqual(sampleSize(800, 600, 1600), { width: 800, height: 600 });
  const flat = (v) => new Uint8ClampedArray(64 * 64 * 4).fill(v);
  const a = thumbnail(flat(100), 64, 64);
  assert.equal(changed(a, thumbnail(flat(100), 64, 64)), false);
  assert.equal(changed(a, thumbnail(flat(140), 64, 64)), true);
  assert.equal(changed(null, a), true);
});

test("web speech keeps final phrases with the times they were heard", async () => {
  let clock = 0;
  let instance;
  class FakeRecognition {
    constructor() {
      instance = this;
    }
    start() {}
    stop() {
      setTimeout(() => this.onend?.(), 0);
    }
  }
  const speech = listen(() => clock, { Recognition: FakeRecognition });
  clock = 400;
  instance.onspeechstart();
  clock = 1800;
  const result = (text, isFinal) => Object.assign([{ transcript: text }], { isFinal });
  instance.onresult({ resultIndex: 0, results: [result("this button", true), result("ignored", false)] });
  clock = 2600;
  instance.onresult({ resultIndex: 1, results: [result("x", true), result(" is broken ", true)] });
  const t = await speech.stop(2500);
  assert.equal(t.source, "web-speech");
  assert.equal(t.estimated, true);
  assert.deepEqual(t.segments, [
    { t0: 400, t1: 1800, text: "this button" },
    { t0: 1800, t1: 2500, text: "is broken" },
  ]);
  const none = await listen(() => 0, { Recognition: undefined }).stop(10);
  assert.match(none.missing.reason, /no speech recognition/);
});

const wasm = new URL("../wasm/scrawl_wasm.js", import.meta.url);
test("a capture compiles into a bundle through the browser wasm", { skip: !existsSync(wasm) && "run npm run build:wasm" }, async () => {
  const { initSync, Compiler } = await import(wasm);
  initSync({ module: readFileSync(new URL("../wasm/scrawl_wasm_bg.wasm", import.meta.url)) });
  const width = 320;
  const height = 200;
  const frames = [0, 1200, 2400].map((t, i) => {
    const data = new Uint8ClampedArray(width * height * 4).fill(30 + i * 40);
    return { t, width, height, data };
  });
  const capture = {
    id: "web-test",
    source: "web",
    target: { mode: "window", app: "localhost:5173", displayId: 0, frame: { x: 0, y: 0, width, height, scale: 1 } },
    durationMs: 3000,
    events: [{ kind: "click", t: 1300, x: 0.5, y: 0.5, label: "<button> “Save”" }],
    strokes: [strokeOf([{ x: 40, y: 40, t: 2000 }, { x: 120, y: 90, t: 2100 }, { x: 200, y: 60, t: 2200 }], width, height)],
    transcript: normalizeTranscript({ segments: [{ t0: 1000, t1: 2300, text: "save does nothing" }] }, 3000),
    context: { url: "http://localhost:5173/settings" },
  };
  const built = compileFrames(Compiler, capture, frames);
  const names = built.files.map(([n]) => n);
  assert.equal(names[0], "contact-sheet.png");
  assert.deepEqual(names.slice(-2), ["scrawl.json", "summary.md"]);
  assert.equal(built.manifest.id, "web-test");
  assert.equal(built.manifest.strokes.length, 1);
  assert.equal(built.manifest.marks[0].shape, "underline");
  assert.ok(built.manifest.keyframes.some((k) => k.markId === "m1"));
  assert.match(built.summary, /save does nothing/);
  assert.match(built.summary, /url: http:\/\/localhost:5173\/settings/);
});

test("sampled frames pick keyframes small and draw them at full size", { skip: !existsSync(wasm) && "run npm run build:wasm" }, async () => {
  const { initSync, Compiler } = await import(wasm);
  initSync({ module: readFileSync(new URL("../wasm/scrawl_wasm_bg.wasm", import.meta.url)) });
  const [width, height] = [1600, 1000];
  // A card that moves every frame, so the frames differ.
  const decode = async ({ t }, maxEdge = Infinity) => {
    const scale = Math.min(1, maxEdge / width);
    const [w, h] = [Math.round(width * scale), Math.round(height * scale)];
    const data = new Uint8ClampedArray(w * h * 4).fill(240);
    const x0 = Math.round(((t / 250) * 90 * scale) % (w - 100 * scale));
    for (let y = Math.round(200 * scale); y < Math.round(500 * scale); y++) {
      for (let x = x0; x < x0 + Math.round(300 * scale); x++) data.set([30, 90, 200, 255], (y * w + x) * 4);
    }
    decoded.push(w);
    return { t, width: w, height: h, data };
  };
  const decoded = [];
  const frames = Array.from({ length: 24 }, (_, i) => ({ t: i * 250 }));
  const capture = {
    id: "sampled",
    target: { mode: "window", app: "page", displayId: 0, frame: { x: 0, y: 0, width, height, scale: 1 } },
    durationMs: 6000,
    events: [{ kind: "click", t: 2600, x: 0.5, y: 0.5 }],
  };
  const built = await compileSampled(Compiler, capture, frames, decode);
  const small = decoded.filter((w) => w === CANDIDATE_EDGE).length;
  const full = decoded.filter((w) => w === width).length;
  assert.equal(small, frames.length, "every frame once as a candidate");
  assert.equal(full, built.manifest.keyframes.length, "only the keyframes at full size");
  assert.ok(full < frames.length, `${full} of ${frames.length}`);
  for (const [name, png] of built.files.filter(([n]) => n.startsWith("frames/"))) {
    assert.equal(Buffer.from(png).readUInt32BE(16), width, `${name} is full size`);
  }
});
