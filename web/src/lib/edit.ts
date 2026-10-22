/**
 * Pure edit-stack helpers (reducers). The stack mirrors crates/ip-render (docs/api-contract-m3.md A).
 * Every function returns a new stack and never mutates its input; results are normalised so a
 * photo whose sliders all return to neutral ends up with an empty stack (= "not edited").
 */
import {
  ADJUST_NUMERIC_KEYS,
  EDIT_SECTIONS,
  HSL_BANDS,
  ONE_WAY_KEYS,
  type Adjust,
  type AdjustKey,
  type CropOp,
  type CurveChannel,
  type EditSection,
  type DetailOp,
  type EditStack,
  type EffectsOp,
  type GlobalOp,
  type Grading,
  type Grain,
  type HslBand,
  type KnownOp,
  type LocalOp,
  type ColorRange,
  type LutOp,
  type LuminanceRange,
  type MaskRef,
  type MaskRange,
  type NoiseReduction,
  type Op,
  type OutputSharpenOp,
  type SharpenDetail,
  type Vignette,
  type BeautyOp,
  type WarpBodyOp,
  type WarpFaceOp,
  type PortraitOp,
  type PatchOp,
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
export const isBeauty = (o: Op): o is BeautyOp => o.type === 'beauty'
export const isWarpFace = (o: Op): o is WarpFaceOp => o.type === 'warp' && (o as { kind?: unknown }).kind === 'face'
export const isWarpBody = (o: Op): o is WarpBodyOp => o.type === 'warp' && (o as { kind?: unknown }).kind === 'body'
export const isPortrait = (o: Op): o is PortraitOp => isBeauty(o) || isWarpFace(o) || isWarpBody(o)
export const isPatch = (o: Op): o is PatchOp => o.type === 'patch'
export const isEffects = (o: Op): o is EffectsOp => o.type === 'effects'
export const isDetail = (o: Op): o is DetailOp => o.type === 'detail'
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
  if (ONE_WAY_KEYS.includes(key)) return { key, min: 0, max: 100, step: 1, decimals: 0 }
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
    } else if (isPortrait(op)) {
      if (!isNeutralPortrait(op)) ops.push(op)
    } else if (isEffects(op)) {
      const e = normalizeEffects(op)
      if (e) ops.push(e)
    } else if (isDetail(op)) {
      const d = normalizeDetail(op)
      if (d) ops.push(d)
    } else ops.push(op)
  }
  return { version: stack.version ?? 1, ops }
}

const zero = (v: unknown) => !(typeof v === 'number' && Math.abs(v) > 1e-6)

/** A portrait op with every amount at 0 renders nothing and is dropped (`protect_background` / `level` alone do not count). */
export function isNeutralPortrait(o: PortraitOp): boolean {
  if (isBeauty(o)) return zero(o.smooth) && zero(o.whiten) && !o.blemish && zero(o.eye_brighten) && zero(o.teeth_whiten) && zero(o.dark_circles)
  if (isWarpFace(o)) return zero(o.slim) && zero(o.chin) && zero(o.eyes) && zero(o.nose)
  return zero(o.arms) && zero(o.legs) && zero(o.waist) && zero(o.lengthen_legs)
}

// ---- M9: effects / detail

/** Neutral settings, spelled out so the UI and the Rust defaults agree (`ip-render` `Default`). */
export type VignetteValues = Required<Omit<Vignette, 'style'>>
export const VIGNETTE_DEFAULTS: VignetteValues = { amount: 0, midpoint: 50, roundness: 0, feather: 50, highlight_contrast: 0 }
export const GRAIN_DEFAULTS: Required<Grain> = { amount: 0, size: 25, frequency: 50 }
export const SHARPEN_DEFAULTS: Required<SharpenDetail> = { amount: 0, radius: 1, detail: 25, masking: 0 }
export const NOISE_DEFAULTS: Required<NoiseReduction> = { luminance: 0, luminance_detail: 50, luminance_contrast: 0, color: 0, color_detail: 50 }

const filled = <T extends object>(defaults: T, v: Partial<T> | undefined): T => ({ ...defaults, ...(v ?? {}) })

/** The vignette with defaults filled in; `amount` 0 means off. */
export const vignetteOf = (o: EffectsOp | undefined): VignetteValues & { style: NonNullable<Vignette['style']> } => ({
  ...filled(VIGNETTE_DEFAULTS, o?.vignette),
  style: o?.vignette?.style ?? 'highlight_priority',
})
export const grainOf = (o: EffectsOp | undefined): Required<Grain> => filled(GRAIN_DEFAULTS, o?.grain)
export const sharpenOf = (o: DetailOp | undefined): Required<SharpenDetail> => filled(SHARPEN_DEFAULTS, o?.sharpen)
export const noiseOf = (o: DetailOp | undefined): Required<NoiseReduction> => filled(NOISE_DEFAULTS, o?.noise)

export const isNeutralEffects = (o: EffectsOp): boolean => grainOf(o).amount <= 0 && vignetteOf(o).amount === 0
export const isNeutralDetail = (o: DetailOp): boolean => sharpenOf(o).amount <= 0 && noiseOf(o).luminance <= 0 && noiseOf(o).color <= 0

