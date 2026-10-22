// The Worker that builds a bundle off the page's thread: it loads the wasm compiler once, then
// answers each `{ id, capture, frames }` with `{ id, files, summary, manifest }` or
// `{ id, error }`.

import init, { Compiler } from "../wasm/scrawl_wasm.js";
import { compileStreaming, decodeFrame } from "./compile.js";

const ready = init();

self.onmessage = async ({ data: { id, capture, frames } }) => {
  try {
    await ready;
    // One decoded frame at a time: RGBA is 4 bytes a pixel, far more than the WebP blobs.
    const built = await compileStreaming(Compiler, capture, async (add) => {
      for (const frame of frames) add(await decodeFrame(frame));
    });
    self.postMessage({ id, ...built }, built.files.map(([, bytes]) => bytes.buffer));
  } catch (error) {
    self.postMessage({ id, error: String(error?.message || error) });
  }
};
