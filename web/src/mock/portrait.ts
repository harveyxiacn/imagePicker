/**
 * Mock approximation of the M4 portrait ops (beauty softening / brightening inside the face box, face slim as a
 * horizontal squeeze, simple body squeeze). NOT the real engine (crates/ip-render): it uses the synthetic people of
 * the mock backend and exaggerates amounts so every slider visibly does something at preview resolution.
 */
import type { BeautyOp, EditStack, Level, Photo, PortraitOp, WarpBodyOp, WarpFaceOp } from '@/api/types'
import { isBeauty, isWarpBody, isWarpFace } from '@/lib/edit'
import { mockPeopleOfPhoto } from './ai'
import { isBeautyReady } from './beautyState'

const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v)
const smooth = (t: number) => {
  const x = clamp01(t)
  return x * x * (3 - 2 * x)
}

/** Level multiplier for geometry (docs: natural <= 3 %, refined <= 6 % of the face width; exaggerated here for the mock). */
const WARP_K: Record<Level, number> = { natural: 0.5, standard: 0.8, refined: 1.15 }
/** Level cap for skin blending. */
const BEAUTY_K: Record<Level, number> = { natural: 0.55, standard: 0.8, refined: 1 }

interface Box {
  x: number
  y: number
  w: number
  h: number
}

/** Face boxes (output pixels) an op applies to. Ops of people that are not in the photo are skipped like the real renderer does. */
function targets(op: PortraitOp, p: Photo, crop: [number, number, number, number], cw: number, ch: number): Box[] {
  const [rx, ry, rw, rh] = crop
  return mockPeopleOfPhoto(p)
    .filter((x) => op.person_id == null || x.person_id === op.person_id)
    .map((x) => ({
      x: ((x.face_box[0] - rx) / rw) * cw,
      y: ((x.face_box[1] - ry) / rh) * ch,
      w: (x.face_box[2] / rw) * cw,
      h: (x.face_box[3] / rh) * ch,
    }))
}

function sample(src: Uint8ClampedArray, w: number, h: number, fx: number, fy: number, out: number[]) {
  const x = Math.min(w - 1.001, Math.max(0, fx))
  const y = Math.min(h - 1.001, Math.max(0, fy))
  const x0 = Math.floor(x)
  const y0 = Math.floor(y)
  const tx = x - x0
  const ty = y - y0
  const i00 = (y0 * w + x0) * 4
  const i10 = i00 + 4
  const i01 = i00 + w * 4
  const i11 = i01 + 4
  for (let c = 0; c < 3; c++) {
    const top = src[i00 + c] * (1 - tx) + src[i10 + c] * tx
    const bot = src[i01 + c] * (1 - tx) + src[i11 + c] * tx
    out[c] = top * (1 - ty) + bot * ty
  }
}

/** Apply a displacement field `disp(x, y) -> [dx, dy]` (source = dest + d) inside `region`. */
function warpRegion(px: Uint8ClampedArray, w: number, h: number, region: Box, disp: (x: number, y: number, o: [number, number]) => void) {
  const src = new Uint8ClampedArray(px)
  const x0 = Math.max(0, Math.floor(region.x))
  const y0 = Math.max(0, Math.floor(region.y))
  const x1 = Math.min(w - 1, Math.ceil(region.x + region.w))
  const y1 = Math.min(h - 1, Math.ceil(region.y + region.h))
  const d: [number, number] = [0, 0]
  const col = [0, 0, 0]
  for (let y = y0; y <= y1; y++)
    for (let x = x0; x <= x1; x++) {
      d[0] = 0
      d[1] = 0
      disp(x, y, d)
      if (d[0] === 0 && d[1] === 0) continue
      sample(src, w, h, x + d[0], y + d[1], col)
      const i = (y * w + x) * 4
      px[i] = col[0]
      px[i + 1] = col[1]
      px[i + 2] = col[2]
    }
}