/** Drop sub-objects that are still at their neutral defaults. */
export function normalizeEffects(o: EffectsOp): EffectsOp | null {
  const out: EffectsOp = { type: 'effects' }
  const v = o.vignette
  if (v && (v.amount ?? 0) !== 0) out.vignette = { ...VIGNETTE_DEFAULTS, ...v }
  if (grainOf(o).amount > 0) out.grain = { ...GRAIN_DEFAULTS, ...(o.grain ?? {}) }
  return out.vignette || out.grain ? out : null
}

export function normalizeDetail(o: DetailOp): DetailOp | null {
  const out: DetailOp = { type: 'detail' }
  if (sharpenOf(o).amount > 0) out.sharpen = { ...SHARPEN_DEFAULTS, ...(o.sharpen ?? {}) }
  const n = noiseOf(o)
  if (n.luminance > 0 || n.color > 0) out.noise = { ...NOISE_DEFAULTS, ...(o.noise ?? {}) }
  return out.sharpen || out.noise ? out : null
}

/** A disabled patch renders nothing (ip-render `is_identity`), so it does not make a stack "edited". */
export const isEmptyStack = (s: EditStack): boolean =>
  normalizeStack(s).ops.every((o) => !isKnown(o) && !isPortrait(o) && !(isPatch(o) && o.enabled !== false))

// ---------------------------------------------------------------- ordering

// Render order (docs/api-contract-m9.md A): patch -> crop -> warp -> global -> local -> beauty
// -> effects -> LUT -> detail -> output sharpen. `detail` is a whole-image stage that always
// runs last but one, whatever its position in the array; the order here keeps the stack
// canonical so equal looks share a cache key.
const RANK: Record<string, number> = {
  patch: -1,
  crop: 0,
  warp: 0.5,
  global: 1,
  local: 3,
  beauty: 3.5,
  effects: 3.8,
  lut: 4,
  detail: 4.5,
  output_sharpen: 5,
}
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

/** Replace (or clear, with `null`) the range mask of one local adjustment. */
export const setLocalRange = (s: EditStack, index: number, range: MaskRange | null): EditStack =>
  updateLocal(s, index, (op) => ({ ...op, range: range ?? undefined }))

export const LUMINANCE_RANGE_DEFAULTS = { min: 0, max: 100, softness: 30 } as const
export const COLOR_RANGE_DEFAULTS = { hue: 30, width: 40, sat_min: 10, sat_max: 100, softness: 40 } as const

/** The range with its defaults filled in, or `null` when there is none. */
export function rangeOf(op: LocalOp): LuminanceRange | ColorRange | null {
  const r = op.range
  if (!r) return null
  if (r.kind === 'luminance') {
    const x = r as LuminanceRange
    return { ...LUMINANCE_RANGE_DEFAULTS, ...x }
  }
  if (r.kind === 'color') return { ...COLOR_RANGE_DEFAULTS, ...(r as ColorRange) }
  return null
}

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

/**
 * Largest centred rect of pixel aspect `ratio` inside a w x h image rotated by `angleDeg` (the same
 * closed form as `ip_render::crop_assist::inscribed_rect`); the mock server uses it.
 */
