// Builds the bundle with the wasm compiler (`crates/scrawl-wasm`), in a Worker or anywhere.

/**
 * Compiles `capture` (`scrawl.json`'s fields) from RGBA `frames` with `Compiler` from the
 * wasm package.
 * @param {{ new(): any }} Compiler
 * @param {object} capture
 * @param {Array<{ t: number, width: number, height: number, data: Uint8Array | Uint8ClampedArray }>} frames
 */
export function compileFrames(Compiler, capture, frames) {
  const compiler = new Compiler();
  try {
    for (const f of frames) {
      compiler.addFrame(f.t, f.width, f.height, f.data instanceof Uint8Array ? f.data : new Uint8Array(f.data.buffer, f.data.byteOffset, f.data.byteLength));
    }
    const bundle = compiler.compile(JSON.stringify(capture));
    try {
      const files = bundle.fileNames().map((name) => [name, bundle.file(name)]);
      return { files, summary: bundle.summary, manifest: JSON.parse(bundle.manifest) };
    } finally {
      bundle.free?.();
    }
  } finally {
    compiler.free?.();
  }
}

/** Decodes a sampled frame's image back to RGBA pixels (in a page or a Worker). */
export async function decodeFrame({ t, blob }) {
  const bitmap = await createImageBitmap(blob);
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  const context = canvas.getContext("2d", { willReadFrequently: true });
  context.drawImage(bitmap, 0, 0);
  bitmap.close?.();
  const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
  return { t, width: canvas.width, height: canvas.height, data };
}
