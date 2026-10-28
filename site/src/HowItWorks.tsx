import { type ReactNode, type RefObject, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react"
import { BROKEN, MacWindow } from "./demo/apps"
import { INK_SHADOW, InkPath, inkPath } from "./demo/ink"
import { measureAnchors } from "./demo/measure"
import { smoother, springAt } from "./demo/motion"
import { type Anchors, AREA, buildScript, lerp, NARRATION, progress, type Rect, type Script, STAGE, strokeAt, T } from "./demo/script"
import { FrameBadge, KeyframeScene, MenuBar } from "./demo/stage-parts"
import { Terminal } from "./demo/terminal"
import { useAfterLoad } from "./idle"

// Each diagram is drawn in a 480 × 288 box and scaled to its card. A diagram plays on a loop only
// while it is on screen; reduced motion shows its settled pose and never moves.
const W = 480
const H = 288

type Spring = { duration: number; bounce: number }
const SNAPPY: Spring = { duration: 0.25, bounce: 0 }
const CALM: Spring = { duration: 0.4, bounce: 0 }
const SMOOTH: Spring = { duration: 0.45, bounce: 0 }
const CAMERA: Spring = { duration: 1.1, bounce: 0 }
const RETURN: Spring = { duration: 0.9, bounce: 0 }

const spring = (t: number, at: number, s: Spring) => Math.min(1, springAt(t - at, s))
// The whole diagram cools out together before the loop starts again.
const fadeOut = (t: number, at: number, ms = 450) => 1 - smoother((t - at) / ms)

/* ---------- one shared frame loop ---------- */

const listeners = new Set<(now: number) => void>()
let raf = 0
const frame = (now: number) => {
  for (const l of listeners) l(now)
  raf = listeners.size ? requestAnimationFrame(frame) : 0
}
const onFrame = (cb: (now: number) => void) => {
  listeners.add(cb)
  if (!raf) raf = requestAnimationFrame(frame)
  return () => {
    listeners.delete(cb)
  }
}

const reduceQuery = typeof window === "undefined" ? null : window.matchMedia?.("(prefers-reduced-motion: reduce)")
const subscribeReduced = (cb: () => void) => {
  reduceQuery?.addEventListener("change", cb)
  return () => reduceQuery?.removeEventListener("change", cb)
}
const useReducedMotion = () => useSyncExternalStore(subscribeReduced, () => !!reduceQuery?.matches, () => false)

function useLoop(length: number, settled: number) {
  const ref = useRef<HTMLDivElement>(null)
  const reduced = useReducedMotion()
  const [t, setT] = useState(0)
  const [live, setLive] = useState(false)
  const [scale, setScale] = useState(0)
  const tRef = useRef(0)

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const measure = () => setScale(el.clientWidth / W)
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    let visible = false
    const update = () => setLive(visible && !document.hidden)
    const io = new IntersectionObserver(
      ([entry]) => {
        visible = entry.isIntersecting
        update()
      },
      { threshold: 0.35 },
    )
    io.observe(el)
    document.addEventListener("visibilitychange", update)
    return () => {
      ro.disconnect()
      io.disconnect()
      document.removeEventListener("visibilitychange", update)
    }
  }, [])

  useEffect(() => {
    if (!live || reduced) return
    let last = performance.now()
    return onFrame((now) => {
      const dt = Math.min(100, Math.max(0, now - last))
      last = now
      tRef.current = (tRef.current + dt) % length
      setT(tRef.current)
    })
  }, [live, reduced, length])

  return { ref, t: reduced ? settled : t, scale }
}

function Visual({ loop, settled, label, children }: { loop: number; settled: number; label: string; children: (t: number) => ReactNode }) {
  const { ref, t, scale } = useLoop(loop, settled)
  const live = useAfterLoad(ref)
  return (
    <div ref={ref} className="how-visual" role="img" aria-label={label}>
      <div className="how-box" style={{ width: W, height: H, transform: `scale(${scale})` }} aria-hidden inert>
        {live && children(t)}
      </div>
    </div>
  )
}

