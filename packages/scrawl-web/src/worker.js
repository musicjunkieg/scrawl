// The Worker that builds a bundle off the page's thread: it loads the wasm compiler once, then
// answers each `{ id, capture, frames }` with `{ id, files, summary, manifest }` or
// `{ id, error }`.

import init, { Compiler } from "../wasm/scrawl_wasm.js";
import { compileSampled, decodeFrame } from "./compile.js";

const ready = init();

self.onmessage = async ({ data: { id, capture, frames } }) => {
  try {
    await ready;
    // Candidates small, then only the keyframes at full size: RGBA is 4 bytes a pixel, far more
    // than the WebP blobs.
    const built = await compileSampled(Compiler, capture, frames, decodeFrame);
    self.postMessage({ id, ...built }, built.files.map(([, bytes]) => bytes.buffer));
  } catch (error) {
    self.postMessage({ id, error: String(error?.message || error) });
  }
};
