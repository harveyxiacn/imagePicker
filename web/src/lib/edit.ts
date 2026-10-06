/**
 * Pure edit-stack helpers (reducers). The stack mirrors crates/ip-render (docs/api-contract-m3.md A).
 * Every function returns a new stack and never mutates its input; results are normalised so a
 * photo whose sliders all return to neutral ends up with an empty stack (= "not edited").
 */
import {
  ADJUST_NUMERIC_KEYS,
  EDIT_SECTIONS,
  HSL_BANDS,
  type Adjust,
  type AdjustKey,
  type CropOp,
  type CurveChannel,
  type EditSection,
  type EditStack,
  type GlobalOp,
  type Grading,
  type HslBand,
  type KnownOp,
  type LocalOp,
  type LutOp,
  type MaskRef,
  type Op,
  type OutputSharpenOp,
  type Point,
} from '@/api/types'
import { isIdentityCurve } from './curves'

export const emptyStack = (): EditStack => ({ version: 1, ops: [] })

export const FULL_RECT: CropOp['rect'] = [0, 0, 1, 1]

// ---------------------------------------------------------------- guards

export const isCrop = (o: Op): o is CropOp => o.type === 'crop'
export const isGlobal = (o: Op): o is GlobalOp => o.type === 'global'
export const isLocal = (o: Op): o is LocalOp => o.type === 'local'
export const isLut = (o: Op): o is LutOp => o.type === 'lut'
export const isSharpen = (o: Op): o is OutputSharpenOp => o.type === 'output_sharpen'
export const isKnown = (o: Op): o is KnownOp => EDIT_SECTIONS.includes(o.type as EditSection)

// ---------------------------------------------------------------- ranges

export interface SliderSpec {
  key: AdjustKey
  min: number
  max: number
  step: number
  /** decimals shown in the numeric field */
  decimals: number
}

export const SLIDERS: SliderSpec[] = ADJUST_NUMERIC_KEYS.map((key) => {
  if (key === 'exposure') return { key, min: -5, max: 5, step: 0.05, decimals: 2 }
  if (key === 'temp') return { key, min: -3000, max: 3000, step: 50, decimals: 0 }
  return { key, min: -100, max: 100, step: 1, decimals: 0 }
})
export const SLIDER_SPEC = Object.fromEntries(SLIDERS.map((s) => [s.key, s])) as Record<AdjustKey, SliderSpec>

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v))

// ---------------------------------------------------------------- stable comparison

function stable(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(stable)
  if (v && typeof v === 'object') {
    return Object.fromEntries(
      Object.entries(v as Record<string, unknown>)
        .filter(([, x]) => x !== undefined)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([k, x]) => [k, stable(x)]),
    )
  }
  return v
}

/** Key-order independent serialisation (cache keys, equality, request de-duplication). */
export const stackKey = (s: EditStack): string => JSON.stringify(stable(s))
export const stacksEqual = (a: EditStack, b: EditStack): boolean => stackKey(normalizeStack(a)) === stackKey(normalizeStack(b))

// ---------------------------------------------------------------- normalisation

const near = (a: number, b: number, eps = 1e-4) => Math.abs(a - b) <= eps

export function isFullCrop(c: Pick<CropOp, 'rect' | 'angle'>): boolean {
  return near(c.angle ?? 0, 0, 0.005) && near(c.rect[0], 0) && near(c.rect[1], 0) && near(c.rect[2], 1) && near(c.rect[3], 1)
}