function useMacScript(section: RefObject<HTMLElement | null>) {
  const stageRef = useRef<HTMLDivElement>(null)
  const badgeRef = useRef<HTMLSpanElement>(null)
  const actionsRef = useRef<HTMLDivElement>(null)
  const titleRef = useRef<HTMLHeadingElement>(null)
  const [anchors, setAnchors] = useState<Anchors | null>(null)
  const live = useAfterLoad(section)

  useLayoutEffect(() => {
    const measure = () => {
      if (!stageRef.current || !badgeRef.current || !actionsRef.current || !titleRef.current) return
      setAnchors(measureAnchors(stageRef.current, badgeRef.current, actionsRef.current, titleRef.current))
    }
    measure()
    document.fonts?.ready.then(measure)
  }, [live])

  const script = useMemo(() => (anchors ? buildScript("mac", anchors) : null), [anchors])
  const probe = !live ? null : (
    <div className="how-probe" aria-hidden inert>
      <div ref={stageRef} style={{ width: STAGE.w, height: STAGE.h }}>
        <MacWindow fix={BROKEN} refs={{ badge: badgeRef, actions: actionsRef, title: titleRef }} />
      </div>
    </div>
  )
  return { script, probe }
}

/* ---------- 1. Frames only when something happens ---------- */

const TRACK_Y = 206
const X0 = 24
const X1 = 456
const SWEEP: [number, number] = [300, 3900]
const PAUSE: [number, number] = [252, 352]
const timeAt = (x: number) => SWEEP[0] + ((x - X0) / (X1 - X0)) * (SWEEP[1] - SWEEP[0])

type Moment = { x: number; kind: "draw" | "click" | "cue"; label: string; n: number }
const MOMENTS: Moment[] = [
  { x: 88, kind: "draw", label: "draw", n: 1 },
  { x: 198, kind: "click", label: "click", n: 2 },
  { x: 404, kind: "cue", label: "“now”", n: 3 },
]
const MARK_COLOR = { draw: "#ff3b30", click: "#f5f5f7", cue: "#ffd60a" }
const THUMB = { w: 92, h: 62, top: 54 }

function Thumb({ kind, n }: { kind: Moment["kind"]; n: number }) {
  const { w, h } = THUMB
  return (
    <g>
      <rect width={w} height={h} rx={7} fill="#1c1c1f" stroke="rgba(255,255,255,.12)" />
      {[0, 1, 2].map((i) => (
        <circle key={i} cx={9 + i * 6} cy={8} r={2} fill="rgba(255,255,255,.22)" />
      ))}
      {kind === "cue" ? (
        <>
          <rect x={10} y={18} width={40} height={5} rx={2.5} fill="rgba(255,255,255,.5)" />
          {[14, 22, 18, 28, 24, 32].map((v, i) => (
            <rect key={i} x={12 + i * 11} y={54 - v} width={7} height={v} rx={1.5} fill="#0a84ff" opacity={0.75} />
          ))}
        </>
      ) : (
        <>
          <rect x={10} y={18} width={34} height={5} rx={2.5} fill="rgba(255,255,255,.5)" />
          <rect x={48} y={17} width={18} height={7} rx={3.5} fill="#30d158" opacity={0.85} />
          {[30, 38].map((y) => (
            <rect key={y} x={10} y={y} width={60} height={4} rx={2} fill="rgba(255,255,255,.16)" />
          ))}
          <rect x={52} y={46} width={30} height={9} rx={3} fill="#0a84ff" />
        </>
      )}
      {kind === "draw" && <ellipse cx={42} cy={20.5} rx={30} ry={9} fill="none" stroke="#ff3b30" strokeWidth={2} transform="rotate(-4 42 20.5)" />}
      {kind === "click" && (
        <>
          <circle cx={67} cy={50.5} r={9} fill="none" stroke="#fff" strokeWidth={1.4} opacity={0.7} />
          <circle cx={67} cy={50.5} r={2.6} fill="#fff" />
        </>
      )}
      <rect x={w - 21} y={5} width={16} height={14} rx={7} fill="rgba(18,18,20,.86)" stroke="rgba(255,255,255,.16)" />
      <text x={w - 13} y={15} textAnchor="middle" fontSize={9} fontWeight={700} fill="#fff">
        {n}
      </text>
    </g>
  )
}

