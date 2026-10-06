/**
 * Mock renderer for POST /api/render/preview: rasterises the photo's mock SVG on an OffscreenCanvas and
 * applies a cheap CPU approximation of the edit stack (exposure / white balance / tone / curves / HSL /
 * grading / crop+straighten / local masks / LUT / sharpen) so every control visibly does something.
 * This is NOT the real engine; it only has to be plausible and fast (~10-40 ms at long_edge 800).
 */
import type { Adjust, EditStack, HslBand, LocalOp, MaskRef, Photo } from '@/api/types'
import { HSL_BANDS } from '@/api/types'
import { curveFn } from '@/lib/curves'
import { getCrop, isEmptyStack, isGlobal, isLocal, isLut, isSharpen, normalizeStack } from '@/lib/edit'
import { drawMask } from './masks'
import { applyBeauty, applyWarps } from './portrait'
import { photoSvg } from './svg'

const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v)
const smooth = (t: number) => {
  const x = clamp01(t)
  return x * x * (3 - 2 * x)
}

// ---------------------------------------------------------------- colour helpers

const SRGB2LIN = new Float32Array(256)
for (let i = 0; i < 256; i++) {
  const c = i / 255
  SRGB2LIN[i] = c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
}
const lin2srgb = (v: number) => (v <= 0.0031308 ? 12.92 * v : 1.055 * Math.max(0, v) ** (1 / 2.4) - 0.055)

const BAND_HUE: Record<HslBand, number> = { red: 0, orange: 30, yellow: 60, green: 120, aqua: 180, blue: 240, purple: 270, magenta: 320 }

function rgb2hsl(r: number, g: number, b: number): [number, number, number] {
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const l = (max + min) / 2
  const d = max - min
  if (d < 1e-6) return [0, 0, l]
  const s = d / (1 - Math.abs(2 * l - 1))
  let h: number
  if (max === r) h = ((g - b) / d) % 6
  else if (max === g) h = (b - r) / d + 2
  else h = (r - g) / d + 4
  return [(h * 60 + 360) % 360, s, l]
}
function hsl2rgb(h: number, s: number, l: number): [number, number, number] {
  const c = (1 - Math.abs(2 * l - 1)) * s
  const hp = (((h % 360) + 360) % 360) / 60
  const x = c * (1 - Math.abs((hp % 2) - 1))
  const [r, g, b] = hp < 1 ? [c, x, 0] : hp < 2 ? [x, c, 0] : hp < 3 ? [0, c, x] : hp < 4 ? [0, x, c] : hp < 5 ? [x, 0, c] : [c, 0, x]
  const m = l - c / 2
  return [r + m, g + m, b + m]
}

// ---------------------------------------------------------------- adjuster

type Pix = (px: Uint8ClampedArray, i: number) => void

