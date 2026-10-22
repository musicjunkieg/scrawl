// The opt-in `transcribe: "web-speech"`: the browser's own speech recognition (Chrome and
// Edge send the audio to their speech service; Safari needs Siri or Dictation on; Firefox
// has none). It listens live while recording, since it can't take a recording afterwards, and
// it reports phrases without word times, so the times are when each phrase was heard and the
// transcript is marked `estimated`.

import { missingTranscript } from "./transcript.js";

/** The browser's `SpeechRecognition`, or `undefined`. */
export function speechRecognition(scope = globalThis) {
  return scope.SpeechRecognition || scope.webkitSpeechRecognition;
}

/**
 * Starts listening. `now()` is ms since the recording started.
 * @returns {{ stop(durationMs: number): Promise<object> }}
 */
export function listen(now, { lang, Recognition = speechRecognition() } = {}) {
  if (!Recognition) {
    return { stop: async () => missingTranscript("This browser has no speech recognition.") };
  }
  const segments = [];
  let heardFrom = null;
  let stopped = false;
  let failure = null;
  const recognition = new Recognition();
  recognition.continuous = true;
  recognition.interimResults = false;
  if (lang) recognition.lang = lang;
  recognition.onspeechstart = () => {
    if (heardFrom === null) heardFrom = now();
  };
  recognition.onresult = (event) => {
    for (let i = event.resultIndex; i < event.results.length; i++) {
      const result = event.results[i];
      if (!result.isFinal) continue;
      const text = result[0]?.transcript?.trim();
      if (!text) continue;
      const t1 = now();
      const previous = segments.length ? segments[segments.length - 1].t1 : 0;
      const t0 = Math.min(heardFrom ?? previous, t1);
      segments.push({ t0, t1, text });
      heardFrom = null;
    }
  };
  recognition.onerror = (event) => {
    // "no-speech" and "aborted" come and go; anything else ends the transcript.
    if (event.error !== "no-speech" && event.error !== "aborted") failure = event.error;
  };
  // Recognition ends itself after a pause; keep listening until the recording stops.
  let ended = Promise.resolve();
  let resolveEnded;
  recognition.onend = () => {
    if (!stopped && !failure) {
      try {
        recognition.start();
        return;
      } catch {
        // fall through: it won't restart
      }
    }
    resolveEnded?.();
  };
  recognition.start();
  return {
    async stop(durationMs) {
      stopped = true;
      ended = new Promise((resolve) => {
        resolveEnded = resolve;
      });
      recognition.stop();
      // The last phrase arrives before `end`; don't wait forever for a browser that never ends.
      await Promise.race([ended, new Promise((resolve) => setTimeout(resolve, 1500))]);
      if (!segments.length) {
        return missingTranscript(
          failure ? `Speech recognition failed (${failure}).` : "Nothing was heard.",
        );
      }
      for (const s of segments) s.t1 = Math.min(s.t1, durationMs);
      return {
        source: "web-speech",
        text: segments.map((s) => s.text).join(" "),
        segments,
        estimated: true,
      };
    },
  };
}
