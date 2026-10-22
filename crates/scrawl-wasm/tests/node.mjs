// Builds a bundle through the wasm package in Node, as a page would in a browser.
//
//   cargo build -p scrawl-wasm --release --target wasm32-unknown-unknown
//   wasm-bindgen --target nodejs --out-dir target/scrawl-wasm-node \
//     target/wasm32-unknown-unknown/release/scrawl_wasm.wasm
//   node crates/scrawl-wasm/tests/node.mjs target/scrawl-wasm-node
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { resolve } from "node:path";

const dir = resolve(process.argv[2] ?? "target/scrawl-wasm-node");
const { Compiler } = createRequire(import.meta.url)(`${dir}/scrawl_wasm.js`);

const width = 320;
const height = 200;
const frame = (shade) => {
  const rgba = new Uint8Array(width * height * 4).fill(shade);
  // A card that moves, so the frames differ.
  for (let y = 40; y < 120; y++) {
    for (let x = shade; x < shade + 80; x++) {
      const i = (y * width + x) * 4;
      rgba.set([30, 90, 200, 255], i);
    }
  }
  return rgba;
};

const compiler = new Compiler();
for (const [t, shade] of [[0, 20], [1200, 60], [2400, 100], [3600, 140]]) {
  compiler.addFrame(t, width, height, frame(shade));
}
assert.equal(compiler.frameCount, 4);

const capture = {
  id: "web-test",
  createdAtMs: 1_700_000_000_000,
  source: "web",
  target: {
    mode: "window",
    app: "Example page",
    windowTitle: "localhost:5173",
    displayId: 0,
    frame: { x: 0, y: 0, width, height, scale: 1 },
  },
  durationMs: 4000,
  events: [{ kind: "click", t: 1300, x: 0.4, y: 0.4 }],
  transcript: {
    source: "web-speech",
    text: "this card jumps",
    segments: [{ t0: 1000, t1: 2200, text: "this card jumps" }],
  },
  context: { route: "/cards" },
};
const started = performance.now();
const bundle = compiler.compile(JSON.stringify(capture));
const ms = Math.round(performance.now() - started);
assert.equal(compiler.frameCount, 0, "compile uses the frames up");

const names = bundle.fileNames();
assert.equal(names[0], "contact-sheet.png");
assert.deepEqual(names.slice(-2), ["scrawl.json", "summary.md"]);
assert.ok(names.some((n) => n.startsWith("frames/frame-")), names.join(", "));
const png = [0x89, 0x50, 0x4e, 0x47];
for (const name of names.filter((n) => n.endsWith(".png"))) {
  assert.deepEqual([...bundle.file(name).slice(0, 4)], png, `${name} is a PNG`);
}
assert.equal(bundle.file("nope.png"), undefined);

const manifest = JSON.parse(bundle.manifest);
assert.equal(manifest.id, "web-test");
assert.equal(manifest.source, "web");
assert.equal(manifest.durationMs, 4000);
assert.ok(manifest.keyframes.length > 0);
assert.deepEqual(JSON.parse(new TextDecoder().decode(bundle.file("scrawl.json"))), manifest);
assert.ok(bundle.summary.includes("this card jumps"), bundle.summary);
assert.equal(new TextDecoder().decode(bundle.file("summary.md")), bundle.summary);

assert.throws(() => new Compiler().compile(JSON.stringify(capture)), /at least one frame/);
const bad = new Compiler();
bad.addFrame(0, width, height, new Uint8Array(10));
assert.throws(() => bad.compile(JSON.stringify(capture)), /RGBA/);
assert.throws(() => compiler.compile("{"), /valid JSON/);

console.log(`ok: ${names.length} files, ${manifest.keyframes.length} keyframes, ${ms} ms`);