/** Builds an in-place per-pixel function for one Adjust block. */
function makeAdjuster(a: Adjust): Pix | null {
  const num = (k: keyof Adjust) => (typeof a[k] === 'number' ? (a[k] as number) : 0)
  const gain = 2 ** num('exposure')
  const t = num('temp') / 3000
  const tint = num('tint') / 100
  const wb = [1 + t * 0.32, 1 - tint * 0.16, 1 - t * 0.32]
  const sh = num('shadows') / 100
  const hi = num('highlights') / 100
  const wh = num('whites') / 100
  const bl = num('blacks') / 100
  const con = num('contrast') / 100 + num('dehaze') / 220
  const cl = num('clarity') / 100
  const sat = 1 + num('saturation') / 100 + (num('dehaze') > 0 ? num('dehaze') / 400 : 0)
  const vib = num('vibrance') / 100
  const hsl = a.hsl && Object.keys(a.hsl).length ? a.hsl : null
  const g = a.grading && ((a.grading.shadows?.[1] ?? 0) + (a.grading.midtones?.[1] ?? 0) + (a.grading.highlights?.[1] ?? 0) > 0 || a.grading.balance) ? a.grading : null

  const tone = (x: number) => {
    let y = x
    y += 0.3 * sh * (1 - y) ** 2 * 0.7
    y += 0.3 * hi * y ** 2
    y += 0.2 * wh * y ** 6
    y += 0.2 * bl * (1 - y) ** 6
    if (con) y = 0.5 + (y - 0.5) * (1 + con * 0.9)
    if (cl) y += cl * 0.5 * (y - 0.5) * (1 - Math.abs(2 * y - 1))
    return clamp01(y)
  }
  const rgbCurve = a.curve?.rgb?.length ? curveFn(a.curve.rgb) : null
  const chCurves = [a.curve?.r, a.curve?.g, a.curve?.b].map((p) => (p?.length ? curveFn(p) : null))
  const luts = [0, 1, 2].map((ch) => {
    const lut = new Uint8ClampedArray(256)
    for (let i = 0; i < 256; i++) {
      let y = clamp01(lin2srgb(SRGB2LIN[i] * gain * wb[ch]))
      y = tone(y)
      if (rgbCurve) y = rgbCurve(y)
      const c = chCurves[ch]
      if (c) y = c(y)
      lut[i] = Math.round(y * 255)
    }
    return lut
  })
  const doColour = sat !== 1 || vib !== 0 || !!hsl || !!g
  const [lr, lg, lb] = luts
  const gradeRgb = (hue: number, amt: number): [number, number, number] => {
    const [r, gg, b] = hsl2rgb(hue, 1, 0.5)
    return [(r - 0.5) * amt, (gg - 0.5) * amt, (b - 0.5) * amt]
  }
  const gs = g ? gradeRgb(g.shadows?.[0] ?? 0, (g.shadows?.[1] ?? 0) * 0.5) : null
  const gm = g ? gradeRgb(g.midtones?.[0] ?? 0, (g.midtones?.[1] ?? 0) * 0.4) : null
  const gh = g ? gradeRgb(g.highlights?.[0] ?? 0, (g.highlights?.[1] ?? 0) * 0.5) : null
  const bal = (g?.balance ?? 0) / 100
  const hslBands = hsl ? HSL_BANDS.filter((b) => hsl[b] && (hsl[b]!.h || hsl[b]!.s || hsl[b]!.l)) : []

  return (px, i) => {
    let r = lr[px[i]] / 255
    let gr = lg[px[i + 1]] / 255
    let b = lb[px[i + 2]] / 255
    if (doColour) {
      const luma = 0.299 * r + 0.587 * gr + 0.114 * b
      if (sat !== 1 || vib) {
        const chroma = Math.max(r, gr, b) - Math.min(r, gr, b)
        const f = sat * (1 + vib * (1 - chroma))
        r = luma + (r - luma) * f
        gr = luma + (gr - luma) * f
        b = luma + (b - luma) * f
      }
      if (hslBands.length) {
        const [h0, s0, l0] = rgb2hsl(clamp01(r), clamp01(gr), clamp01(b))
        if (s0 > 0.03) {
          let dh = 0
          let ds = 0
          let dl = 0
          for (const band of hslBands) {
            let d = Math.abs(h0 - BAND_HUE[band])
            if (d > 180) d = 360 - d
            const w = smooth(1 - d / 38)
            if (w <= 0) continue
            const v = hsl![band]!
            dh += w * (v.h ?? 0) * 0.3
            ds += w * ((v.s ?? 0) / 100)
            dl += w * ((v.l ?? 0) / 100) * 0.35
          }
          ;[r, gr, b] = hsl2rgb(h0 + dh, clamp01(s0 * (1 + ds)), clamp01(l0 + dl * s0 * 1.4))
        }
      }
      if (g && gs && gm && gh) {
        const l = clamp01(0.299 * r + 0.587 * gr + 0.114 * b)
        const ws = (1 - l) ** 2 * (1 - bal * 0.7)
        const wHi = l ** 2 * (1 + bal * 0.7)
        const wm = 4 * l * (1 - l)
        r += gs[0] * ws + gm[0] * wm + gh[0] * wHi
        gr += gs[1] * ws + gm[1] * wm + gh[1] * wHi
        b += gs[2] * ws + gm[2] * wm + gh[2] * wHi
      }
    }
    px[i] = clamp01(r) * 255
    px[i + 1] = clamp01(gr) * 255
    px[i + 2] = clamp01(b) * 255
  }
}

// ---------------------------------------------------------------- LUTs

const MOCK_LUTS: Record<string, (r: number, g: number, b: number) => [number, number, number]> = {
  film_warm: (r, g, b) => [r * 1.06 + 0.03, g * 1.01 + 0.01, b * 0.88 + 0.02],
  film_cool: (r, g, b) => [r * 0.93, g * 1.0 + 0.01, b * 1.1 + 0.03],
  teal_orange: (r, g, b) => {
    const l = 0.299 * r + 0.587 * g + 0.114 * b
    return [r + (l - 0.4) * 0.35, g + (0.4 - Math.abs(l - 0.5)) * 0.06, b + (0.55 - l) * 0.35]
  },
  matte: (r, g, b) => [0.08 + r * 0.88, 0.08 + g * 0.88, 0.09 + b * 0.9],
  bw: (r, g, b) => {
    const l = 0.299 * r + 0.587 * g + 0.114 * b
    return [l, l, l]
  },
}