function MomentsDiagram({ t }: { t: number }) {
  const x = lerp(X0, X1, progress(t, SWEEP[0], SWEEP[1]))
  const env = fadeOut(t, 6900)
  const headIn = smoother(progress(t, 0, 300)) * (1 - smoother(progress(t, SWEEP[1], SWEEP[1] + 300)))
  const pauseIn = smoother(progress(t, timeAt(PAUSE[0]), timeAt(PAUSE[0]) + 300))
  const seg = (a: number, b: number) => (x > a ? <line x1={a} x2={Math.min(x, b)} y1={TRACK_Y} y2={TRACK_Y} /> : null)
  return (
    <svg className="how-svg" viewBox={`0 0 ${W} ${H}`} aria-hidden>
      <line x1={X0} x2={X1} y1={TRACK_Y} y2={TRACK_Y} stroke="rgba(255,255,255,.08)" strokeWidth={2} strokeLinecap="round" />
      <g opacity={env}>
        <g stroke="rgba(255,255,255,.4)" strokeWidth={2} strokeLinecap="round">
          {seg(X0, PAUSE[0])}
          {seg(PAUSE[1], X1)}
        </g>
        <g stroke="rgba(255,255,255,.4)" strokeWidth={2} strokeDasharray="2 5" strokeLinecap="round">
          {seg(PAUSE[0], PAUSE[1])}
        </g>
        <text className="how-label" x={(PAUSE[0] + PAUSE[1]) / 2} y={TRACK_Y + 26} textAnchor="middle" opacity={pauseIn * 0.7}>
          pause
        </text>
        {MOMENTS.map((m) => {
          const at = timeAt(m.x)
          const mark = spring(t, at, SNAPPY)
          const rise = spring(t, at + 60, CALM)
          const ripple = progress(t, at, at + 460)
          const stemTop = THUMB.top + THUMB.h + 6
          return (
            <g key={m.n}>
              {rise > 0 && (
                <>
                  <line x1={m.x} x2={m.x} y1={TRACK_Y - 8} y2={lerp(TRACK_Y - 8, stemTop, rise)} stroke="rgba(255,255,255,.18)" strokeWidth={1} />
                  <g transform={`translate(${m.x - THUMB.w / 2} ${(THUMB.top + (1 - rise) * 30).toFixed(2)})`} opacity={rise}>
                    <Thumb kind={m.kind} n={m.n} />
                  </g>
                </>
              )}
              {mark > 0 && (
                <>
                  {ripple < 1 && (
                    <circle cx={m.x} cy={TRACK_Y} r={5 + 13 * (1 - (1 - ripple) ** 3)} fill="none" stroke={MARK_COLOR[m.kind]} strokeWidth={1.5} opacity={0.6 * (1 - ripple) ** 2} />
                  )}
                  <circle cx={m.x} cy={TRACK_Y} r={5 * (0.6 + 0.4 * mark)} fill={MARK_COLOR[m.kind]} opacity={mark} />
                  <text className="how-label" x={m.x} y={TRACK_Y + 26} textAnchor="middle" opacity={mark * 0.9}>
                    {m.label}
                  </text>
                </>
              )}
            </g>
          )
        })}
      </g>
      <g opacity={headIn} transform={`translate(${x.toFixed(2)} 0)`}>
        <line y1={TRACK_Y - 14} y2={TRACK_Y + 8} stroke="#fff" strokeWidth={1.5} strokeLinecap="round" />
      </g>
    </svg>
  )
}

/* ---------- 2. Each frame keeps its words ---------- */

const RATE = 1.5
const SPEECH_START = 300
const recAt = (t: number) => (t - SPEECH_START) * RATE
const DURATION = T.finish - T.record
const AXIS = { y: 30, x0: 24, x1: 456 }
const axisX = (ms: number) => lerp(AXIS.x0, AXIS.x1, ms / DURATION)
const ROW = { x: 150, top: [62, 168], thumbW: 110 }
const THUMB_H = (ROW.thumbW * AREA.h) / AREA.w

