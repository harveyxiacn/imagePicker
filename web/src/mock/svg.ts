import type { Photo } from '@/api/types'
import { faceInner } from './avatar'
import { avatarStyleOf, renderableFaces } from './ai'
import { burstKeyOf, mulberry32 } from './db'

/** Deterministic landscape-ish placeholder "photo" as SVG, sized to the photo's aspect. */
export function photoSvg(p: Photo, longEdge: number, label = true): string {
  const w0 = p.width ?? 6000
  const h0 = p.height ?? 4000
  const k = longEdge / Math.max(w0, h0)
  const w = Math.max(16, Math.round(w0 * k))
  const h = Math.max(16, Math.round(h0 * k))
  // burst neighbours share a hue so groups look alike
  const group = burstKeyOf(p.id)
  const rnd = mulberry32(group * 7919 + 13)
  const hue = Math.floor(rnd() * 360)
  const r2 = mulberry32(p.id * 104729 + 7)
  const sunX = (0.2 + r2() * 0.6) * w
  const sunY = (0.18 + r2() * 0.25) * h
  const sunR = (0.05 + r2() * 0.05) * Math.min(w, h)
  const hor = (0.55 + r2() * 0.15) * h
  let ridge = `M0 ${h} L0 ${hor}`
  const steps = 8
  for (let i = 1; i <= steps; i++) ridge += ` L${((i / steps) * w).toFixed(1)} ${(hor - r2() * 0.18 * h).toFixed(1)}`
  ridge += ` L${w} ${h} Z`
  const bright = 30 + Math.round(r2() * 20)
  const blur = (p.id % 11 === 0) ? `<filter id="b"><feGaussianBlur stdDeviation="${(w / 60).toFixed(1)}"/></filter>` : ''
  const filt = blur ? ' filter="url(#b)"' : ''
  // Analysed photos show their (synthetic) faces so crops, boxes and expressions are visible.
  let faces = ''
  for (const { face, pi } of renderableFaces(p.id)) {
    const [bx, by, bw, bh] = face.bbox
    const st = avatarStyleOf(pi)
    // faceInner is drawn in a 100x100 box where the head spans ~x22..78, y16..87
    faces +=
      `<svg x="${(bx * w).toFixed(1)}" y="${(by * h).toFixed(1)}" width="${(bw * w).toFixed(1)}" height="${(bh * h).toFixed(1)}" viewBox="14 8 72 86">` +
      faceInner(st, face.eyes_open ?? 1, face.smile ?? 0.3) +
      `</svg>`
  }
  const fs = Math.max(8, Math.round(h * 0.08))
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1">` +
    `<stop offset="0" stop-color="hsl(${hue} 55% ${bright}%)"/>` +
    `<stop offset="1" stop-color="hsl(${(hue + 45) % 360} 65% ${bright + 28}%)"/></linearGradient>${blur}</defs>` +
    `<g${filt}><rect width="${w}" height="${h}" fill="url(#g)"/>` +
    `<circle cx="${sunX.toFixed(1)}" cy="${sunY.toFixed(1)}" r="${sunR.toFixed(1)}" fill="hsl(${(hue + 20) % 360} 90% 85%)" opacity=".85"/>` +
    `<path d="${ridge}" fill="hsl(${(hue + 200) % 360} 30% ${12 + (p.id % 7)}%)" opacity=".9"/></g>` +
    (label
      ? `<text x="${Math.round(w * 0.04)}" y="${Math.round(h - h * 0.05)}" font-family="monospace" font-size="${fs}" fill="#fff" fill-opacity=".75">${p.file_name}</text>`
      : '') +
    faces +
    `</svg>`
  )
}

/** Normalised scene layout of a mock photo (same random sequence as `photoSvg`), used by the mock renderer / masks. */
export function photoGeometry(p: Photo): { sun: { x: number; y: number; r: number }; horizon: number; ridge: number[] } {
  const r2 = mulberry32(p.id * 104729 + 7)
  const sunX = 0.2 + r2() * 0.6
  const sunY = 0.18 + r2() * 0.25
  const sunR = 0.05 + r2() * 0.05
  const hor = 0.55 + r2() * 0.15
  const ridge = [hor]
  for (let i = 1; i <= 8; i++) ridge.push(hor - r2() * 0.18)
  return { sun: { x: sunX, y: sunY, r: sunR }, horizon: hor, ridge }
}