// ---------------------------------------------------------------- masks

function geometricMask(mask: MaskRef, w: number, h: number): Float32Array | null {
  const m = new Float32Array(w * h)
  if (mask.kind === 'radial') {
    const [cx, cy] = mask.center
    const rx = Math.max(0.01, mask.radius[0])
    const ry = Math.max(0.01, mask.radius[1])
    const inner = 1 - clamp01(mask.feather ?? 0.5)
    for (let y = 0; y < h; y++)
      for (let x = 0; x < w; x++) {
        const d = Math.hypot((x / w - cx) / rx, (y / h - cy) / ry)
        m[y * w + x] = d <= inner ? 1 : d >= 1 ? 0 : 1 - smooth((d - inner) / Math.max(1e-3, 1 - inner))
      }
    return m
  }
  if (mask.kind === 'linear') {
    const [sx, sy] = mask.start
    const [ex, ey] = mask.end
    const dx = ex - sx
    const dy = ey - sy
    const len2 = Math.max(1e-6, dx * dx + dy * dy)
    for (let y = 0; y < h; y++)
      for (let x = 0; x < w; x++) {
        const t = ((x / w - sx) * dx + (y / h - sy) * dy) / len2
        m[y * w + x] = 1 - smooth(t)
      }
    return m
  }
  return null
}

// ---------------------------------------------------------------- source images

const imgCache = new Map<string, HTMLImageElement>()
async function sourceImage(p: Photo, longEdge: number): Promise<HTMLImageElement> {
  // quantise the source resolution so successive requests hit the cache
  const px = Math.max(256, Math.ceil(longEdge / 128) * 128)
  const key = `${p.id}:${px}`
  const hit = imgCache.get(key)
  if (hit) return hit
  const url = URL.createObjectURL(new Blob([photoSvg(p, px, px >= 768)], { type: 'image/svg+xml' }))
  const img = new Image()
  img.src = url
  await img.decode()
  imgCache.set(key, img)
  if (imgCache.size > 24) {
    const first = imgCache.keys().next().value
    if (first !== undefined) imgCache.delete(first)
  }
  return img
}

// ---------------------------------------------------------------- main

export interface MockRenderOptions {
  longEdge: number
  /** ignore the stack (before/after) */
  original?: boolean
}

/** Output pixel size for a photo + stack at `longEdge` (crop aware). */
export function outputSize(p: Photo, stack: EditStack, longEdge: number): { W: number; H: number; cw: number; ch: number } {
  const iw = p.width ?? 3000
  const ih = p.height ?? 2000
  const crop = getCrop(stack)
  const [, , rw, rh] = crop?.rect ?? [0, 0, 1, 1]
  const cwPx = rw * iw
  const chPx = rh * ih
  const k = longEdge / Math.max(cwPx, chPx)
  return { W: Math.max(8, Math.round(iw * k)), H: Math.max(8, Math.round(ih * k)), cw: Math.max(8, Math.round(cwPx * k)), ch: Math.max(8, Math.round(chPx * k)) }
}

