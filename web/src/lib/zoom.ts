/**
 * Zoom/pan maths. The view is expressed relative to the "fit" scale so it can be
 * shared between images of different pixel sizes (compare mode sync):
 *   zoom = 1 -> image fits the container; cx/cy = normalized image point at the container centre.
 */
export interface ViewState {
  zoom: number
  cx: number
  cy: number
}

export const FIT_VIEW: ViewState = { zoom: 1, cx: 0.5, cy: 0.5 }

export interface Size {
  w: number
  h: number
}

/** CSS px per image px when fitted. */
export function fitScale(c: Size, img: Size): number {
  if (c.w <= 0 || c.h <= 0 || img.w <= 0 || img.h <= 0) return 1
  return Math.min(c.w / img.w, c.h / img.h)
}

/** `zoom` value that corresponds to 100% (1 image px = 1 CSS px). */
export function hundredZoom(c: Size, img: Size): number {
  return 1 / fitScale(c, img)
}

export function maxZoom(c: Size, img: Size): number {
  return Math.max(2, hundredZoom(c, img) * 4)
}

export function clampView(v: ViewState, c: Size, img: Size): ViewState {
  const zoom = Math.min(maxZoom(c, img), Math.max(1, v.zoom))
  const s = fitScale(c, img) * zoom
  const dw = img.w * s
  const dh = img.h * s
  const clampAxis = (n: number, container: number, display: number) => {
    if (display <= container) return 0.5
    const half = container / (2 * display)
    return Math.min(1 - half, Math.max(half, n))
  }
  return { zoom, cx: clampAxis(v.cx, c.w, dw), cy: clampAxis(v.cy, c.h, dh) }
}

export interface Rect {
  left: number
  top: number
  width: number
  height: number
}

export function imageRect(v: ViewState, c: Size, img: Size): Rect {
  const s = fitScale(c, img) * v.zoom
  const width = img.w * s
  const height = img.h * s
  return { left: c.w / 2 - v.cx * width, top: c.h / 2 - v.cy * height, width, height }
}

/** Normalized image coordinates under container point (px, py). */
export function pointToNorm(v: ViewState, c: Size, img: Size, px: number, py: number) {
  const r = imageRect(v, c, img)
  return { nx: (px - r.left) / r.width, ny: (py - r.top) / r.height }
}

/** Multiply zoom by `factor`, keeping the image point under (px, py) fixed. */
export function zoomAt(v: ViewState, factor: number, px: number, py: number, c: Size, img: Size): ViewState {
  const before = pointToNorm(v, c, img, px, py)
  const zoom = Math.min(maxZoom(c, img), Math.max(1, v.zoom * factor))
  const s = fitScale(c, img) * zoom
  const width = img.w * s
  const height = img.h * s
  // want: px = c.w/2 - cx*width + nx*width  =>  cx = nx - (px - c.w/2)/width
  const next = { zoom, cx: before.nx - (px - c.w / 2) / width, cy: before.ny - (py - c.h / 2) / height }
  return clampView(next, c, img)
}

export function panBy(v: ViewState, dx: number, dy: number, c: Size, img: Size): ViewState {
  const r = imageRect(v, c, img)
  return clampView({ ...v, cx: v.cx - dx / r.width, cy: v.cy - dy / r.height }, c, img)
}

/** View showing the given normalized point at 100%. */
export function viewAt100(nx: number, ny: number, c: Size, img: Size): ViewState {
  return clampView({ zoom: hundredZoom(c, img), cx: nx, cy: ny }, c, img)
}

/** Toggle between fit and 100% (centered on the optional point). */
export function toggleFit100(v: ViewState, c: Size, img: Size, px?: number, py?: number): ViewState {
  if (v.zoom > 1.01) return FIT_VIEW
  const p = px !== undefined && py !== undefined ? pointToNorm(v, c, img, px, py) : { nx: 0.5, ny: 0.5 }
  return viewAt100(p.nx, p.ny, c, img)
}
