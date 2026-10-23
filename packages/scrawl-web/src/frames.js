// Frames of the page while it records: the tab's own video (`getDisplayMedia` with
// `preferCurrentTab`), sampled every `interval` ms and whenever something happens, skipping
// samples where nothing on screen changed. Each is kept as a compressed image until the bundle
// is built, so a long recording doesn't hold hundreds of raw frames.

/** How different two 16×16 luma thumbnails must be (mean absolute difference, 0–255). */
export const CHANGE_THRESHOLD = 1.5;
const THUMB = 16;
/** How long start() waits for the tab's first frame (ms); sampling begins whenever it comes. */
export const FIRST_FRAME_WAIT = 1_000;

/** The capture size for a `width`×`height` video with its long edge at most `maxEdge`. */
export function sampleSize(width, height, maxEdge) {
  const scale = Math.min(1, maxEdge / Math.max(width, height, 1));
  return {
    width: Math.max(2, Math.round(width * scale)),
    height: Math.max(2, Math.round(height * scale)),
  };
}

/** A 16×16 luma thumbnail of RGBA pixels, for telling whether the screen changed. */
export function thumbnail(rgba, width, height) {
  const out = new Float32Array(THUMB * THUMB);
  for (let ty = 0; ty < THUMB; ty++) {
    for (let tx = 0; tx < THUMB; tx++) {
      const x = Math.min(width - 1, Math.floor(((tx + 0.5) * width) / THUMB));
      const y = Math.min(height - 1, Math.floor(((ty + 0.5) * height) / THUMB));
      const i = (y * width + x) * 4;
      out[ty * THUMB + tx] = 0.299 * rgba[i] + 0.587 * rgba[i + 1] + 0.114 * rgba[i + 2];
    }
  }
  return out;
}

/** Whether two thumbnails differ enough to keep the second frame. */
export function changed(a, b, threshold = CHANGE_THRESHOLD) {
  if (!a || !b) return true;
  let sum = 0;
  for (let i = 0; i < a.length; i++) sum += Math.abs(a[i] - b[i]);
  return sum / a.length > threshold;
}

/**
 * Samples `stream`'s video. `now()` is ms since the recording started.
 * @param {MediaStream} stream
 */
export async function sampleFrames(stream, now, { interval = 500, maxEdge = 1600, maxFrames = 300 } = {}) {
  const video = document.createElement("video");
  video.muted = true;
  video.playsInline = true;
  video.srcObject = stream;
  // `play()` resolves at the first frame, which a static tab can be slow to send (and a headless
  // browser may never send): wait a moment for it, never forever. A refusal still throws.
  const playing = video.play();
  playing.catch(() => {});
  await Promise.race([playing, new Promise((resolve) => setTimeout(resolve, FIRST_FRAME_WAIT))]);
  const frames = [];
  let last = null;
  let paused = 0;
  let pending = Promise.resolve();
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d", { willReadFrequently: true });

  const sample = (force = false) => {
    // A size can come before the picture: without current data the canvas would be blank.
    const ready = video.readyState >= video.HAVE_CURRENT_DATA && video.videoWidth;
    if (paused > 0 || !ready || frames.length >= maxFrames) return pending;
    const t = now();
    const { width, height } = sampleSize(video.videoWidth, video.videoHeight, maxEdge);
    canvas.width = width;
    canvas.height = height;
    context.drawImage(video, 0, 0, width, height);
    const thumb = thumbnail(context.getImageData(0, 0, width, height).data, width, height);
    if (!force && !changed(last, thumb)) return pending;
    last = thumb;
    // WebP where the browser can write it (Safari writes PNG instead).
    const blob = new Promise((resolve) => canvas.toBlob(resolve, "image/webp", 0.92));
    pending = pending.then(() => blob).then((b) => {
      if (b) frames.push({ t, blob: b });
    });
    return pending;
  };
  sample(true);
  // Without a frame yet, the first one is sampled when it arrives.
  if (video.readyState < video.HAVE_CURRENT_DATA) {
    video.addEventListener("loadeddata", () => sample(true), { once: true });
  }
  const timer = setInterval(() => sample(false), interval);
  return {
    /** Samples now, even when nothing changed (a click, the start of ink). */
    sample: () => sample(true),
    /** Stops sampling until `resume` (while ink is on screen, so frames show the page). */
    pause: () => {
      paused++;
    },
    resume: () => {
      paused = Math.max(0, paused - 1);
    },
    async stop() {
      clearInterval(timer);
      paused = 0;
      await sample(true);
      await pending;
      video.srcObject = null;
      return frames;
    },
  };
}
