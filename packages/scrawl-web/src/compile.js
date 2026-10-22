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
    for (const f of frames) addFrame(compiler, f);
    return collect(compiler, capture);
  } finally {
    compiler.free?.();
  }
}

/** The candidates' longest side (px), as the Mac app's: they only feed keyframe selection. */
export const CANDIDATE_EDGE = 480;

/**
 * Compiles `capture` from sampled `frames` (`{ t, blob }`) without holding them all at full
 * size: each is decoded at `CANDIDATE_EDGE` px to pick the keyframes, then only the keyframes
 * are decoded again at full size for the bundle's images. `decode(frame, maxEdge?)` gives RGBA
 * pixels (`decodeFrame`, in a page or Worker).
 * @param {{ new(): any }} Compiler
 * @param {object} capture
 * @param {Array<{ t: number }>} frames
 * @param {(frame: any, maxEdge?: number) => Promise<{ t: number, width: number, height: number, data: Uint8Array | Uint8ClampedArray }>} decode
 */
export async function compileSampled(Compiler, capture, frames, decode) {
  const compiler = new Compiler();
  try {
    for (const frame of frames) addFrame(compiler, await decode(frame, CANDIDATE_EDGE));
    for (const t of compiler.keyframeTimes(JSON.stringify(capture))) {
      const frame = nearest(frames, t);
      if (frame) {
        const full = await decode(frame);
        compiler.addFullFrame(t, full.width, full.height, bytes(full.data));
      }
    }
    return collect(compiler, capture);
  } finally {
    compiler.free?.();
  }
}

function nearest(frames, t) {
  let best = null;
  for (const f of frames) if (!best || Math.abs(f.t - t) < Math.abs(best.t - t)) best = f;
  return best;
}

const bytes = (data) => (data instanceof Uint8Array ? data : new Uint8Array(data.buffer, data.byteOffset, data.byteLength));

function addFrame(compiler, f) {
  compiler.addFrame(f.t, f.width, f.height, bytes(f.data));
}

function collect(compiler, capture) {
  const bundle = compiler.compile(JSON.stringify(capture));
  try {
    const files = bundle.fileNames().map((name) => [name, bundle.file(name)]);
    return { files, summary: bundle.summary, manifest: JSON.parse(bundle.manifest) };
  } finally {
    bundle.free?.();
  }
}

/**
 * Decodes a sampled frame's image back to RGBA pixels (in a page or a Worker), scaled down to
 * at most `maxEdge` px on its longest side when given.
 */
export async function decodeFrame({ t, blob }, maxEdge = Infinity) {
  const bitmap = await createImageBitmap(blob);
  const scale = Math.min(1, maxEdge / Math.max(bitmap.width, bitmap.height));
  const canvas = new OffscreenCanvas(Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale)));
  const context = canvas.getContext("2d", { willReadFrequently: true });
  context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close?.();
  const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
  return { t, width: canvas.width, height: canvas.height, data };
}