function normalizeAdjust(a: Adjust, keepSource = true): Adjust {
  const out: Adjust = {}
  for (const k of ADJUST_NUMERIC_KEYS) {
    const v = a[k]
    if (typeof v === 'number' && !near(v, 0, 1e-6)) out[k] = v
  }
  if (a.curve) {
    const curve: NonNullable<Adjust['curve']> = {}
    for (const ch of ['rgb', 'r', 'g', 'b'] as CurveChannel[]) {
      const pts = a.curve[ch]
      if (pts && pts.length && !(pts.length <= 2 && isIdentityCurve(pts))) curve[ch] = pts
    }
    if (Object.keys(curve).length) out.curve = curve
  }
  if (a.hsl) {
    const hsl: NonNullable<Adjust['hsl']> = {}
    for (const b of HSL_BANDS) {
      const v = a.hsl[b]
      if (v && (v.h || v.s || v.l)) hsl[b] = { h: v.h ?? 0, s: v.s ?? 0, l: v.l ?? 0 }
    }
    if (Object.keys(hsl).length) out.hsl = hsl
  }
  if (a.grading) {
    const g = a.grading
    const wheels = ['shadows', 'midtones', 'highlights'] as const
    const active = wheels.some((w) => (g[w]?.[1] ?? 0) > 1e-4) || (g.balance ?? 0) !== 0
    if (active)
      out.grading = {
        shadows: g.shadows ?? [0, 0],
        midtones: g.midtones ?? [0, 0],
        highlights: g.highlights ?? [0, 0],
        balance: g.balance ?? 0,
      }
  }
  if (keepSource && a.source) out.source = a.source
  return out
}

const hasAdjustance = (a: Adjust) => Object.keys(a).some((k) => k !== 'source')

/** Drop neutral ops so "all sliders at 0" == the empty stack. Unknown ops are preserved verbatim. */
export function normalizeStack(stack: EditStack): EditStack {
  const ops: Op[] = []
  for (const op of stack.ops) {
    if (isCrop(op)) {
      if (!isFullCrop(op)) ops.push(op)
    } else if (isGlobal(op)) {
      const { type: _t, ...rest } = op
      void _t
      const adj = normalizeAdjust(rest)
      if (hasAdjustance(adj)) ops.push({ type: 'global', ...adj })
    } else if (isSharpen(op)) {
      if (op.amount > 0) ops.push(op)
    } else if (isLut(op)) {
      if (op.file && (op.amount ?? 1) > 0) ops.push(op)
    } else if (isLocal(op)) {
      ops.push({ ...op, adjust: normalizeAdjust(op.adjust, false) })
    } else ops.push(op)
  }
  return { version: stack.version ?? 1, ops }
}

export const isEmptyStack = (s: EditStack): boolean => normalizeStack(s).ops.every((o) => !isKnown(o))

// ---------------------------------------------------------------- ordering

const RANK: Record<string, number> = { crop: 0, global: 1, local: 3, lut: 4, output_sharpen: 5 }
const rankOf = (o: Op) => RANK[o.type] ?? 2

export function orderOps(ops: Op[]): Op[] {
  return ops
    .map((o, i) => ({ o, i }))
    .sort((a, b) => rankOf(a.o) - rankOf(b.o) || a.i - b.i)
    .map((x) => x.o)
}

const withOps = (s: EditStack, ops: Op[]): EditStack => normalizeStack({ version: s.version ?? 1, ops: orderOps(ops) })

// ---------------------------------------------------------------- global

export const getGlobal = (s: EditStack): GlobalOp | undefined => s.ops.find(isGlobal)
export const getAdjust = (s: EditStack): Adjust => {
  const g = getGlobal(s)
  if (!g) return {}
  const { type: _t, ...rest } = g
  void _t
  return rest
}

/** Apply `fn` to the (first) global op, creating it when absent. */
export function updateGlobal(s: EditStack, fn: (a: Adjust) => Adjust): EditStack {
  const idx = s.ops.findIndex(isGlobal)
  if (idx < 0) return withOps(s, [...s.ops, { type: 'global', ...fn({}) }])
  const cur = s.ops[idx] as GlobalOp
  const { type: _t, ...rest } = cur
  void _t
  const ops = [...s.ops]
  ops[idx] = { type: 'global', ...fn(rest) }
  return withOps(s, ops)
}

export const numberField = (a: Adjust | undefined, k: AdjustKey): number => a?.[k] ?? 0

/** Set one slider on an Adjust (clamped to its range). */
export function setField(a: Adjust, key: AdjustKey, value: number): Adjust {
  const spec = SLIDER_SPEC[key]
  return { ...a, [key]: clamp(value, spec.min, spec.max) }
}

export const setGlobalField = (s: EditStack, key: AdjustKey, value: number): EditStack =>
  updateGlobal(s, (a) => setField(a, key, value))

export function setCurve(a: Adjust, ch: CurveChannel, points: Point[]): Adjust {
  return { ...a, curve: { ...a.curve, [ch]: points } }
}

