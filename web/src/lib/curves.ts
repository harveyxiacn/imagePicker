import type { Point } from '@/api/types'

/** Default (identity) control points shown in the editor when a channel has none. */
export const IDENTITY_CURVE: Point[] = [
  [0, 0],
  [1, 1],
]

const clamp01 = (v: number) => Math.min(1, Math.max(0, v))

export function sortPoints(points: Point[]): Point[] {
  return [...points].sort((a, b) => a[0] - b[0])
}

/** True when the curve maps every x to itself (or has no control points). */
export function isIdentityCurve(points: Point[] | undefined): boolean {
  return !points || points.length === 0 || points.every(([x, y]) => Math.abs(x - y) < 1e-4)
}

/**
 * Monotone cubic (Fritsch-Carlson) interpolation through control points (x ascending, 0..1).
 * Returns a function y(x); flat extrapolation outside the first/last point, identity for no points.
 */
export function curveFn(points: Point[] | undefined): (x: number) => number {
  const pts = sortPoints(points && points.length ? points : IDENTITY_CURVE)
  const n = pts.length
  if (n === 1) return () => clamp01(pts[0][1])
  const dx: number[] = []
  const slope: number[] = []
  for (let i = 0; i < n - 1; i++) {
    const d = Math.max(1e-6, pts[i + 1][0] - pts[i][0])
    dx.push(d)
    slope.push((pts[i + 1][1] - pts[i][1]) / d)
  }
  const m: number[] = new Array<number>(n)
  m[0] = slope[0]
  m[n - 1] = slope[n - 2]
  for (let i = 1; i < n - 1; i++) m[i] = slope[i - 1] * slope[i] <= 0 ? 0 : (slope[i - 1] + slope[i]) / 2
  // Fritsch-Carlson limiter keeps the curve monotone between points.
  for (let i = 0; i < n - 1; i++) {
    if (Math.abs(slope[i]) < 1e-9) {
      m[i] = 0
      m[i + 1] = 0
      continue
    }
    const a = m[i] / slope[i]
    const b = m[i + 1] / slope[i]
    const s = a * a + b * b
    if (s > 9) {
      const tau = 3 / Math.sqrt(s)
      m[i] = tau * a * slope[i]
      m[i + 1] = tau * b * slope[i]
    }
  }
  return (x: number) => {
    if (x <= pts[0][0]) return clamp01(pts[0][1])
    if (x >= pts[n - 1][0]) return clamp01(pts[n - 1][1])
    let i = 0
    while (i < n - 2 && x > pts[i + 1][0]) i++
    const h = dx[i]
    const t = (x - pts[i][0]) / h
    const t2 = t * t
    const t3 = t2 * t
    const y =
      (2 * t3 - 3 * t2 + 1) * pts[i][1] +
      (t3 - 2 * t2 + t) * h * m[i] +
      (-2 * t3 + 3 * t2) * pts[i + 1][1] +
      (t3 - t2) * h * m[i + 1]
    return clamp01(y)
  }
}

/** Sample the curve at `n` evenly spaced x values in [0, 1] (for drawing and LUT building). */
export function sampleCurve(points: Point[] | undefined, n: number): number[] {
  const f = curveFn(points)
  const out: number[] = []
  for (let i = 0; i < n; i++) out.push(f(i / (n - 1)))
  return out
}

/** SVG path (`M..L..`) of the curve in a unit box scaled to `w` x `h` (y up). */
export function curvePath(points: Point[] | undefined, w: number, h: number, steps = 64): string {
  const ys = sampleCurve(points, steps + 1)
  return ys.map((y, i) => `${i === 0 ? 'M' : 'L'}${((i / steps) * w).toFixed(2)} ${((1 - y) * h).toFixed(2)}`).join(' ')
}

const MIN_GAP = 0.01

/** Insert a control point (x snapped away from neighbours). Returns the new list and the point's index. */
export function addCurvePoint(points: Point[], x: number, y: number): { points: Point[]; index: number } {
  const base = sortPoints(points.length ? points : IDENTITY_CURVE)
  const px = clamp01(x)
  const near = base.findIndex((p) => Math.abs(p[0] - px) < MIN_GAP)
  if (near >= 0) return { points: base, index: near }
  const next = sortPoints([...base, [px, clamp01(y)] as Point])
  return { points: next, index: next.findIndex((p) => p[0] === px) }
}

/** Move point `i`; x stays between its neighbours, the two end points keep their x. */
export function moveCurvePoint(points: Point[], i: number, x: number, y: number): Point[] {
  const pts = points.map((p) => [...p] as Point)
  if (i < 0 || i >= pts.length) return points
  const last = pts.length - 1
  const lo = i === 0 ? 0 : pts[i - 1][0] + MIN_GAP
  const hi = i === last ? 1 : pts[i + 1][0] - MIN_GAP
  const keepX = i === 0 || i === last
  pts[i] = [keepX ? pts[i][0] : Math.min(hi, Math.max(lo, x)), clamp01(y)]
  return pts
}

/** Remove an interior point; the first and last point cannot be removed. */
export function removeCurvePoint(points: Point[], i: number): Point[] {
  if (i <= 0 || i >= points.length - 1) return points
  return points.filter((_, k) => k !== i)
}

/** Index of the control point nearest to (x, y) within `radius` (unit-box distance), or -1. */
export function hitCurvePoint(points: Point[], x: number, y: number, radius: number): number {
  let best = -1
  let bestD = radius
  points.forEach(([px, py], i) => {
    const d = Math.hypot(px - x, py - y)
    if (d <= bestD) {
      bestD = d
      best = i
    }
  })
  return best
}