function WordsDiagram({ t, script }: { t: number; script: Script | null }) {
  const { words, clause2 } = NARRATION.mac
  const rec = recAt(t)
  const env = fadeOut(t, 9300)
  const rel = words.map(([w, at, end]) => [w, at - T.record, end - T.record] as const)
  const clauses = [rel.slice(0, clause2), rel.slice(clause2)]
  const frames = script?.frames ?? []
  const head = Math.max(0, Math.min(DURATION, rec))
  const headIn = smoother(progress(t, SPEECH_START, SPEECH_START + 300)) * (1 - smoother(progress(rec, DURATION - 300, DURATION)))
  return (
    <>
      <svg className="how-svg" viewBox={`0 0 ${W} ${H}`} aria-hidden>
        <line x1={AXIS.x0} x2={AXIS.x1} y1={AXIS.y} y2={AXIS.y} stroke="rgba(255,255,255,.08)" strokeWidth={2} strokeLinecap="round" />
        <g opacity={env}>
          {rel.map(([, at], i) => {
            const on = smoother(progress(rec, at - 35, at + 165))
            return on > 0 ? <rect key={i} x={axisX(at) - 1} y={AXIS.y - 6} width={2} height={12} rx={1} fill="#f5f5f7" opacity={0.75 * on} /> : null
          })}
          {clauses.map((c, i) => {
            const f = frames[i]
            const shown = f ? spring(rec / RATE, f.t / RATE, CALM) : 0
            if (shown <= 0) return null
            const a = axisX(c[0][1]) - 3
            const b = axisX(c[c.length - 1][1]) + 5
            return (
              <path
                key={i}
                d={`M${a} ${AXIS.y + 11}v4H${b}v-4`}
                fill="none"
                stroke="rgba(255,255,255,.35)"
                strokeWidth={1}
                opacity={shown}
                transform={`translate(0 ${((1 - shown) * -3).toFixed(2)})`}
              />
            )
          })}
        </g>
        <g opacity={headIn} transform={`translate(${axisX(head).toFixed(2)} 0)`}>
          <line y1={AXIS.y - 11} y2={AXIS.y + 11} stroke="#fff" strokeWidth={1.5} strokeLinecap="round" />
        </g>
      </svg>
      {clauses.map((c, i) => {
        const f = frames[i]
        const top = ROW.top[i]
        const shown = f ? spring(rec / RATE, f.t / RATE, CALM) : 0
        const k = ROW.thumbW / AREA.w
        return (
          <div key={i} className="how-row" style={{ top }}>
            <div className="how-slot" style={{ width: ROW.thumbW, height: THUMB_H }}>
              {script && f && shown > 0 && (
                <div className="how-thumb sheet-card" style={{ opacity: shown * env, transform: shown < 1 ? `scale(${(0.96 + 0.04 * shown).toFixed(4)})` : undefined }}>
                  <KeyframeScene mode="mac" script={script} frame={f} scale={k} />
                  <FrameBadge n={f.n} k={k * 1.8} />
                </div>
              )}
            </div>
            <div className="how-quote" style={{ opacity: env }}>
              <span className="how-when" style={{ opacity: shown }}>
                #{i + 1} · 0:0{Math.floor((f?.t ?? 0) / 1000)}
              </span>
              <p>
                {c.map(([w, at], j) => {
                  const a = Math.min(1, Math.max(0, (rec - (at - 35)) / 240))
                  const e = 1 - (1 - a) ** 3
                  return (
                    <span key={j}>
                      <span
                        className="how-word"
                        style={{
                          opacity: e,
                          transform: e < 1 ? `translate3d(0, ${((1 - e) * 3).toFixed(2)}px, 0)` : undefined,
                          filter: e > 0 && e < 1 ? `blur(${((1 - e) * 1.5).toFixed(2)}px)` : undefined,
                        }}
                      >
                        {j === 0 ? "“" : ""}
                        {w}
                        {j === c.length - 1 ? "”" : ""}
                      </span>{" "}
                    </span>
                  )
                })}
              </p>
            </div>
          </div>
        )
      })}
    </>
  )
}

/* ---------- 3. Cropped to where you worked ---------- */

const FULL: Rect = { x: 0, y: 0, w: STAGE.w, h: STAGE.h }
// The camera ends on the crop with a little margin, at the box's aspect.
const VIEW: Rect = (() => {
  const h = AREA.h + 40
  const w = (h * W) / H
  return { x: AREA.x + AREA.w / 2 - w / 2, y: AREA.y + AREA.h / 2 - h / 2, w, h }
})()
const lerpRect = (a: Rect, b: Rect, k: number): Rect => ({ x: lerp(a.x, b.x, k), y: lerp(a.y, b.y, k), w: lerp(a.w, b.w, k), h: lerp(a.h, b.h, k) })
const CROP_INK: [number, number] = [500, 1700]
const CROP_AT = 2000
const ZOOM_AT = 2900
const BACK_AT = 6200