export function setHsl(a: Adjust, band: HslBand, ch: 'h' | 's' | 'l', value: number): Adjust {
  const cur = a.hsl?.[band] ?? {}
  return { ...a, hsl: { ...a.hsl, [band]: { h: 0, s: 0, l: 0, ...cur, [ch]: clamp(value, -100, 100) } } }
}

export const getHsl = (a: Adjust, band: HslBand) => ({ h: 0, s: 0, l: 0, ...a.hsl?.[band] })

export type Wheel = 'shadows' | 'midtones' | 'highlights'
export function setGradingWheel(a: Adjust, wheel: Wheel, hue: number, amount: number): Adjust {
  const g: Partial<Grading> = a.grading ?? {}
  return { ...a, grading: { ...g, [wheel]: [((hue % 360) + 360) % 360, clamp(amount, 0, 1)] as Point } }
}
export function setGradingBalance(a: Adjust, balance: number): Adjust {
  return { ...a, grading: { ...a.grading, balance: clamp(balance, -100, 100) } }
}

// ---------------------------------------------------------------- AI auto merge + diff

export interface AdjustDiff {
  key: AdjustKey
  from: number
  to: number
}

export function diffAdjust(before: Adjust | undefined, after: Adjust | undefined): AdjustDiff[] {
  const out: AdjustDiff[] = []
  for (const key of ADJUST_NUMERIC_KEYS) {
    const from = before?.[key] ?? 0
    const to = after?.[key] ?? 0
    if (!near(from, to, 1e-6)) out.push({ key, from, to })
  }
  return out
}

/** Merge AI-suggested values into the global op: provided numeric fields replace the current ones. */
export function mergeAuto(s: EditStack, auto: Adjust): EditStack {
  return updateGlobal(s, (a) => {
    const next: Adjust = { ...a }
    for (const k of ADJUST_NUMERIC_KEYS) {
      const v = auto[k]
      if (typeof v === 'number') next[k] = clamp(v, SLIDER_SPEC[k].min, SLIDER_SPEC[k].max)
    }
    if (auto.curve) next.curve = auto.curve
    if (auto.hsl) next.hsl = auto.hsl
    if (auto.grading) next.grading = auto.grading
    next.source = auto.source ?? 'ai_auto@1'
    return next
  })
}

// ---------------------------------------------------------------- local adjustments

export interface LocalEntry {
  op: LocalOp
  /** index within `stack.ops` */
  opIndex: number
  /** index among local ops (stable identity for the UI) */
  index: number
}

export function localOps(s: EditStack): LocalEntry[] {
  const out: LocalEntry[] = []
  s.ops.forEach((op, opIndex) => {
    if (isLocal(op)) out.push({ op, opIndex, index: out.length })
  })
  return out
}

export function maskLabelKey(m: MaskRef): string {
  return m.kind === 'ai' ? `edit.target_${m.target}` : `edit.mask_${m.kind}`
}

export function defaultMask(kind: 'radial' | 'linear'): MaskRef {
  return kind === 'radial'
    ? { kind: 'radial', center: [0.5, 0.5], radius: [0.3, 0.3], feather: 0.5 }
    : { kind: 'linear', start: [0.5, 0.25], end: [0.5, 0.6] }
}

export function addLocal(s: EditStack, mask: MaskRef, adjust: Adjust = {}): EditStack {
  const op: LocalOp = { type: 'local', mask, amount: 1, invert: false, adjust }
  return withOps(s, [...s.ops, op])
}

export function removeLocal(s: EditStack, index: number): EditStack {
  const entry = localOps(s)[index]
  if (!entry) return s
  return withOps(
    s,
    s.ops.filter((_, i) => i !== entry.opIndex),
  )
}

export function updateLocal(s: EditStack, index: number, fn: (op: LocalOp) => LocalOp): EditStack {
  const entry = localOps(s)[index]
  if (!entry) return s
  const ops = [...s.ops]
  ops[entry.opIndex] = fn(entry.op)
  return withOps(s, ops)
}

export const setLocalField = (s: EditStack, index: number, key: AdjustKey, value: number): EditStack =>
  updateLocal(s, index, (op) => ({ ...op, adjust: setField(op.adjust, key, value) }))

// ---------------------------------------------------------------- crop

export const getCrop = (s: EditStack): CropOp | undefined => s.ops.find(isCrop)

