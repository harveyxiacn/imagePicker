import { useEffect, useRef, useState, type PointerEvent as RPointerEvent } from 'react'
import {
  clampPan,
  classifySwipe,
  distance,
  DOUBLE_TAP_ZOOM,
  isTap,
  LONG_PRESS_MS,
  midpoint,
  nextTap,
  pinchZoom,
  TAP_SLOP_PX,
  zoomAround,
  type SwipeDir,
  type Tap,
} from '@/lib/gestures'

interface Opts {
  /** Surface size, for pan clamping. */
  box: { w: number; h: number }
  /** Changes when the content changes (new photo): zoom resets. */
  resetKey: unknown
  /** Directions a swipe may commit in; other directions spring back. */
  allow?: SwipeDir[]
  zoomable?: boolean
  /** Fired when a swipe commits. Return false to spring back (e.g. already at the first photo). */
  onSwipe: (dir: SwipeDir) => boolean | void
  onTap?: () => void
  onLongPress?: () => void
}

export interface Zoom {
  scale: number
  tx: number
  ty: number
}

const IDENTITY: Zoom = { scale: 1, tx: 0, ty: 0 }
const ZERO = { dx: 0, dy: 0 }

/**
 * Pointer-event gesture engine for a full-screen photo surface: one-finger drag (swipe / pan when zoomed),
 * two-finger pinch + pan, double-tap zoom, long-press. Classification rules live in `lib/gestures.ts`.
 * The element needs `touch-action: none`.
 */