function CropDiagram({ t, script }: { t: number; script: Script | null }) {
  const back = spring(t, BACK_AT, RETURN)
  const zoom = spring(t, ZOOM_AT, CAMERA) * (1 - back)
  const view = lerpRect(FULL, VIEW, zoom)
  const s = W / view.w
  const out = fadeOut(t, BACK_AT, 400)
  const crop = lerpRect(FULL, AREA, spring(t, CROP_AT + 120, SMOOTH))
  const cropIn = smoother(progress(t, CROP_AT, CROP_AT + 200)) * out
  const box = { x: (crop.x - view.x) * s, y: (crop.y - view.y) * s, w: crop.w * s, h: crop.h * s }
  const dim = cropIn * (0.45 + 0.51 * zoom)
  const circle = script?.strokes.find((st) => st.id === "circle")
  const live = circle && t > CROP_INK[0] ? strokeAt(circle, lerp(circle.t0, circle.t1, progress(t, CROP_INK[0], CROP_INK[1]))) : null
  const badge = spring(t, ZOOM_AT + 700, CALM) * out
  return (
    <>
      <div className="sheet-scene wallpaper" style={{ width: STAGE.w, height: STAGE.h, transform: `scale(${s.toFixed(5)}) translate(${(-view.x).toFixed(2)}px, ${(-view.y).toFixed(2)}px)` }}>
        <MenuBar t={0} />
        <MacWindow fix={BROKEN} />
        {script && <Terminal t={0} mode="mac" script={script} />}
        {live && (
          <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`} style={{ ...INK_SHADOW, opacity: out }}>
            <InkPath d={inkPath(live.points, 9, 0)} />
          </svg>
        )}
      </div>
      {cropIn > 0 && (
        <svg className="how-svg" viewBox={`0 0 ${W} ${H}`} aria-hidden>
          <path
            fillRule="evenodd"
            d={`M-2 -2H${W + 2}V${H + 2}H-2ZM${box.x} ${box.y}h${box.w}v${box.h}H${box.x}Z`}
            fill={`rgba(11,11,14,${dim.toFixed(3)})`}
          />
          <rect x={box.x} y={box.y} width={box.w} height={box.h} rx={2} fill="none" stroke="#fff" strokeWidth={1.5} opacity={cropIn * (0.9 - 0.6 * zoom)} />
        </svg>
      )}
      {badge > 0 && (
        <div className="how-crop-badge" style={{ left: box.x, top: box.y, width: box.w, height: box.h, opacity: badge }}>
          <FrameBadge n={1} k={0.9} />
        </div>
      )}
    </>
  )
}

/* ---------- 4. A closer look when needed ---------- */

const COMMAND = `scrawl zoom latest --at "colliding"`
const TYPE: [number, number] = [300, 300 + COMMAND.length * 28]
const CELLS_AT = TYPE[1] + 350
const CELL = { w: 102, h: 58, gap: 8, top: 58 }
const ZOOM_FROM = 2860
const ZOOM_STEP = 2000 / 11
const zoomClock = (ms: number) => `0:${String(Math.floor(ms / 1000)).padStart(2, "0")}.${String(Math.round(ms % 1000)).padStart(3, "0")}`

function ZoomCell({ i }: { i: number }) {
  // The badge slides in over the burst and lands on the title.
  const k = smoother(i / 11)
  const bx = lerp(74, 44, k)
  return (
    <svg width={CELL.w} height={CELL.h} viewBox={`0 0 ${CELL.w} ${CELL.h}`} aria-hidden>
      <rect x={0.5} y={0.5} width={CELL.w - 1} height={CELL.h - 1} rx={6} fill="#1c1c1f" stroke="rgba(255,255,255,.12)" />
      <rect x={10} y={11} width={44} height={6} rx={3} fill="rgba(255,255,255,.55)" />
      <rect x={bx} y={lerp(9, 10, k)} width={22} height={10} rx={5} fill="#12301b" stroke="#30d158" strokeOpacity={0.6} />
      <circle cx={bx + 6} cy={lerp(14, 15, k)} r={1.8} fill="#30d158" />
      {[26, 33].map((y) => (
        <rect key={y} x={10} y={y} width={70} height={3.5} rx={1.75} fill="rgba(255,255,255,.14)" />
      ))}
      <rect x={4} y={CELL.h - 15} width={46} height={11} rx={5.5} fill="rgba(18,18,20,.86)" stroke="rgba(255,255,255,.14)" />
      <text x={27} y={CELL.h - 7} textAnchor="middle" className="how-cell-time">
        {zoomClock(ZOOM_FROM + i * ZOOM_STEP)}
      </text>
    </svg>
  )
}

function ZoomDiagram({ t }: { t: number }) {
  const env = fadeOut(t, 6400)
  const typed = Math.floor(progress(t, TYPE[0], TYPE[1]) * COMMAND.length)
  const typing = t >= TYPE[0] - 200 && t < CELLS_AT
  const said = spring(t, CELLS_AT + 12 * 45 + 150, CALM)
  const x0 = (W - (CELL.w * 4 + CELL.gap * 3)) / 2
  return (
    <div className="how-zoom" style={{ opacity: env }}>
      <div className="how-cmd" style={{ left: x0, right: x0 }}>
        <span className="dim">$ </span>
        {COMMAND.slice(0, typed)}
        <i className={`how-caret${typing ? " on" : ""}`} />
      </div>
      {Array.from({ length: 12 }, (_, i) => {
        const k = spring(t, CELLS_AT + i * 45, CALM)
        if (k <= 0) return null
        const col = i % 4
        const row = Math.floor(i / 4)
        return (
          <div
            key={i}
            className="how-cell"
            style={{
              left: x0 + col * (CELL.w + CELL.gap),
              top: CELL.top + row * (CELL.h + CELL.gap),
              opacity: k,
              transform: k < 1 ? `translate3d(0, ${((1 - k) * -8).toFixed(2)}px, 0)` : undefined,
            }}
          >
            <ZoomCell i={i} />
          </div>
        )
      })}
      <div className="how-said" style={{ left: x0, opacity: said }}>
        Said: <span>“it's colliding with the title,”</span>
      </div>
    </div>
  )
}

/* ---------- section ---------- */

export function HowItWorks() {
  const section = useRef<HTMLElement>(null)
  const { script, probe } = useMacScript(section)
  return (
    <section id="how" className="how-section" ref={section}>
      {probe}
      <h2>How it works</h2>
      <div className="feature-cards">
        <article className="feature-card">
          <Visual loop={7600} settled={6000} label="A timeline where drawing, a click and the word “now” each add a frame, and a pause adds none">
            {(t) => <MomentsDiagram t={t} />}
          </Visual>
          <div className="feature-copy">
            <h3>Frames only when something happens</h3>
            <p>Scrawl takes a frame when you draw, click, say “look” or “now”, or the screen changes.</p>
          </div>
        </article>
        <article className="feature-card">
          <Visual loop={10000} settled={8800} label="Each spoken clause lands beside the keyframe it describes">
            {(t) => <WordsDiagram t={t} script={script} />}
          </Visual>
          <div className="feature-copy">
            <h3>Each frame keeps its words</h3>
            <p>Your speech is transcribed on your computer, and each sentence goes with the frame it was about.</p>
          </div>
        </article>
        <article className="feature-card">
          <Visual loop={7400} settled={5000} label="A full-screen recording cropped to the window that was drawn on">
            {(t) => <CropDiagram t={t} script={script} />}
          </Visual>
          <div className="feature-copy">
            <h3>Cropped to where you worked</h3>
            <p>Record the whole screen, and each frame is cut down to the window you drew or clicked in.</p>
          </div>
        </article>
        <article className="feature-card">
          <Visual loop={7200} settled={4000} label="The agent asks for a dense grid of frames around one moment">
            {(t) => <ZoomDiagram t={t} />}
          </Visual>
          <div className="feature-copy">
            <h3>A closer look when it’s needed</h3>
            <p>The agent can ask for a dense grid of frames around any moment, by time or by a phrase you said.</p>
          </div>
        </article>
      </div>
    </section>
  )
}
