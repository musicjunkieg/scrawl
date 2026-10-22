/** A transcriber's answer: times in ms from the start of the recording. */
export interface Transcribed {
  text?: string;
  segments?: Array<{ t0: number; t1: number; text: string }>;
  words?: Array<{ t0: number; t1: number; text: string }>;
  source?: string;
  model?: string;
  estimated?: boolean;
}

export interface ScrawlOptions {
  /** Your transcriber, or `"web-speech"` for the browser's live recognition. */
  transcribe?: ((audio: Blob, info: { durationMs: number }) => Promise<Transcribed>) | "web-speech";
  /** Web Speech's language, like `"en-US"`. */
  lang?: string;
  note?: string;
  context?: Record<string, string>;
  /**
   * Scrawl's `agentContext` experiment: numbered marks, a richer `summary.md`, and what only the
   * page knows (the element, component and source `file:line` under clicks and ⇧ marks, console
   * errors, failed requests, video playback). Off by default.
   */
  agentContext?: boolean;
  /** Ms between frame samples (500). */
  interval?: number;
  /** Frames' longest side in px (1600). */
  maxEdge?: number;
  /** Most frames kept (300). */
  maxFrames?: number;
  /** Draw ⇧ ink while it's drawn (true). */
  showInk?: boolean;
  /** Where `worker.js` is served from (next to this module by default). */
  worker?: URL | string;
}

export interface ScrawlBundle {
  id: string;
  /** `[path, bytes]` for every file, in save order (`summary.md` last). */
  files: Array<[string, Uint8Array]>;
  /** `summary.md`: the text to hand an agent. */
  summary: string;
  /** `scrawl.json`. */
  manifest: Record<string, unknown>;
  /** The recorded voice, when your `transcribe` was used. */
  audio: Blob | null;
  file(name: string): Uint8Array | undefined;
  /** The bundle as one `.zip` with a `<id>.scrawl/` folder inside. */
  zip(): Blob;
}

export interface Scrawl {
  readonly recording: boolean;
  /** Asks to record this tab (and the microphone, with a `transcribe` function), then starts. */
  start(): Promise<void>;
  /** Stops and builds the bundle. */
  stop(): Promise<ScrawlBundle>;
}

export function createScrawl(options?: ScrawlOptions): Scrawl;
export function zip(files: Array<[string, Uint8Array]>, root?: string): Uint8Array;
export function normalizeTranscript(result: Transcribed, durationMs: number): Record<string, unknown>;
export function missingTranscript(reason: string): Record<string, unknown>;
export function compileFrames(
  Compiler: new () => unknown,
  capture: Record<string, unknown>,
  frames: Array<{ t: number; width: number; height: number; data: Uint8Array | Uint8ClampedArray }>,
): { files: Array<[string, Uint8Array]>; summary: string; manifest: Record<string, unknown> };
export function decodeFrame(frame: { t: number; blob: Blob }): Promise<{
  t: number;
  width: number;
  height: number;
  data: Uint8ClampedArray;
}>;