export function inscribedRect(w: number, h: number, angleDeg: number, ratio: number): [number, number, number, number] {
  const th = (angleDeg * Math.PI) / 180
  const s = Math.abs(Math.sin(th))
  const c = Math.abs(Math.cos(th))
  const k = Math.min(w / 2 / (ratio * c + s), h / 2 / (ratio * s + c))
  const a = k * ratio
  return [0.5 - a / w, 0.5 - k / h, (2 * a) / w, (2 * k) / h]
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

// ---------------------------------------------------------------- effects / detail

export const getEffects = (s: EditStack): EffectsOp | undefined => s.ops.find(isEffects)
/** Replace (or clear) the effects op. Neutral settings normalise away to "no op". */
export function setEffects(s: EditStack, e: EffectsOp | null): EditStack {
  const rest = s.ops.filter((o) => !isEffects(o))
  return withOps(s, e ? [...rest, e] : rest)
}
export const updateEffects = (s: EditStack, fn: (e: EffectsOp) => EffectsOp): EditStack =>
  setEffects(s, fn(getEffects(s) ?? { type: 'effects' }))

export const getDetail = (s: EditStack): DetailOp | undefined => s.ops.find(isDetail)
export function setDetail(s: EditStack, d: DetailOp | null): EditStack {
  const rest = s.ops.filter((o) => !isDetail(o))
  return withOps(s, d ? [...rest, d] : rest)
}
export const updateDetail = (s: EditStack, fn: (d: DetailOp) => DetailOp): EditStack =>
  setDetail(s, fn(getDetail(s) ?? { type: 'detail' }))

export function setVignette(s: EditStack, patch: Partial<Vignette>): EditStack {
  return updateEffects(s, (e) => ({ ...e, vignette: { ...vignetteOf(e), ...patch } }))
}
export function setGrain(s: EditStack, patch: Partial<Grain>): EditStack {
  return updateEffects(s, (e) => ({ ...e, grain: { ...grainOf(e), ...patch } }))
}
export function setSharpenDetail(s: EditStack, patch: Partial<SharpenDetail>): EditStack {
  return updateDetail(s, (d) => ({ ...d, sharpen: { ...sharpenOf(d), ...patch } }))
}
export function setNoiseReduction(s: EditStack, patch: Partial<NoiseReduction>): EditStack {
  return updateDetail(s, (d) => ({ ...d, noise: { ...noiseOf(d), ...patch } }))
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

export const PRESET_SECTIONS: EditSection[] = ['global', 'local', 'effects', 'lut', 'detail', 'output_sharpen']
export const applyPreset = (s: EditStack, preset: EditStack): EditStack => applySections(s, preset, PRESET_SECTIONS)

// ---------------------------------------------------------------- preset strength (mirrors ip-render adapt.rs)

/** Largest preset strength (150 %), as `ip_render::PRESET_AMOUNT_MAX`. */
export const PRESET_AMOUNT_MAX = 1.5

const scaleCurve = (pts: Point[] | undefined, k: number): Point[] | undefined => pts?.map(([x, y]) => [x, clamp(x + (y - x) * k, 0, 1)] as Point)

function scaleAdjust(a: Adjust, k: number): Adjust {
  const out: Adjust = { ...a }
  for (const key of ADJUST_NUMERIC_KEYS) {
    const v = a[key]
    if (typeof v === 'number') out[key] = clamp(v * k, SLIDER_SPEC[key].min, SLIDER_SPEC[key].max)
  }
  if (a.curve) out.curve = { rgb: scaleCurve(a.curve.rgb, k), r: scaleCurve(a.curve.r, k), g: scaleCurve(a.curve.g, k), b: scaleCurve(a.curve.b, k) }
  if (a.hsl) {
    const hsl: NonNullable<Adjust['hsl']> = {}
    for (const b of HSL_BANDS) {
      const v = a.hsl[b]
      if (v) hsl[b] = { h: clamp((v.h ?? 0) * k, -100, 100), s: clamp((v.s ?? 0) * k, -100, 100), l: clamp((v.l ?? 0) * k, -100, 100) }
    }
    out.hsl = hsl
  }
  if (a.grading) {
    const g = a.grading
    const w = (p: Point | undefined): Point => [p?.[0] ?? 0, clamp((p?.[1] ?? 0) * k, 0, 1)]
    out.grading = { shadows: w(g.shadows), midtones: w(g.midtones), highlights: w(g.highlights), balance: clamp((g.balance ?? 0) * k, -100, 100) }
  }
  if (k === 0) {
    // 0 % = no look at all (a scaled curve would be a multi-point identity that is not dropped)
    delete out.curve
    delete out.hsl
    delete out.grading
  }
  return out
}

/**
 * Scale a preset stack's deltas by `amount` (0..1.5): sliders, curve offsets, HSL, grading and the
 * LUT / local / sharpen amounts. The editor uses it for the live preview while the strength slider
 * is dragged; on release the server fits the preset again at that strength (`POST /edits/{id}/preset`).
 */
export function scalePresetStack(s: EditStack, amount: number): EditStack {
  const k = clamp(Number.isFinite(amount) ? amount : 1, 0, PRESET_AMOUNT_MAX)
  if (k === 1) return s
  const ops: Op[] = []
  for (const o of s.ops) {
    if (isGlobal(o)) ops.push({ ...scaleAdjust(o, k), type: 'global' })
    else if (isLocal(o)) ops.push({ ...o, amount: clamp((o.amount ?? 1) * k, 0, 1) })
    else if (isLut(o)) ops.push({ ...o, amount: clamp((o.amount ?? 1) * k, 0, 1) })
    else if (isSharpen(o)) ops.push({ ...o, amount: clamp(o.amount * k, 0, 100) })
    else ops.push(o)
  }
  return normalizeStack({ version: s.version ?? 1, ops })
}

/** `preset:<id>;amount=<percent>`, the `source` the server writes into a fitted preset's global op. */
export function parsePresetSource(source: string | undefined): { id: string; amount: number } | null {
  if (!source?.startsWith('preset:')) return null
  const rest = source.slice('preset:'.length)
  const i = rest.indexOf(';amount=')
  const id = i < 0 ? rest : rest.slice(0, i)
  const amount = i < 0 ? 1 : Number(rest.slice(i + ';amount='.length)) / 100
  return id && Number.isFinite(amount) ? { id, amount } : null
}

/** The preset the stack's global op came from, if any (drives the strength slider). */
export const appliedPreset = (s: EditStack): { id: string; amount: number } | null => parsePresetSource(getAdjust(s).source)

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

/** The stack without its crop: Best Take edits in full-frame coordinates (face boxes are pre-crop). */
export function withoutCrop(s: EditStack): EditStack {
  return { version: s.version ?? 1, ops: s.ops.filter((o) => !isCrop(o)) }
}
