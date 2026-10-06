/**
 * Touch gesture classification for the mobile cull view (doc 08 section 5, doc 04 section 5).
 * Pure functions so the rules (distance / velocity thresholds, axis dominance, conflicts with
 * pinch, zoomed-in panning and scrollable containers) are unit-testable without a browser.
 */

export type SwipeDir = 'left' | 'right' | 'up' | 'down'

export interface SwipeOptions {
  /** Distance (px) that commits a swipe by itself. */
  distance: number
  /** Shorter flicks commit when faster than this (px/ms) ... */
  velocity: number
  /** ... and moved at least this far (px). */
  minFlick: number
  /** Dominant axis must exceed the other by this ratio, otherwise it is a diagonal wobble. */
  axisRatio: number
}

export const DEFAULT_SWIPE: SwipeOptions = { distance: 64, velocity: 0.45, minFlick: 24, axisRatio: 1.4 }

export interface SwipeInput {
  dx: number
  dy: number
  /** Duration of the gesture (ms). */
  dt: number
  /** Pointers down during the gesture (2+ = pinch). */
  pointers?: number
  /** Image zoom factor: while zoomed in, a drag pans instead of swiping. */
  zoom?: number
  /** The gesture started inside a scrollable region that can scroll along the dominant axis. */
  scrollable?: boolean
  /** The press became a long-press (multi-select): never a swipe. */
  longPressed?: boolean
}

/** Dominant axis of a movement, `null` when it is too small / too diagonal. */
export function dominantAxis(dx: number, dy: number, ratio = DEFAULT_SWIPE.axisRatio): 'x' | 'y' | null {
  const ax = Math.abs(dx)
  const ay = Math.abs(dy)
  if (ax === 0 && ay === 0) return null
  if (ax >= ay * ratio) return 'x'
  if (ay >= ax * ratio) return 'y'
  return null
}

/** Direction of a finished gesture, or `null` when it should not trigger a swipe. */
export function classifySwipe(g: SwipeInput, opts: SwipeOptions = DEFAULT_SWIPE): SwipeDir | null {
  if ((g.pointers ?? 1) > 1) return null
  if ((g.zoom ?? 1) > 1.02) return null
  if (g.scrollable || g.longPressed) return null
  const axis = dominantAxis(g.dx, g.dy, opts.axisRatio)
  if (!axis) return null
  const dist = Math.abs(axis === 'x' ? g.dx : g.dy)
  const speed = g.dt > 0 ? dist / g.dt : Infinity
  const committed = dist >= opts.distance || (dist >= opts.minFlick && speed >= opts.velocity)
  if (!committed) return null
  if (axis === 'x') return g.dx < 0 ? 'left' : 'right'
  return g.dy < 0 ? 'up' : 'down'
}

/** 0..1 progress of an in-flight drag toward committing in `dir` (drives the feedback overlay opacity). */
export function swipeProgress(dx: number, dy: number, dir: SwipeDir, opts: SwipeOptions = DEFAULT_SWIPE): number {
  const axis = dir === 'left' || dir === 'right' ? 'x' : 'y'
  if (dominantAxis(dx, dy, opts.axisRatio) !== axis) return 0
  const v = axis === 'x' ? dx : dy
  const signed = dir === 'left' || dir === 'up' ? -v : v
  return Math.max(0, Math.min(1, signed / opts.distance))
}

/** Direction the drag is currently heading in (for the live overlay), `null` while ambiguous. */
export function liveDirection(dx: number, dy: number, minPx = 8, ratio = DEFAULT_SWIPE.axisRatio): SwipeDir | null {
  if (Math.hypot(dx, dy) < minPx) return null
  const axis = dominantAxis(dx, dy, ratio)
  if (axis === 'x') return dx < 0 ? 'left' : 'right'
  if (axis === 'y') return dy < 0 ? 'up' : 'down'
  return null
}

// ---------------------------------------------------------------- taps

export interface Tap {
  x: number
  y: number
  t: number
}

export const DOUBLE_TAP_MS = 300
export const DOUBLE_TAP_PX = 32
export const TAP_SLOP_PX = 10
export const LONG_PRESS_MS = 480

/** A press that barely moved and was short is a tap. */
export function isTap(dx: number, dy: number, dt: number): boolean {
  return Math.hypot(dx, dy) <= TAP_SLOP_PX && dt < LONG_PRESS_MS
}

/** Feeds a tap into the double-tap detector: returns the new "previous tap" memory and whether this tap completed a double tap. */
export function nextTap(prev: Tap | null, tap: Tap): { prev: Tap | null; double: boolean } {
  if (prev && tap.t - prev.t <= DOUBLE_TAP_MS && Math.hypot(tap.x - prev.x, tap.y - prev.y) <= DOUBLE_TAP_PX) {
    return { prev: null, double: true }
  }
  return { prev: tap, double: false }
}

// ---------------------------------------------------------------- pinch

export const MIN_ZOOM = 1
export const MAX_ZOOM = 5
export const DOUBLE_TAP_ZOOM = 2.5

export function distance(a: { x: number; y: number }, b: { x: number; y: number }): number {
  return Math.hypot(a.x - b.x, a.y - b.y)
}

export function midpoint(a: { x: number; y: number }, b: { x: number; y: number }): { x: number; y: number } {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }
}

/** New zoom after a pinch: `startZoom * cur/start`, clamped. */
export function pinchZoom(startZoom: number, startDist: number, curDist: number, min = MIN_ZOOM, max = MAX_ZOOM): number {
  if (startDist <= 0) return startZoom
  return Math.min(max, Math.max(min, startZoom * (curDist / startDist)))
}

/** Clamp a pan offset so the zoomed image never leaves a gap. `box` = container size. */
export function clampPan(tx: number, ty: number, zoom: number, box: { w: number; h: number }): { tx: number; ty: number } {
  const mx = (box.w * (zoom - 1)) / 2
  const my = (box.h * (zoom - 1)) / 2
  return { tx: Math.min(mx, Math.max(-mx, tx)), ty: Math.min(my, Math.max(-my, ty)) }
}

/** Keeps the point under `focus` (container coords, origin at the container centre) fixed while zoom changes. */
export function zoomAround(tx: number, ty: number, oldZoom: number, newZoom: number, focus: { x: number; y: number }): { tx: number; ty: number } {
  const k = newZoom / oldZoom
  return { tx: focus.x - (focus.x - tx) * k, ty: focus.y - (focus.y - ty) * k }
}
