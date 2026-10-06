/**
 * Brush stroke capture for "画笔消除" (docs/api-contract-m5.md C, `POST /api/photos/{id}/inpaint`).
 *
 * Coordinate spaces
 *  - container px : relative to the zoom pane (what pointer events give us)
 *  - frame (u, v) : normalised position inside the rendered preview frame, i.e. AFTER crop / straighten
 *  - source (x, y): normalised upright image, BEFORE crop: what the API wants (contract: "正向裁剪前坐标")
 * `radius` is a fraction of the source image WIDTH (documented choice: the contract only says "normalised").
 */
import type { CropOp, Point, Stroke } from '@/api/types'
import { pointToNorm, type Size, type ViewState } from './zoom'

export const MIN_RADIUS = 0.004
export const MAX_RADIUS = 0.12
export const DEFAULT_RADIUS = 0.025
/** Minimum spacing between recorded points, as a fraction of the radius (keeps the payload small). */
const SPACING = 0.35

const clamp01 = (v: number) => Math.min(1, Math.max(0, v))
/** 4 decimals: no f32->f64 noise on the wire (contract D). */
const r4 = (v: number) => Math.round(v * 1e4) / 1e4

type CropLike = Pick<CropOp, 'rect'> & Partial<Pick<CropOp, 'angle'>>
const FULL: CropLike = { rect: [0, 0, 1, 1], angle: 0 }

/** Frame (u, v) -> source (x, y). `aspect` = image width / height (needed to rotate in pixel space). */
export function frameToSource(u: number, v: number, crop: CropLike | undefined, aspect: number): Point {
  const c = crop ?? FULL
  const [rx, ry, rw, rh] = c.rect
  let x = rx + u * rw
  let y = ry + v * rh
  const ang = ((c.angle ?? 0) * Math.PI) / 180
  if (Math.abs(ang) > 1e-6) {
    // content was rotated counter-clockwise by `angle` about the centre: undo it in pixel units
    const X = (x - 0.5) * aspect
    const Y = y - 0.5
    const cos = Math.cos(ang)
    const sin = Math.sin(ang)
    x = (X * cos - Y * sin) / aspect + 0.5
    y = X * sin + Y * cos + 0.5
  }
  return [clamp01(x), clamp01(y)]
}

/** Source (x, y) -> frame (u, v): inverse of `frameToSource` (not clamped: off-frame points land outside 0..1). */
export function sourceToFrame(x: number, y: number, crop: CropLike | undefined, aspect: number): Point {
  const c = crop ?? FULL
  const [rx, ry, rw, rh] = c.rect
  let px = x
  let py = y
  const ang = ((c.angle ?? 0) * Math.PI) / 180
  if (Math.abs(ang) > 1e-6) {
    const X = (px - 0.5) * aspect
    const Y = py - 0.5
    const cos = Math.cos(ang)
    const sin = Math.sin(ang)
    px = (X * cos + Y * sin) / aspect + 0.5
    py = -X * sin + Y * cos + 0.5
  }
  return [(px - rx) / rw, (py - ry) / rh]
}

/** Pointer position (container px) -> frame (u, v), honouring zoom and pan of the pane. */
export function containerToFrame(view: ViewState, container: Size, frame: Size, px: number, py: number): Point {
  const n = pointToNorm(view, container, frame, px, py)
  return [n.nx, n.ny]
}

/** Pointer position -> normalised source coordinates (zoom + pan + crop + straighten). */
export function containerToSource(view: ViewState, container: Size, frame: Size, crop: CropLike | undefined, aspect: number, px: number, py: number): Point {
  const [u, v] = containerToFrame(view, container, frame, px, py)
  return frameToSource(u, v, crop, aspect)
}

/** Pointer position -> frame (u, v) from the on-screen rectangle of the image (DOM-measured equivalent of `containerToFrame`). */
export function rectToFrame(rect: { left: number; top: number; width: number; height: number }, clientX: number, clientY: number): Point {
  return [(clientX - rect.left) / rect.width, (clientY - rect.top) / rect.height]
}

/** Brush radius (source-width fraction) -> CSS px on a frame displayed `displayWidth` px wide. */
export const radiusToDisplay = (radius: number, crop: CropLike | undefined, displayWidth: number): number => (radius / (crop ?? FULL).rect[2]) * displayWidth

export const clampRadius = (r: number): number => Math.min(MAX_RADIUS, Math.max(MIN_RADIUS, r))

/** Append `p` unless it is closer than a fraction of the radius to the last point. */
export function appendPoint(points: Point[], p: Point, radius: number, aspect: number): Point[] {
  const last = points[points.length - 1]
  if (last) {
    const dx = (p[0] - last[0]) * aspect
    const dy = p[1] - last[1]
    if (Math.hypot(dx, dy) < radius * SPACING) return points
  }
  return [...points, p]
}

/** Finish a stroke: rounds coordinates; a click without movement stays a single dot. */
export function finishStroke(points: Point[], radius: number): Stroke | null {
  if (points.length === 0) return null
  return { points: points.map(([x, y]) => [r4(x), r4(y)] as Point), radius: r4(clampRadius(radius)) }
}

/** Bounding box of all stroke discs, normalised, clamped to the image. */
export function strokesBounds(strokes: Stroke[], aspect: number): [number, number, number, number] | null {
  if (!strokes.length) return null
  let x0 = 1
  let y0 = 1
  let x1 = 0
  let y1 = 0
  for (const s of strokes) {
    const ry = s.radius * aspect // radius is a fraction of width: in y units it is radius * (W / H)
    for (const [x, y] of s.points) {
      x0 = Math.min(x0, x - s.radius)
      x1 = Math.max(x1, x + s.radius)
      y0 = Math.min(y0, y - ry)
      y1 = Math.max(y1, y + ry)
    }
  }
  x0 = clamp01(x0)
  y0 = clamp01(y0)
  x1 = clamp01(x1)
  y1 = clamp01(y1)
  return [x0, y0, x1 - x0, y1 - y0]
}

/** Drop empty strokes; the API body for `{strokes}`. */
export const strokesBody = (strokes: Stroke[]): { strokes: Stroke[] } => ({ strokes: strokes.filter((s) => s.points.length > 0) })
