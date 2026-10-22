// What the person does on the page while it records: clicks, scrolls, and ⇧ ink (hold ⇧ and
// move the pointer to draw). Positions are fractions of the viewport, which the tab's video
// shows.

/** `scrawl.json`'s ink coordinate scale: a stroke's points run from 0 to `SCALE`. */
export const SCALE = 10_000;

/**
 * A short, plain name for an element, as Scrawl's `sdk.js` gives it: `aria-label`, `title`,
 * `alt` or `data-testid`, else up to 60 characters of its text. Form fields give only a
 * placeholder or name, and editable text only its label: never what was typed. Password
 * fields give nothing.
 */
export function describe(element) {
  const named = nameOf(element);
  if (!named) return undefined;
  return named.text ? `<${named.tag}> “${named.text}”` : `<${named.tag}>`;
}

/**
 * The element `describe` names (`el`, its `tag`) and its plain name (`text`, maybe empty), by
 * `describe`'s rules.
 */
export function nameOf(element) {
  if (!element || element.nodeType !== 1) return undefined;
  // Inside editable text, name its editing host: an ancestor's text would include what was typed.
  let host = element.isContentEditable ? element : null;
  while (host?.parentElement?.isContentEditable) host = host.parentElement;
  const el = host || element.closest?.("a, button, input, select, textarea, [role], label, [aria-label]") || element;
  const tag = el.tagName.toLowerCase();
  const attr = (name) => el.getAttribute?.(name)?.trim();
  const named = (text) => ({ el, tag, text: (text || "").slice(0, 60) });
  // Editable text: name it by its label or placeholder, never by what was typed into it.
  const role = attr("role");
  if (el.isContentEditable || role === "textbox" || role === "searchbox") {
    return named(attr("aria-label") || attr("placeholder") || attr("aria-placeholder"));
  }
  if (tag === "input" || tag === "textarea" || tag === "select") {
    if (attr("type") === "password") return named("");
    return named(attr("aria-label") || attr("placeholder") || attr("name"));
  }
  return named(
    attr("aria-label") || attr("title") || attr("alt") || attr("data-testid") ||
      (el.textContent || "").replace(/\s+/g, " ").trim(),
  );
}

/** A fraction of the viewport, rounded to 4 places. */
export function fraction(px, size) {
  return Math.round(Math.min(Math.max(px / Math.max(size, 1), 0), 1) * 10_000) / 10_000;
}

/**
 * A stroke in `scrawl.json`'s shape from pointer points (`{ x, y, t }`, CSS px and ms since
 * the recording started) on a `width`×`height` viewport; `undefined` for fewer than two.
 */
export function strokeOf(points, width, height) {
  if (points.length < 2) return undefined;
  const t0 = points[0].t;
  const pts = [];
  for (const p of points) {
    pts.push(Math.round(fraction(p.x, width) * SCALE), Math.round(fraction(p.y, height) * SCALE), p.t - t0);
  }
  return { t0, t1: points[points.length - 1].t, fw: Math.round(width), fh: Math.round(height), pts };
}

/**
 * Listens to the page. `now()` is ms since the recording started; `onInk(drawing)` says when
 * ink starts and when it has faded, so frame sampling can wait for a clean screen.
 */
export function recordInput(now, { onAction, onInk, showInk = true } = {}) {
  const events = [];
  const strokes = [];
  let stroke = null;
  let lastScroll = 0;
  const overlay = showInk ? inkOverlay() : null;

  const viewport = () => ({ width: window.innerWidth, height: window.innerHeight });
  const onPointerDown = (e) => {
    if (e.shiftKey) return;
    const { width, height } = viewport();
    const event = { kind: "click", t: now(), x: fraction(e.clientX, width), y: fraction(e.clientY, height) };
    if (e.button === 2) event.button = "right";
    const label = describe(e.target);
    if (label) event.label = label;
    events.push(event);
    onAction?.();
  };
  const onWheel = (e) => {
    const t = now();
    if (t - lastScroll < 250) return;
    lastScroll = t;
    const { width, height } = viewport();
    events.push({ kind: "scroll", t, x: fraction(e.clientX, width), y: fraction(e.clientY, height), dx: e.deltaX, dy: e.deltaY });
  };
  const onPointerMove = (e) => {
    if (!e.shiftKey) return endStroke();
    if (!stroke) {
      stroke = [];
      onInk?.(true);
    }
    stroke.push({ x: e.clientX, y: e.clientY, t: now() });
    overlay?.draw(stroke);
  };
  const endStroke = () => {
    if (!stroke) return;
    const { width, height } = viewport();
    const done = strokeOf(stroke, width, height);
    if (done) strokes.push(done);
    stroke = null;
    const fade = overlay ? overlay.fade() : Promise.resolve();
    fade.then(() => onInk?.(false));
  };
  const onKeyUp = (e) => {
    if (e.key === "Shift") endStroke();
  };
  window.addEventListener("pointerdown", onPointerDown, true);
  window.addEventListener("pointermove", onPointerMove, true);
  window.addEventListener("wheel", onWheel, { capture: true, passive: true });
  window.addEventListener("keyup", onKeyUp, true);
  return {
    stop() {
      endStroke();
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("pointermove", onPointerMove, true);
      window.removeEventListener("wheel", onWheel, { capture: true });
      window.removeEventListener("keyup", onKeyUp, true);
      overlay?.remove();
      return { events, strokes };
    },
  };
}

/** A canvas over the page showing ink while it's drawn, then fading it out. */
function inkOverlay() {
  const canvas = document.createElement("canvas");
  canvas.setAttribute("data-scrawl-ink", "");
  Object.assign(canvas.style, {
    position: "fixed", inset: "0", width: "100vw", height: "100vh",
    pointerEvents: "none", zIndex: "2147483647", transition: "opacity 300ms",
  });
  document.documentElement.appendChild(canvas);
  const context = canvas.getContext("2d");
  let lines = [];
  const paint = () => {
    const ratio = window.devicePixelRatio || 1;
    canvas.width = window.innerWidth * ratio;
    canvas.height = window.innerHeight * ratio;
    context.scale(ratio, ratio);
    context.strokeStyle = "#ff2d2d";
    context.lineWidth = 3;
    context.lineCap = context.lineJoin = "round";
    for (const line of lines) {
      context.beginPath();
      line.forEach((p, i) => (i ? context.lineTo(p.x, p.y) : context.moveTo(p.x, p.y)));
      context.stroke();
    }
  };
  return {
    draw(points) {
      canvas.style.opacity = "1";
      if (!lines.includes(points)) lines.push(points);
      paint();
    },
    fade() {
      canvas.style.opacity = "0";
      return new Promise((resolve) =>
        setTimeout(() => {
          lines = [];
          paint();
          resolve();
        }, 350),
      );
    },
    remove: () => canvas.remove(),
  };
}