function warpFace(px: Uint8ClampedArray, w: number, h: number, b: Box, op: WarpFaceOp) {
  const k = WARP_K[op.level] ?? 0.8
  const slim = (op.slim / 100) * k * 0.45
  const chin = (op.chin / 100) * k * 0.22
  const eyes = (op.eyes / 100) * k * 0.6
  const nose = (op.nose / 100) * k * 0.4
  if (!slim && !chin && !eyes && !nose) return
  const cx = b.x + b.w / 2
  const cy = b.y + b.h / 2
  const hw = b.w / 2
  const hh = b.h / 2
  const region: Box = { x: cx - hw * 2, y: cy - hh * 2, w: hw * 4, h: hh * 4 }
  const eyeY = b.y + b.h * 0.4
  const eyeR = Math.max(3, b.w * 0.2)
  warpRegion(px, w, h, region, (x, y, o) => {
    const u = (x - cx) / hw
    const v = (y - cy) / hh
    const r = Math.hypot(u, v)
    if (slim) {
      // squeeze the lower two thirds of the face toward the centre line
      const wgt = smooth(1 - r / 1.9) * smooth((v + 0.7) / 1.1)
      o[0] += slim * (x - cx) * wgt
    }
    if (chin) {
      const dy = (y - (b.y + b.h * 0.95)) / (b.h * 0.35)
      const dx = (x - cx) / (b.w * 0.5)
      o[1] += chin * b.h * 0.5 * Math.exp(-(dx * dx + dy * dy))
    }
    if (nose) {
      const dx = (x - cx) / (b.w * 0.2)
      const dy = (y - (b.y + b.h * 0.62)) / (b.h * 0.2)
      o[0] += nose * (x - cx) * Math.exp(-(dx * dx + dy * dy))
    }
    if (eyes) {
      for (const sx of [-0.23, 0.23]) {
        const ex = cx + b.w * sx
        const dx = x - ex
        const dy = y - eyeY
        const rr = Math.hypot(dx, dy) / eyeR
        if (rr < 1) {
          const f = 1 - eyes * (1 - rr * rr)
          o[0] += dx * (f - 1)
          o[1] += dy * (f - 1)
        }
      }
    }
  })
}

function warpBody(px: Uint8ClampedArray, w: number, h: number, face: Box, op: WarpBodyOp) {
  const k = WARP_K[op.level] ?? 0.8
  const arms = (op.arms / 100) * k * 0.25
  const legs = (op.legs / 100) * k * 0.25
  const waist = (op.waist / 100) * k * 0.3
  if (!arms && !legs && !waist) return
  const cx = face.x + face.w / 2
  const top = face.y + face.h * 1.1
  const bh = face.h * 5.2
  const bw = face.w * 1.7
  warpRegion(px, w, h, { x: cx - bw * 1.3, y: top, w: bw * 2.6, h: bh }, (x, y, o) => {
    const t = (y - top) / bh // 0 shoulders .. 1 feet
    if (t < 0 || t > 1) return
    const dx = x - cx
    const side = smooth((Math.abs(dx) - bw * 0.55) / (bw * 0.4)) * (1 - smooth((Math.abs(dx) - bw * 1.25) / (bw * 0.3)))
    let amt = 0
    if (arms) amt += arms * side * (1 - smooth((t - 0.4) / 0.2))
    if (waist) amt += waist * smooth(1 - Math.abs(t - 0.32) / 0.2) * smooth(1 - Math.abs(dx) / (bw * 1.3))
    if (legs) amt += legs * smooth((t - 0.45) / 0.2) * smooth(1 - Math.abs(dx) / (bw * 1.1))
    o[0] += dx * amt
  })
}

/** crop -> warp: displacement ops in stack order, only once the photo's geometry is prepared. */
export function applyWarps(px: Uint8ClampedArray, cw: number, ch: number, stack: EditStack, p: Photo, crop: [number, number, number, number]) {
  if (!isBeautyReady(p.id)) return
  for (const op of stack.ops) {
    if (isWarpFace(op)) for (const b of targets(op, p, crop, cw, ch)) warpFace(px, cw, ch, b, op)
    else if (isWarpBody(op)) for (const b of targets(op, p, crop, cw, ch)) warpBody(px, cw, ch, b, op)
  }
}

function boxBlur(src: Float32Array, w: number, h: number, r: number): Float32Array {
  if (r < 1) return src
  const tmp = new Float32Array(src.length)
  const out = new Float32Array(src.length)
  const n = 2 * r + 1
  for (let y = 0; y < h; y++) {
    for (let c = 0; c < 3; c++) {
      let acc = 0
      for (let x = -r; x <= r; x++) acc += src[(y * w + Math.min(w - 1, Math.max(0, x))) * 3 + c]
      for (let x = 0; x < w; x++) {
        tmp[(y * w + x) * 3 + c] = acc / n
        acc += src[(y * w + Math.min(w - 1, x + r + 1)) * 3 + c] - src[(y * w + Math.max(0, x - r)) * 3 + c]
      }
    }
  }
  for (let x = 0; x < w; x++) {
    for (let c = 0; c < 3; c++) {
      let acc = 0
      for (let y = -r; y <= r; y++) acc += tmp[(Math.min(h - 1, Math.max(0, y)) * w + x) * 3 + c]
      for (let y = 0; y < h; y++) {
        out[(y * w + x) * 3 + c] = acc / n
        acc += tmp[(Math.min(h - 1, y + r + 1) * w + x) * 3 + c] - tmp[(Math.max(0, y - r) * w + x) * 3 + c]
      }
    }
  }
  return out
}

