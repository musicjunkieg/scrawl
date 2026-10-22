// @scrawl/web: record your own page and hand an agent a `.scrawl` bundle. The tab's video
// (`getDisplayMedia` with `preferCurrentTab`, so the browser asks once), clicks, scrolls, ⇧ ink
// and voice go in; the bundle (`contact-sheet.png`, `frames/`, `scrawl.json`, `summary.md`)
// comes out, built in a Worker by Scrawl's compiler in wasm. Nothing leaves the page unless the
// page sends it.
//
//   const scrawl = createScrawl({ transcribe: async (audio) => ({ text, segments }) });
//   await scrawl.start();
//   const bundle = await scrawl.stop();
//   download(bundle.zip(), `${bundle.id}.scrawl.zip`);

import { sampleFrames } from "./frames.js";
import { recordInput } from "./input.js";
import { missingTranscript, normalizeTranscript } from "./transcript.js";
import { listen } from "./webspeech.js";
import { zip } from "./zip.js";

export { compileFrames, decodeFrame } from "./compile.js";
export { missingTranscript, normalizeTranscript } from "./transcript.js";
export { zip } from "./zip.js";

/**
 * @param {object} [options]
 * @param {((audio: Blob, info: { durationMs: number }) => Promise<object>) | "web-speech"} [options.transcribe]
 *   Your transcriber (the recorded audio in, `{ text, segments: [{ t0, t1, text }] }` out, ms),
 *   or `"web-speech"` for the browser's live recognition. Without one the bundle has no
 *   transcript and no microphone is used.
 * @param {string} [options.lang] Web Speech's language (`"en-US"`).
 * @param {string} [options.note] A note for `summary.md`.
 * @param {Record<string, string>} [options.context] What the page knows about itself (route, build).
 * @param {boolean} [options.agentContext] Scrawl's `agentContext` experiment.
 * @param {number} [options.interval] Ms between frame samples (500).
 * @param {number} [options.maxEdge] Frames' longest side in px (1600).
 * @param {boolean} [options.showInk] Draw ⇧ ink while it's drawn (true).
 * @param {URL | string} [options.worker] Where `worker.js` is served from.
 */
export function createScrawl(options = {}) {
  let session = null;
  let starting = false;
  return {
    get recording() {
      return session !== null;
    },
    async start() {
      if (session || starting) throw new Error("Already recording");
      starting = true;
      try {
        session = await begin(options);
      } finally {
        starting = false;
      }
    },
    async stop() {
      if (!session) throw new Error("Not recording");
      const running = session;
      session = null;
      return finish(running, options);
    },
  };
}

async function begin(options) {
  const display = await navigator.mediaDevices.getDisplayMedia({
    video: { displaySurface: "browser" },
    audio: false,
    preferCurrentTab: true,
    selfBrowserSurface: "include",
  });
  const started = performance.now();
  const now = () => Math.round(performance.now() - started);
  const createdAtMs = Date.now();
  let frames = null;
  let input = null;
  try {
    return await record(display, now, createdAtMs, options, (f, i) => {
      frames = f;
      input = i;
    });
  } catch (error) {
    // A refused microphone, say: stop what already started.
    input?.stop();
    await frames?.stop();
    display.getTracks().forEach((track) => track.stop());
    throw error;
  }
}

async function record(display, now, createdAtMs, options, started) {
  const frames = await sampleFrames(display, now, options);
  started(frames, null);
  const input = recordInput(now, {
    showInk: options.showInk ?? true,
    onAction: () => setTimeout(() => frames.sample(), 150),
    onInk: (drawing) => {
      if (drawing) {
        frames.sample();
        frames.pause();
      } else {
        frames.resume();
        frames.sample();
      }
    },
  });
  started(frames, input);
  let audio = null;
  let speech = null;
  if (options.transcribe === "web-speech") {
    speech = listen(now, { lang: options.lang });
  } else if (typeof options.transcribe === "function") {
    audio = await recordAudio();
  }
  return { display, now, createdAtMs, frames, input, audio, speech };
}

