import type { MaskRef } from '@/api/types'

const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v)
const smooth = (t: number) => {
  const x = clamp01(t)
  return x * x * (3 - 2 * x)
}

/**
 * Mask strength (0..1) of a geometric mask at a normalised point, following the contract (A):
 * radial = full effect inside the feathered ellipse; linear = full at `start`, none at `end`.
 * AI masks are bitmaps and return 0 here.
 */
export function maskValue(m: MaskRef, nx: number, ny: number): number {
  if (m.kind === 'radial') {
    const rx = Math.max(0.01, m.radius[0])
    const ry = Math.max(0.01, m.radius[1])
    const d = Math.hypot((nx - m.center[0]) / rx, (ny - m.center[1]) / ry)
    const inner = 1 - clamp01(m.feather ?? 0.5)
    if (d <= inner) return 1
    if (d >= 1) return 0
    return 1 - smooth((d - inner) / Math.max(1e-3, 1 - inner))
  }
  if (m.kind === 'linear') {
    const dx = m.end[0] - m.start[0]
    const dy = m.end[1] - m.start[1]
    const len2 = Math.max(1e-6, dx * dx + dy * dy)
    return 1 - smooth(((nx - m.start[0]) * dx + (ny - m.start[1]) * dy) / len2)
  }
  return 0
}