const MIN_CROP = 0.04

/** Clamp a crop rect into the unit square with a minimum size. */
export function normalizeCropRect(rect: readonly number[]): CropOp['rect'] {
  let [x, y, w, h] = rect
  w = clamp(Number.isFinite(w) ? w : 1, MIN_CROP, 1)
  h = clamp(Number.isFinite(h) ? h : 1, MIN_CROP, 1)
  x = clamp(Number.isFinite(x) ? x : 0, 0, 1 - w)
  y = clamp(Number.isFinite(y) ? y : 0, 0, 1 - h)
  return [x, y, w, h]
}

export function setCrop(s: EditStack, crop: Partial<Omit<CropOp, 'type'>> | null): EditStack {
  const rest = s.ops.filter((o) => !isCrop(o))
  if (!crop) return withOps(s, rest)
  const cur = getCrop(s)
  const next: CropOp = {
    type: 'crop',
    rect: normalizeCropRect(crop.rect ?? cur?.rect ?? FULL_RECT),
    angle: clamp(crop.angle ?? cur?.angle ?? 0, -45, 45),
  }
  const aspect = 'aspect' in crop ? crop.aspect : cur?.aspect
  if (aspect) next.aspect = aspect
  return withOps(s, [...rest, next])
}

export const ASPECTS = ['original', '1:1', '4:5', '3:4', '16:9', 'free'] as const
export type AspectId = (typeof ASPECTS)[number]

/** Width/height pixel ratio for an aspect id; `original` -> image ratio, `free` -> null. Portrait images flip fixed ratios. */
export function aspectRatio(id: string | undefined, imgAspect: number): number | null {
  if (!id || id === 'free') return null
  if (id === 'original') return imgAspect
  const m = /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.exec(id)
  if (!m) return null
  const r = Number(m[1]) / Number(m[2])
  return imgAspect >= 1 ? Math.max(r, 1 / r) : Math.min(r, 1 / r)
}

/** Largest rect of pixel ratio `ratio` that fits inside `rect`, keeping its centre. */
export function fitAspect(rect: CropOp['rect'], ratio: number, imgAspect: number): CropOp['rect'] {
  // normalised w/h that yields the pixel ratio: (w*imgW)/(h*imgH) = ratio
  const k = ratio / imgAspect
  const cx = rect[0] + rect[2] / 2
  const cy = rect[1] + rect[3] / 2
  let w = rect[2]
  let h = w / k
  if (h > rect[3]) {
    h = rect[3]
    w = h * k
  }
  return normalizeCropRect([cx - w / 2, cy - h / 2, w, h])
}

export type CropHandle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w' | 'move'

/**
 * Drag a crop handle by (dx, dy) in normalised units. With `ratio` set (pixel w/h) the aspect is kept;
 * `imgAspect` converts between pixel and normalised ratios.
 */
export function dragCropRect(
  start: CropOp['rect'],
  handle: CropHandle,
  dx: number,
  dy: number,
  ratio: number | null,
  imgAspect: number,
): CropOp['rect'] {
  const [x, y, w, h] = start
  if (handle === 'move') return normalizeCropRect([clamp(x + dx, 0, 1 - w), clamp(y + dy, 0, 1 - h), w, h])
  let x0 = x
  let y0 = y
  let x1 = x + w
  let y1 = y + h
  if (handle.includes('w')) x0 = clamp(x + dx, 0, x1 - MIN_CROP)
  if (handle.includes('e')) x1 = clamp(x + w + dx, x0 + MIN_CROP, 1)
  if (handle.includes('n')) y0 = clamp(y + dy, 0, y1 - MIN_CROP)
  if (handle.includes('s')) y1 = clamp(y + h + dy, y0 + MIN_CROP, 1)
  if (!ratio) return normalizeCropRect([x0, y0, x1 - x0, y1 - y0])

  const k = ratio / imgAspect // normalised w/h
  const west = handle.includes('w')
  const east = handle.includes('e')
  const north = handle.includes('n')
  const south = handle.includes('s')
  let nw = x1 - x0
  let nh = y1 - y0
  if ((west || east) && !(north || south)) nh = nw / k
  else if ((north || south) && !(west || east)) nw = nh * k
  else if (Math.abs(dx) >= Math.abs(dy) * k) nh = nw / k
  else nw = nh * k
  // The opposite edge/corner stays put; edge handles keep the other axis centred.
  const anchorX = west ? x + w : east ? x : x + w / 2
  const anchorY = north ? y + h : south ? y : y + h / 2
  const maxW = west ? anchorX : east ? 1 - anchorX : 2 * Math.min(anchorX, 1 - anchorX)
  const maxH = north ? anchorY : south ? 1 - anchorY : 2 * Math.min(anchorY, 1 - anchorY)
  if (nw > maxW) {
    nw = maxW
    nh = nw / k
  }
  if (nh > maxH) {
    nh = maxH
    nw = nh * k
  }
  nw = Math.max(nw, MIN_CROP)
  nh = nw / k
  const rx = west ? anchorX - nw : east ? anchorX : anchorX - nw / 2
  const ry = north ? anchorY - nh : south ? anchorY : anchorY - nh / 2
  return normalizeCropRect([rx, ry, nw, nh])
}