async function recordAudio() {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const chunks = [];
  let recorder;
  try {
    recorder = new MediaRecorder(stream);
    recorder.ondataavailable = (e) => e.data.size && chunks.push(e.data);
    recorder.start(1000);
  } catch (error) {
    // A recorder that can't start must not leave the microphone on.
    stream.getTracks().forEach((track) => track.stop());
    throw error;
  }
  return {
    stop: () =>
      new Promise((resolve) => {
        recorder.onstop = () => {
          stream.getTracks().forEach((track) => track.stop());
          resolve(new Blob(chunks, { type: recorder.mimeType }));
        };
        recorder.stop();
      }),
  };
}

async function finish(session, options) {
  const durationMs = Math.max(1, session.now());
  const { events, strokes } = session.input.stop();
  const frames = await session.frames.stop();
  session.display.getTracks().forEach((track) => track.stop());
  const audioBlob = session.audio ? await session.audio.stop() : null;

  let transcript;
  if (session.speech) {
    transcript = await session.speech.stop(durationMs);
  } else if (audioBlob) {
    try {
      transcript = normalizeTranscript(await options.transcribe(audioBlob, { durationMs }), durationMs);
    } catch (error) {
      transcript = missingTranscript(`Transcription failed: ${error?.message || error}`);
    }
  } else {
    transcript = missingTranscript("No transcriber was set up.");
  }

  const id = `scrawl-${new Date(session.createdAtMs).toISOString().replace(/[-:]/g, "").replace(/\..*/, "")}`;
  const capture = {
    id,
    createdAtMs: session.createdAtMs,
    source: "web",
    target: {
      mode: "window",
      app: location.host || "page",
      windowTitle: document.title || undefined,
      displayId: 0,
      frame: { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight, scale: window.devicePixelRatio || 1 },
    },
    durationMs,
    strokes,
    events,
    transcript,
    note: options.note,
    context: { url: location.href, ...(options.context || {}) },
    agentContext: Boolean(options.agentContext),
  };
  const built = await compileInWorker(options.worker, capture, frames);
  return bundleOf(id, built, audioBlob);
}

// One Worker per script URL, kept between recordings so the wasm loads once.
const workers = new Map();
let nextId = 0;
function compileInWorker(url, capture, frames) {
  const href = String(url ?? new URL("./worker.js", import.meta.url));
  if (!workers.has(href)) workers.set(href, new Worker(href, { type: "module" }));
  const current = workers.get(href);
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const done = () => {
      current.removeEventListener("message", onMessage);
      current.removeEventListener("error", onError);
    };
    const onMessage = ({ data }) => {
      if (data.id !== id) return;
      done();
      data.error ? reject(new Error(data.error)) : resolve(data);
    };
    // The script failed to load or threw outside a message: drop this Worker so the next
    // stop() starts a fresh one instead of waiting on a dead one.
    const onError = (event) => {
      done();
      current.terminate();
      if (workers.get(href) === current) workers.delete(href);
      reject(new Error(`Scrawl's worker failed: ${event.message || "it could not be loaded"}`));
    };
    current.addEventListener("message", onMessage);
    current.addEventListener("error", onError);
    current.postMessage({ id, capture, frames });
  });
}

/** What `stop()` hands back. */
function bundleOf(id, { files, summary, manifest }, audio) {
  return {
    id,
    /** `[path, bytes]` for every file, `summary.md` last. */
    files,
    summary,
    manifest,
    /** The recorded voice (when your `transcribe` was used), to keep beside the bundle. */
    audio,
    file: (name) => files.find(([n]) => n === name)?.[1],
    /** The bundle as one `.zip` (a `<id>.scrawl/` folder inside). */
    zip: () => new Blob([zip(files, `${id}.scrawl`)], { type: "application/zip" }),
  };
}