export async function renderMock(p: Photo, stackIn: EditStack, opts: MockRenderOptions): Promise<Blob> {
  const stack = opts.original ? { version: 1, ops: [] } : normalizeStack(stackIn)
  const longEdge = Math.min(2048, Math.max(16, Math.round(opts.longEdge)))
  const { W, H, cw, ch } = outputSize(p, stack, longEdge)
  const canvas = new OffscreenCanvas(cw, ch)
  const ctx = canvas.getContext('2d', { willReadFrequently: true })!
  const img = await sourceImage(p, Math.max(W, H))

  // geometry: crop rect + straighten about the image centre (cover-scaled so no empty corners)
  const crop = getCrop(stack)
  const [rx, ry] = crop?.rect ?? [0, 0, 1, 1]
  const angle = ((crop?.angle ?? 0) * Math.PI) / 180
  const cover = Math.cos(Math.abs(angle)) + Math.sin(Math.abs(angle)) * Math.max(W / H, H / W)
  const geom = (c: OffscreenCanvasRenderingContext2D) => {
    c.translate(-rx * W, -ry * H)
    c.translate(W / 2, H / 2)
    c.rotate(-angle)
    c.scale(cover, cover)
    c.translate(-W / 2, -H / 2)
  }
  ctx.save()
  geom(ctx)
  ctx.drawImage(img, 0, 0, W, H)
  ctx.restore()

  if (!isEmptyStack(stack)) {
    const data = ctx.getImageData(0, 0, cw, ch)
    const px = data.data
    const n = cw * ch
    const cropRect = (crop?.rect ?? [0, 0, 1, 1]) as [number, number, number, number]
    applyWarps(px, cw, ch, stack, p, cropRect)
    for (const op of stack.ops) {
      if (isGlobal(op)) {
        const { type: _t, ...adj } = op
        void _t
        const f = makeAdjuster(adj)
        if (f) for (let i = 0; i < n * 4; i += 4) f(px, i)
      }
    }
    for (const op of stack.ops) {
      if (!isLocal(op)) continue
      applyLocal(op, px, cw, ch, (c) => {
        geom(c)
      }, p, W, H)
    }
    applyBeauty(px, cw, ch, stack, p, cropRect)
    for (const op of stack.ops) {
      if (isLut(op)) {
        const f = MOCK_LUTS[op.file] ?? MOCK_LUTS.teal_orange
        const amt = clamp01(op.amount ?? 1)
        for (let i = 0; i < n * 4; i += 4) {
          const [r, g, b] = f(px[i] / 255, px[i + 1] / 255, px[i + 2] / 255)
          px[i] = (px[i] / 255 + (clamp01(r) - px[i] / 255) * amt) * 255
          px[i + 1] = (px[i + 1] / 255 + (clamp01(g) - px[i + 1] / 255) * amt) * 255
          px[i + 2] = (px[i + 2] / 255 + (clamp01(b) - px[i + 2] / 255) * amt) * 255
        }
      }
    }
    const sharpen = stack.ops.find(isSharpen)
    if (sharpen && sharpen.amount > 0) unsharp(px, cw, ch, sharpen.amount / 100)
    ctx.putImageData(data, 0, 0)
  }
  return canvas.convertToBlob({ type: 'image/jpeg', quality: 0.9 })
}

function applyLocal(
  op: LocalOp,
  px: Uint8ClampedArray,
  w: number,
  h: number,
  geom: (c: OffscreenCanvasRenderingContext2D) => void,
  p: Photo,
  fullW: number,
  fullH: number,
) {
  const f = makeAdjuster(op.adjust)
  if (!f) return
  let m = geometricMask(op.mask, w, h)
  if (!m && op.mask.kind === 'ai') {
    const mc = new OffscreenCanvas(w, h)
    const mctx = mc.getContext('2d', { willReadFrequently: true })!
    mctx.save()
    geom(mctx)
    drawMask(mctx, op.mask.target, p, fullW, fullH)
    mctx.restore()
    const md = mctx.getImageData(0, 0, w, h).data
    m = new Float32Array(w * h)
    for (let i = 0; i < w * h; i++) m[i] = md[i * 4] / 255
  }
  if (!m) return
  const amount = clamp01(op.amount ?? 1)
  const tmp = new Uint8ClampedArray(4)
  for (let i = 0, k = 0; k < w * h; i += 4, k++) {
    let a = m[k]
    if (op.invert) a = 1 - a
    a *= amount
    if (a <= 0.003) continue
    tmp[0] = px[i]
    tmp[1] = px[i + 1]
    tmp[2] = px[i + 2]
    f(tmp, 0)
    px[i] += (tmp[0] - px[i]) * a
    px[i + 1] += (tmp[1] - px[i + 1]) * a
    px[i + 2] += (tmp[2] - px[i + 2]) * a
  }
}

function unsharp(px: Uint8ClampedArray, w: number, h: number, amount: number) {
  const src = new Uint8ClampedArray(px)
  const k = amount * 1.2
  for (let y = 1; y < h - 1; y++)
    for (let x = 1; x < w - 1; x++) {
      const i = (y * w + x) * 4
      for (let c = 0; c < 3; c++) {
        const blur = (src[i + c - 4] + src[i + c + 4] + src[i + c - w * 4] + src[i + c + w * 4]) / 4
        px[i + c] = src[i + c] + (src[i + c] - blur) * k
      }
    }
}
