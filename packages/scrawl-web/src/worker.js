// The Worker that builds a bundle off the page's thread: it loads the wasm compiler once, then
// answers each `{ id, capture, frames }` with `{ id, files, summary, manifest }` or
// `{ id, error }`.

import init, { Compiler } from "../wasm/scrawl_wasm.js";
import { compileFrames, decodeFrame } from "./compile.js";

const ready = init();

self.onmessage = async ({ data: { id, capture, frames } }) => {
  try {
    await ready;
    const decoded = [];
    for (const frame of frames) decoded.push(await decodeFrame(frame));
    const built = compileFrames(Compiler, capture, decoded);
    self.postMessage({ id, ...built }, built.files.map(([, bytes]) => bytes.buffer));
  } catch (error) {
    self.postMessage({ id, error: String(error?.message || error) });
  }
};
