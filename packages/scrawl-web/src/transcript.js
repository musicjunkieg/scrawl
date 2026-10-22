// The transcript a bundle carries, from whatever the page used to hear the speech.

/** The bundle's transcript when nothing transcribed the audio. */
export function missingTranscript(reason) {
  return {
    source: "none",
    text: "",
    segments: [],
    missing: { reason },
  };
}

/**
 * A `transcribe` result in `scrawl.json`'s shape. It may give `segments` (`{ t0, t1, text }`,
 * ms from the start of the recording, the shape Whisper-style engines return), `words` in the
 * same shape, or only `text`.
 * @param {{ text?: string, segments?: Array<{t0: number, t1: number, text: string}>,
 *   words?: Array<{t0: number, t1: number, text: string}>, source?: string, model?: string }} result
 * @param {number} durationMs
 */
export function normalizeTranscript(result, durationMs) {
  if (!result || typeof result !== "object") {
    return missingTranscript("The page's transcriber returned nothing.");
  }
  const timed = (list) =>
    (Array.isArray(list) ? list : [])
      .filter((s) => s && typeof s.text === "string" && s.text.trim())
      .map((s) => {
        const t0 = clamp(Math.round(Number(s.t0) || 0), 0, durationMs);
        return { t0, t1: clamp(Math.round(Number(s.t1) || t0), t0, durationMs), text: s.text.trim() };
      })
      .sort((a, b) => a.t0 - b.t0);
  let segments = timed(result.segments);
  const text = (typeof result.text === "string" ? result.text : segments.map((s) => s.text).join(" ")).trim();
  if (!text) return missingTranscript("Nothing was said.");
  let estimated = false;
  if (!segments.length) {
    // Only text: one segment over the whole recording, and the times are a guess.
    segments = [{ t0: 0, t1: durationMs, text }];
    estimated = true;
  }
  const transcript = { source: result.source || "page", text, segments };
  if (result.model) transcript.model = result.model;
  const words = timed(result.words);
  if (words.length) transcript.words = words;
  if (estimated || result.estimated) transcript.estimated = true;
  return transcript;
}

function clamp(n, lo, hi) {
  return Math.min(Math.max(n, lo), hi);
}
