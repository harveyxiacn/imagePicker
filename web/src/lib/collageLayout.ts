/** Collage layouts (PRD F9.5). A pure mirror of `compute_layout` in `crates/ip-core/src/collage.rs`:
 *  the dialog's live preview and the server tile the canvas with the same integer rectangles, and
 *  both test suites assert the same golden rectangles, so change them together. Integer pixels;
 *  neighbours are exactly `gap` apart; the cells cover the area inside `margin` completely. */

export type CollageLayoutKind = 'grid' | 'story' | 'contact'
export type CollageFit = 'cover' | 'contain'

export interface Rect {
  x: number
  y: number
  w: number
  h: number
}

/** One photo's place: its picture and, on contact sheets, the caption band under it. */
export interface Cell {
  photo: Rect
  caption: Rect | null
}

export const MIN_PHOTOS = 2
export const MAX_PHOTOS = 30
export const MIN_SIDE = 64
export const MAX_SIDE = 12_000
export const MAX_PIXELS = 80_000_000
export const MAX_SPACING = 2_000
/** Aspect ratio (w / h) assumed for photos whose size is not known. */
export const DEFAULT_ASPECT = 1.5

export interface CanvasPreset {
  id: string
  width: number
  height: number
}

/** Canvas size presets; `custom` is the free entry. */
export const CANVAS_PRESETS: CanvasPreset[] = [
  { id: '4:5', width: 1080, height: 1350 },
  { id: '1:1', width: 1080, height: 1080 },
  { id: '16:9', width: 3840, height: 2160 },
  { id: 'a4', width: 2480, height: 3508 },
]

/** Height of the caption band of a contact sheet. */
export function captionHeight(width: number, height: number): number {
  return Math.max(Math.floor(Math.min(width, height) / 40), 14)
}

/** `total` pixels cut into `parts` pieces `gap` apart: `[offset, length]`, the first ones one pixel longer. */
function split(total: number, parts: number, gap: number): [number, number][] | null {
  const avail = total - (parts - 1) * gap
  if (avail < 0) return null
  const base = Math.floor(avail / parts)
  const rem = avail % parts
  if (base === 0) return null
  const out: [number, number][] = []
  let at = 0
  for (let i = 0; i < parts; i++) {
    const len = base + (i < rem ? 1 : 0)
    out.push([at, len])
    at += len + gap
  }
  return out
}

/** `r` rows of `n / r` (or one more) cells, each row stretched over the full width; the `r` that
 *  fits the aspect ratios best (least total stretch, fewer rows on a tie). */
function gridIn(area: Rect, aspects: number[], gap: number, caption: number): Rect[] | null {
  const n = aspects.length
  let best: { cost: number; cells: Rect[] } | null = null
  for (let r = 1; r <= n; r++) {
    const heights = split(area.h, r, gap)
    if (!heights) break
    const base = Math.floor(n / r)
    const extra = n % r
    const cells: Rect[] = []
    let cost = 0
    let idx = 0
    let ok = true
    for (let k = 0; k < r && ok; k++) {
      const [y, h] = heights[k]
      if (h <= caption) {
        ok = false
        break
      }
      const widths = split(area.w, base + (k < extra ? 1 : 0), gap)
      if (!widths) {
        ok = false
        break
      }
      for (const [x, w] of widths) {
        const q = w / (h - caption) / Math.max(aspects[idx], 1e-6)
        cost += Math.max(q, 1 / q)
        idx++
        cells.push({ x: area.x + x, y: area.y + y, w, h })
      }
    }
    if (ok && (best === null || cost < best.cost)) best = { cost, cells }
  }
  return best ? best.cells : null
}

/** The cells of a collage of `aspects.length` photos (width / height each, in order) on a
 *  `width` x `height` canvas, or `null` when the canvas is too small for them. */
export function computeLayout(kind: CollageLayoutKind, width: number, height: number, gap: number, margin: number, caption: number, aspects: number[]): Cell[] | null {
  const n = aspects.length
  if (n === 0) return []
  const iw = width - margin * 2
  const ih = height - margin * 2
  if (iw <= 0 || ih <= 0) return null
  const area: Rect = { x: margin, y: margin, w: iw, h: ih }
  const plain = (r: Rect): Cell => ({ photo: r, caption: null })
  if (kind === 'grid') return gridIn(area, aspects, gap, 0)?.map(plain) ?? null
  if (kind === 'contact') {
    const cells = gridIn(area, aspects, gap, caption)
    if (!cells) return null
    return cells.map((r) => (caption === 0 ? plain(r) : { photo: { ...r, h: r.h - caption }, caption: { ...r, y: r.y + r.h - caption, h: caption } }))
  }
  if (n === 1) return [plain(area)]
  const [num, den] = n - 1 <= 4 ? [2, 3] : [1, 2]
  const landscape = iw >= ih
  const avail = (landscape ? iw : ih) - gap
  if (avail < 0) return null
  const heroLen = Math.floor((avail * num) / den)
  const restLen = avail - heroLen
  if (heroLen === 0 || restLen === 0) return null
  const hero: Rect = landscape ? { ...area, w: heroLen } : { ...area, h: heroLen }
  const rest: Rect = landscape ? { ...area, x: area.x + heroLen + gap, w: restLen } : { ...area, y: area.y + heroLen + gap, h: restLen }
  const small = gridIn(rest, aspects.slice(1), gap, 0)
  return small ? [hero, ...small].map(plain) : null
}

/** Aspect ratio of a photo from its stored size. */
export function aspectOfPhoto(p: { width: number | null; height: number | null }): number {
  return p.width && p.height ? p.width / p.height : DEFAULT_ASPECT
}

/** Why a collage cannot be exported with these settings (an i18n key), or `null` when it can. */
export function collageProblem(count: number, width: number, height: number, gap: number, margin: number, cells: Cell[] | null): 'count' | 'size' | 'spacing' | 'tooSmall' | null {
  if (count < MIN_PHOTOS || count > MAX_PHOTOS) return 'count'
  if (![width, height].every((v) => Number.isInteger(v) && v >= MIN_SIDE && v <= MAX_SIDE) || width * height > MAX_PIXELS) return 'size'
  if (![gap, margin].every((v) => Number.isInteger(v) && v >= 0 && v <= MAX_SPACING)) return 'spacing'
  return cells === null ? 'tooSmall' : null
}