function beautyFace(px: Uint8ClampedArray, w: number, h: number, b: Box, op: BeautyOp) {
  const k = BEAUTY_K[op.level] ?? 0.8
  const smoothA = (op.smooth / 100) * k
  const whiten = (op.whiten / 100) * k
  const blem = op.blemish ? 0.12 : 0
  const eyeB = (op.eye_brighten / 100) * k
  const teeth = (op.teeth_whiten / 100) * k
  const dark = (op.dark_circles / 100) * k
  if (!smoothA && !whiten && !blem && !eyeB && !teeth && !dark) return
  const cx = b.x + b.w / 2
  const cy = b.y + b.h / 2
  const rx = Math.max(0, Math.floor(b.x - b.w * 0.15))
  const ry = Math.max(0, Math.floor(b.y - b.h * 0.15))
  const rw = Math.min(w - rx, Math.ceil(b.w * 1.3))
  const rh = Math.min(h - ry, Math.ceil(b.h * 1.3))
  if (rw < 4 || rh < 4) return
  const reg = new Float32Array(rw * rh * 3)
  for (let y = 0; y < rh; y++)
    for (let x = 0; x < rw; x++) {
      const i = ((ry + y) * w + rx + x) * 4
      const j = (y * rw + x) * 3
      reg[j] = px[i]
      reg[j + 1] = px[i + 1]
      reg[j + 2] = px[i + 2]
    }
  const radius = Math.max(1, Math.round(b.w * 0.07 * (0.5 + smoothA * 1.4)))
  const blurred = smoothA || blem ? boxBlur(reg, rw, rh, radius) : reg
  const eyeR = b.w * 0.13
  for (let y = 0; y < rh; y++)
    for (let x = 0; x < rw; x++) {
      const X = rx + x
      const Y = ry + y
      const u = (X - cx) / (b.w * 0.5)
      const v = (Y - cy) / (b.h * 0.55)
      const face = smooth((1.02 - Math.hypot(u, v)) * 3)
      const j = (y * rw + x) * 3
      const i = (Y * w + X) * 4
      let r = reg[j]
      let g = reg[j + 1]
      let bl = reg[j + 2]
      if (face > 0) {
        const a = Math.min(1, (smoothA + blem) * 1.1) * face
        if (a > 0) {
          r += (blurred[j] - r) * a
          g += (blurred[j + 1] - g) * a
          bl += (blurred[j + 2] - bl) * a
        }
        if (whiten) {
          const wv = whiten * 0.4 * face
          r += (255 - r) * wv
          g += (255 - g) * wv * 0.95
          bl += (255 - bl) * wv * 0.9
        }
      }
      const spot = (px0: number, py0: number, rad: number) => smooth(1 - Math.hypot(X - px0, Y - py0) / rad)
      if (eyeB || dark) {
        for (const sx of [-0.23, 0.23]) {
          const ex = cx + b.w * sx
          const e = eyeB ? spot(ex, b.y + b.h * 0.4, eyeR) * eyeB * 0.55 : 0
          const d = dark ? spot(ex, b.y + b.h * 0.5, eyeR * 1.3) * dark * 0.45 : 0
          const t = Math.min(1, e + d)
          r += (255 - r) * t
          g += (255 - g) * t
          bl += (255 - bl) * t
        }
      }
      if (teeth) {
        const t = spot(cx, b.y + b.h * 0.78, b.w * 0.16) * teeth * 0.7
        r += (255 - r) * t
        g += (255 - g) * t
        bl += (250 - bl) * t
      }
      px[i] = r
      px[i + 1] = g
      px[i + 2] = bl
    }
}

/** local -> beauty: skin retouching on the prepared faces. */
export function applyBeauty(px: Uint8ClampedArray, cw: number, ch: number, stack: EditStack, p: Photo, crop: [number, number, number, number]) {
  if (!isBeautyReady(p.id)) return
  for (const op of stack.ops) if (isBeauty(op)) for (const b of targets(op, p, crop, cw, ch)) beautyFace(px, cw, ch, b, op)
}
