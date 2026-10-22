// What only the page knows, for the `agentContext` experiment: the element, component and
// source `file:line` under clicks and ⇧ marks, video seeks and plays, console errors and failed
// requests. The same events Scrawl's `sdk.js` posts to the Mac app (`page::Event`), which the
// compiler turns into click labels, what a mark was drawn on, and app events (`page::apply`).
// Never request bodies, headers, cookies, query strings or typed values.

import { nameOf } from "./input.js";

const MAX_EVENTS = 2_000;
const ignored = /\/node_modules\/|\/\.vite\/deps\/|react-dom|jsx-dev-runtime|jsx-runtime/;
// This file's own frames (the console.error wrapper) are never the page's code.
const self = import.meta.url.split(/[?#]/)[0];

const clip = (text, max) => {
  const s = String(text == null ? "" : text).replace(/\s+/g, " ").trim();
  return s ? s.slice(0, max) : undefined;
};

/** `src/ui/Pill.tsx:142` from the first stack frame in the page's own code. */
export function stackSource(stack) {
  for (const line of String(stack || "").split("\n")) {
    const m = line.match(/((?:https?|file):\/\/[^\s()]+?):(\d+):\d+\)?\s*$/);
    if (!m || ignored.test(m[1]) || m[1] === self) continue;
    try {
      const path = decodeURIComponent(new URL(m[1]).pathname).replace(/^\/(@fs\/)?/, "");
      return `${path}:${m[2]}`;
    } catch {}
  }
  return undefined;
}

const short = (file, line) => {
  try {
    return decodeURIComponent(new URL(file, location.href).pathname).replace(/^\//, "") + (line ? `:${line}` : "");
  } catch {
    return undefined;
  }
};

// Source locators some dev setups add (solid-devtools, react-dev-inspector, Sentry).
const located = (el) => {
  const d = el.dataset || {};
  if (d.sourceLoc) return { source: d.sourceLoc.replace(/^(.*:\d+):\d+$/, "$1") };
  if (d.inspectorRelativePath) return { source: d.inspectorRelativePath + (d.inspectorLine ? `:${d.inspectorLine}` : "") };
  if (d.sentrySourceFile) return { component: d.sentryComponent, source: d.sentrySourceFile };
  return null;
};

const fiberOf = (el) => {
  for (const key in el) {
    if (key.startsWith("__reactFiber$") || key.startsWith("__reactInternalInstance$")) return el[key];
  }
  return null;
};

const reactInfo = (fiber) => {
  const out = {};
  const src = fiber._debugSource;
  if (src?.fileName) out.source = short(src.fileName, src.lineNumber);
  else if (fiber._debugStack) out.source = stackSource(fiber._debugStack.stack);
  for (let f = fiber; f; f = f.return) {
    const type = f.type;
    const fn = typeof type === "function" ? type : type && typeof type === "object" ? type.render || type.type : null;
    const name = fn && (fn.displayName || fn.name);
    if (name) {
      out.component = name;
      break;
    }
  }
  return out;
};

/** The component and source `file:line` an element came from, when the dev setup says. */
export function componentOf(el) {
  for (let node = el, depth = 0; node && depth < 30; node = node.parentElement, depth++) {
    const found = located(node);
    if (found) return found;
    const fiber = fiberOf(node);
    if (fiber) return reactInfo(fiber);
  }
  return {};
}

/** `page::Target` for an element: tag, id, two classes, `describe`'s name, component, source. */
export function targetOf(element) {
  const named = nameOf(element);
  if (!named) return undefined;
  const { el, tag, text } = named;
  const target = { tag };
  if (el.id) target.id = clip(el.id, 60);
  const cls = [...el.classList].filter((c) => c.length <= 30 && !/[:[\]/]/.test(c)).slice(0, 2);
  if (cls.length) target.cls = cls.join(".");
  if (text) target.text = text;
  return Object.assign(target, componentOf(el));
}

const describeValue = (value) =>
  value instanceof Error
    ? value.message
    : typeof value === "string"
      ? value
      : value?.constructor
        ? `[${value.constructor.name}]`
        : String(value);

/**
 * Watches the page until `stop()`, which hands back its events (`page::Event`s, `t` from
 * `stamp()`, epoch ms) and puts back what it wrapped (`console.error`, `fetch`, XHR).
 */
export function watchPage(stamp) {
  const events = [];
  let active = true;
  const push = (event) => {
    if (!active || events.length >= MAX_EVENTS) return;
    event.t = stamp();
    event.focused = document.hasFocus();
    events.push(event);
  };

  const listeners = [];
  const on = (target, type, handler, options) => {
    target.addEventListener(type, handler, options);
    listeners.push(() => target.removeEventListener(type, handler, options));
  };
  // The first node of the composed path: inside an open shadow root `target` is only the host.
  on(window, "pointerdown", (e) => push({ kind: "pointer", target: targetOf(e.composedPath()[0] ?? e.target) }), {
    capture: true,
    passive: true,
  });
  let lastMark = 0;
  on(window, "pointermove", (e) => {
    if (!e.shiftKey) return;
    const now = performance.now();
    if (now - lastMark < 250) return;
    lastMark = now;
    // The ink overlay ignores the pointer, so this is the page under it.
    push({ kind: "mark", target: targetOf(document.elementFromPoint(e.clientX, e.clientY)) });
  }, { capture: true, passive: true });
  for (const type of ["seeked", "play", "pause"]) {
    on(document, type, (e) => {
      const m = e.target;
      if (!(m instanceof HTMLMediaElement)) return;
      const src = (m.currentSrc || m.src || "").split(/[?#]/)[0].split("/").pop();
      push({
        kind: "media",
        event: type,
        // A live stream's duration is Infinity, which JSON would turn into null.
        media: { src: clip(src, 60) || "", time: m.currentTime, duration: Number.isFinite(m.duration) ? m.duration : 0, paused: m.paused },
      });
    }, true);
  }
  on(window, "error", (e) => {
    if (!e.message) return;
    push({
      kind: "error",
      message: clip(/^Uncaught/.test(e.message) ? e.message : `Uncaught ${e.message}`, 200),
      source: (e.error && stackSource(e.error.stack)) || (e.filename ? short(e.filename, e.lineno) : undefined),
    });
  });
  on(window, "unhandledrejection", (e) =>
    push({
      kind: "error",
      message: clip(`Unhandled rejection: ${describeValue(e.reason)}`, 200),
      source: e.reason instanceof Error ? stackSource(e.reason.stack) : undefined,
    }),
  );

  // Wrapped until stop(): each wrapper goes quiet then, and is put back if nothing has wrapped
  // it since.
  const restores = [];
  const wrap = (owner, key, make) => {
    const original = owner[key];
    const wrapper = make(original);
    owner[key] = wrapper;
    restores.push(() => {
      if (owner[key] === wrapper) owner[key] = original;
    });
  };
  wrap(console, "error", (original) =>
    function (...args) {
      try {
        const error = args.find((a) => a instanceof Error);
        push({
          kind: "error",
          message: clip(args.map(describeValue).join(" "), 200),
          source: stackSource(error ? error.stack : new Error().stack),
        });
      } catch {}
      return original.apply(this, args);
    },
  );

  // Failed requests: method, path (no query) and status; never bodies or headers.
  const pathOf = (url) => {
    try {
      const u = new URL(url, location.href);
      return (u.origin === location.origin ? "" : u.host) + u.pathname;
    } catch {
      return null;
    }
  };
  const failed = (method, url, status, started) => {
    const path = pathOf(url);
    if (path) push({ kind: "request", method, path, status, ms: Math.round(performance.now() - started) });
  };
  wrap(window, "fetch", (original) =>
    function (input, init) {
      const started = performance.now();
      const method = String(init?.method || input?.method || "GET").toUpperCase();
      const url = typeof input === "string" ? input : input?.url || String(input);
      const pending = original.call(this, input, init);
      pending.then(
        (r) => {
          // A no-cors response is opaque: status 0 even when it worked.
          if (!r.ok && r.type !== "opaque" && r.type !== "opaqueredirect") failed(method, r.url || url, r.status, started);
        },
        () => failed(method, url, 0, started),
      );
      return pending;
    },
  );
  const xhr = XMLHttpRequest.prototype;
  const seen = new WeakMap();
  wrap(xhr, "open", (original) =>
    function (method, url, ...rest) {
      seen.set(this, { method: String(method || "GET").toUpperCase(), url: String(url) });
      return original.call(this, method, url, ...rest);
    },
  );
  wrap(xhr, "send", (original) =>
    function (...args) {
      const info = seen.get(this);
      if (info && active) {
        const started = performance.now();
        this.addEventListener("loadend", () => {
          if (this.status === 0 || this.status >= 400) failed(info.method, info.url, this.status, started);
        });
      }
      return original.apply(this, args);
    },
  );

  return {
    stop() {
      active = false;
      listeners.forEach((off) => off());
      restores.forEach((restore) => restore());
      return events;
    },
  };
}