export function useGestureSurface(opts: Opts) {
  const [drag, setDrag] = useState(ZERO)
  const [zs, setZs] = useState<Zoom & { key: unknown }>({ key: opts.resetKey, ...IDENTITY })
  const zoom: Zoom = zs.key === opts.resetKey ? zs : IDENTITY
  const latest = useRef({ opts, zoom })
  useEffect(() => {
    latest.current = { opts, zoom }
  })

  const st = useRef({
    pts: new Map<number, { x: number; y: number }>(),
    start: null as null | { x: number; y: number; t: number },
    maxPointers: 0,
    longPressed: false,
    moved: false,
    timer: 0 as unknown as ReturnType<typeof setTimeout>,
    lastTap: null as Tap | null,
    pinch: null as null | { dist: number; zoom: number; mid: { x: number; y: number }; tx: number; ty: number },
  })

  const setZoom = (z: Zoom) => setZs({ key: latest.current.opts.resetKey, ...z })

  const centre = (el: HTMLElement, e: { clientX: number; clientY: number }) => {
    const b = el.getBoundingClientRect()
    return { x: e.clientX - b.left - b.width / 2, y: e.clientY - b.top - b.height / 2 }
  }

  const onPointerDown = (e: RPointerEvent<HTMLElement>) => {
    if (e.pointerType === 'mouse' && e.button !== 0) return
    // Controls overlaid on the surface (back, edit, bulk actions) keep their own taps.
    if ((e.target as HTMLElement).closest('button, a, [data-no-gesture]')) return
    const s = st.current
    e.currentTarget.setPointerCapture(e.pointerId)
    s.pts.set(e.pointerId, { x: e.clientX, y: e.clientY })
    s.maxPointers = Math.max(s.maxPointers, s.pts.size)
    clearTimeout(s.timer)
    if (s.pts.size === 1) {
      s.start = { x: e.clientX, y: e.clientY, t: performance.now() }
      s.longPressed = false
      s.moved = false
      if (latest.current.opts.onLongPress) {
        s.timer = setTimeout(() => {
          if (st.current.moved || st.current.pts.size !== 1) return
          st.current.longPressed = true
          latest.current.opts.onLongPress?.()
        }, LONG_PRESS_MS)
      }
    } else if (s.pts.size === 2 && latest.current.opts.zoomable !== false) {
      const [a, b] = [...s.pts.values()]
      const z = latest.current.zoom
      s.pinch = { dist: distance(a, b), zoom: z.scale, mid: midpoint(a, b), tx: z.tx, ty: z.ty }
      setDrag(ZERO)
    }
  }

  const onPointerMove = (e: RPointerEvent<HTMLElement>) => {
    const s = st.current
    const prev = s.pts.get(e.pointerId)
    if (!prev) return
    const cur = { x: e.clientX, y: e.clientY }
    s.pts.set(e.pointerId, cur)
    const { opts: o, zoom: z } = latest.current

    if (s.pts.size >= 2 && s.pinch) {
      const [a, b] = [...s.pts.values()]
      const scale = pinchZoom(s.pinch.zoom, s.pinch.dist, distance(a, b))
      const el = e.currentTarget
      const r = el.getBoundingClientRect()
      const mid = midpoint(a, b)
      const focus = { x: s.pinch.mid.x - r.left - r.width / 2, y: s.pinch.mid.y - r.top - r.height / 2 }
      const zz = zoomAround(s.pinch.tx, s.pinch.ty, s.pinch.zoom, scale, focus)
      // follow the fingers' midpoint while pinching
      const pan = clampPan(zz.tx + (mid.x - s.pinch.mid.x), zz.ty + (mid.y - s.pinch.mid.y), scale, o.box)
      setZoom({ scale, ...pan })
      s.moved = true
      return
    }

    const st0 = s.start
    if (!st0) return
    if (Math.hypot(cur.x - st0.x, cur.y - st0.y) > TAP_SLOP_PX) {
      s.moved = true
      clearTimeout(s.timer)
    }
    if (s.longPressed) return
    if (z.scale > 1.02) {
      const pan = clampPan(z.tx + (cur.x - prev.x), z.ty + (cur.y - prev.y), z.scale, o.box)
      setZoom({ scale: z.scale, ...pan })
    } else if (s.maxPointers === 1) {
      setDrag({ dx: cur.x - st0.x, dy: cur.y - st0.y })
    }
  }

  const finish = (e: RPointerEvent<HTMLElement>, cancelled: boolean) => {
    const s = st.current
    if (!s.pts.has(e.pointerId)) return
    s.pts.delete(e.pointerId)
    clearTimeout(s.timer)
    const { opts: o, zoom: z } = latest.current
    if (s.pts.size > 0) {
      // one finger of a pinch lifted: the rest of the gesture is not a swipe
      s.pinch = null
      return
    }
    const start = s.start
    s.start = null
    const wasPinch = s.maxPointers > 1
    const pointers = s.maxPointers
    s.maxPointers = 0
    s.pinch = null
    if (wasPinch) {
      setDrag(ZERO)
      if (z.scale < 1.05) setZoom(IDENTITY)
      return
    }
    if (!start || cancelled) {
      setDrag(ZERO)
      return
    }
    const dx = e.clientX - start.x
    const dy = e.clientY - start.y
    const dt = performance.now() - start.t
    if (s.longPressed) {
      s.longPressed = false
      setDrag(ZERO)
      return
    }
    if (isTap(dx, dy, dt)) {
      setDrag(ZERO)
      const tap = { x: e.clientX, y: e.clientY, t: performance.now() }
      const r = nextTap(s.lastTap, tap)
      s.lastTap = r.prev
      if (r.double && o.zoomable !== false) {
        const focus = centre(e.currentTarget, e)
        if (z.scale > 1.02) setZoom(IDENTITY)
        else {
          const pan = zoomAround(0, 0, 1, DOUBLE_TAP_ZOOM, focus)
          setZoom({ scale: DOUBLE_TAP_ZOOM, ...clampPan(pan.tx, pan.ty, DOUBLE_TAP_ZOOM, o.box) })
        }
      } else if (!r.double) {
        o.onTap?.()
      }
      return
    }
    const dir = classifySwipe({ dx, dy, dt, pointers, zoom: z.scale })
    if (dir && (!o.allow || o.allow.includes(dir))) {
      const ok = o.onSwipe(dir)
      if (ok === false) setDrag(ZERO)
      // on success the owner animates and calls `resetDrag`
    } else setDrag(ZERO)
  }

  return {
    drag,
    zoom,
    setZoom,
    resetZoom: () => setZoom(IDENTITY),
    resetDrag: () => setDrag(ZERO),
    /** Drive the drag offset from outside (exit animation). */
    setDrag,
    bind: {
      onPointerDown,
      onPointerMove,
      onPointerUp: (e: RPointerEvent<HTMLElement>) => finish(e, false),
      onPointerCancel: (e: RPointerEvent<HTMLElement>) => finish(e, true),
    },
  }
}