// ---------------------------------------------------------------- lut / sharpen

export const getLut = (s: EditStack): LutOp | undefined => s.ops.find(isLut)
export function setLut(s: EditStack, lut: { file: string; amount?: number } | null): EditStack {
  const rest = s.ops.filter((o) => !isLut(o))
  return withOps(s, lut ? [...rest, { type: 'lut', file: lut.file, amount: lut.amount ?? 1 }] : rest)
}
export const getSharpen = (s: EditStack): number => s.ops.find(isSharpen)?.amount ?? 0
export function setSharpen(s: EditStack, amount: number): EditStack {
  const rest = s.ops.filter((o) => !isSharpen(o))
  return withOps(s, [...rest, { type: 'output_sharpen', amount: clamp(amount, 0, 100) }])
}

// ---------------------------------------------------------------- sections (copy / paste / presets)

const opSection = (o: Op): EditSection | null => (isKnown(o) ? (o.type as EditSection) : null)

/** The ops of `s` that belong to one of `sections` (unknown ops are never copied). */
export function extractSections(s: EditStack, sections: EditSection[]): EditStack {
  return normalizeStack({
    version: s.version ?? 1,
    ops: s.ops.filter((o) => {
      const k = opSection(o)
      return k !== null && sections.includes(k)
    }),
  })
}

/** Replace `sections` of `target` with those of `source` (sections absent in source are cleared). */
export function applySections(target: EditStack, source: EditStack, sections: EditSection[]): EditStack {
  const keep = target.ops.filter((o) => {
    const k = opSection(o)
    return k === null || !sections.includes(k)
  })
  const add = source.ops.filter((o) => {
    const k = opSection(o)
    return k !== null && sections.includes(k)
  })
  return withOps(target, [...keep, ...add])
}

export const PRESET_SECTIONS: EditSection[] = ['global', 'local', 'lut', 'output_sharpen']
export const applyPreset = (s: EditStack, preset: EditStack): EditStack => applySections(s, preset, PRESET_SECTIONS)

/** Sections that actually carry data in the stack. */
export function sectionsPresent(s: EditStack): EditSection[] {
  const set = new Set<EditSection>()
  for (const o of normalizeStack(s).ops) {
    const k = opSection(o)
    if (k) set.add(k)
  }
  return EDIT_SECTIONS.filter((k) => set.has(k))
}

// ---------------------------------------------------------------- preview requests

/** Stack rendered while the crop tool is open: full frame, only the straighten angle applied. */
export function cropToolStack(s: EditStack): EditStack {
  const c = getCrop(s)
  const rest = s.ops.filter((o) => !isCrop(o))
  const ops: Op[] = c && Math.abs(c.angle) > 0.005 ? [{ type: 'crop', rect: [...FULL_RECT], angle: c.angle }, ...rest] : rest
  return { version: s.version ?? 1, ops }
}

/**
 * Request body for the "before" image: the original, but keeping geometry (crop) so before/after
 * line up. Without a crop this is exactly `{ original: true }` (contract B, /api/render/preview).
 */
export function beforeRequest(s: EditStack): { original?: true; stack?: EditStack } {
  const crop = getCrop(s)
  return crop ? { stack: { version: s.version ?? 1, ops: [crop] } } : { original: true }
}
