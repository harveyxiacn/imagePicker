import type { MaskTarget, Photo } from '@/api/types'
import { renderableFaces } from './ai'
import { photoGeometry } from './svg'

/** Models required before a target can be generated (mock of the worker's lazy model fetch). */
export const MASK_MODELS: Record<MaskTarget, string> = {
  sky: 'sky-seg',
  subject: 'sam2-tiny',
  background: 'sam2-tiny',
  person: 'sam2-tiny',
  skin: 'sam2-tiny',
  hair: 'sam2-tiny',
  clothes: 'sam2-tiny',
}

type Ctx = OffscreenCanvasRenderingContext2D

/**
 * Draws a grey mask (white = target) for a mock photo into a W x H context using the photo's own
 * scene layout (sky above the ridge line, subject around the faces / centre).
 */
export function drawMask(ctx: Ctx, target: MaskTarget, p: Photo, W: number, H: number): void {
  const g = photoGeometry(p)
  const faces = renderableFaces(p.id).map((f) => f.face.bbox)
  const blur = Math.max(2, Math.round(Math.min(W, H) * 0.012))
  ctx.fillStyle = '#000'
  ctx.fillRect(0, 0, W, H)
  ctx.save()
  ctx.filter = `blur(${blur}px)`
  ctx.fillStyle = '#fff'
  const ellipse = (cx: number, cy: number, rx: number, ry: number) => {
    ctx.beginPath()
    ctx.ellipse(cx * W, cy * H, rx * W, ry * H, 0, 0, Math.PI * 2)
    ctx.fill()
  }
  const sky = () => {
    ctx.beginPath()
    ctx.moveTo(0, 0)
    ctx.lineTo(W, 0)
    for (let i = 8; i >= 0; i--) ctx.lineTo((i / 8) * W, g.ridge[i] * H)
    ctx.closePath()
    ctx.fill()
  }
  const subject = () => {
    if (faces.length) {
      for (const [x, y, w, h] of faces) ellipse(x + w / 2, y + h * 0.9, Math.max(w * 1.1, 0.06), Math.max(h * 1.6, 0.12))
    } else ellipse(0.5, 0.62, 0.2, 0.28)
  }
  switch (target) {
    case 'sky':
      sky()
      break
    case 'subject':
      subject()
      break
    case 'background':
      // inverse of the subject: white everywhere, subject shapes punched out in black
      ctx.restore()
      ctx.save()
      ctx.fillStyle = '#fff'
      ctx.fillRect(0, 0, W, H)
      ctx.filter = `blur(${blur}px)`
      ctx.fillStyle = '#000'
      subject()
      break
    case 'person':
      if (faces.length) for (const [x, y, w, h] of faces) ellipse(x + w / 2, y + h * 1.4, Math.max(w * 1.5, 0.08), Math.max(h * 2.2, 0.16))
      else ellipse(0.5, 0.65, 0.16, 0.3)
      break
    case 'skin':
      if (faces.length) for (const [x, y, w, h] of faces) ellipse(x + w / 2, y + h * 0.58, w * 0.4, h * 0.38)
      else ellipse(0.5, 0.45, 0.07, 0.1)
      break
    case 'hair':
      if (faces.length) for (const [x, y, w, h] of faces) ellipse(x + w / 2, y + h * 0.16, w * 0.5, h * 0.22)
      else ellipse(0.5, 0.33, 0.09, 0.07)
      break
    case 'clothes':
      if (faces.length) for (const [x, y, w, h] of faces) ellipse(x + w / 2, y + h * 1.9, w * 0.9, h * 0.8)
      else ellipse(0.5, 0.78, 0.17, 0.14)
      break
  }
  ctx.restore()
}

export async function maskPng(p: Photo, target: MaskTarget, longEdge = 512): Promise<Blob> {
  const ar = (p.width ?? 3) / (p.height ?? 2)
  const W = ar >= 1 ? longEdge : Math.round(longEdge * ar)
  const H = ar >= 1 ? Math.round(longEdge / ar) : longEdge
  const c = new OffscreenCanvas(W, H)
  const ctx = c.getContext('2d')!
  drawMask(ctx, target, p, W, H)
  return c.convertToBlob({ type: 'image/png' })
}
