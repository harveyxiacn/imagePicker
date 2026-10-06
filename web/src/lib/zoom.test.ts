import { describe, expect, it } from 'vitest'
import { clampView, FIT_VIEW, fitScale, hundredZoom, imageRect, panBy, pointToNorm, toggleFit100, zoomAt } from './zoom'

const c = { w: 1000, h: 500 }
const img = { w: 6000, h: 4000 }

describe('zoom maths', () => {
  it('fit scale is the limiting dimension', () => {
    expect(fitScale(c, img)).toBeCloseTo(0.125)
    expect(hundredZoom(c, img)).toBeCloseTo(8)
  })

  it('fit view centres the image', () => {
    const r = imageRect(FIT_VIEW, c, img)
    expect(r.width).toBeCloseTo(750)
    expect(r.height).toBeCloseTo(500)
    expect(r.left).toBeCloseTo(125)
  })

  it('wheel zoom keeps the point under the cursor fixed', () => {
    const px = 600
    const py = 200
    const before = pointToNorm(FIT_VIEW, c, img, px, py)
    const v = zoomAt(FIT_VIEW, 3, px, py, c, img)
    const after = pointToNorm(v, c, img, px, py)
    expect(v.zoom).toBeCloseTo(3)
    expect(after.nx).toBeCloseTo(before.nx, 5)
    expect(after.ny).toBeCloseTo(before.ny, 5)
  })

  it('never zooms out below fit or beyond the max', () => {
    expect(zoomAt(FIT_VIEW, 0.2, 500, 250, c, img).zoom).toBe(1)
    expect(zoomAt(FIT_VIEW, 1e6, 500, 250, c, img).zoom).toBeCloseTo(32)
  })

  it('clamps the pan so the image always covers the container', () => {
    const v = clampView({ zoom: 4, cx: -5, cy: 9 }, c, img)
    const r = imageRect(v, c, img)
    expect(r.left).toBeLessThanOrEqual(1e-6)
    expect(r.left + r.width).toBeGreaterThanOrEqual(c.w - 1e-6)
    expect(r.top).toBeLessThanOrEqual(1e-6)
    expect(r.top + r.height).toBeGreaterThanOrEqual(c.h - 1e-6)
  })

  it('panBy moves the image with the drag', () => {
    const z = zoomAt(FIT_VIEW, 4, 500, 250, c, img)
    const before = imageRect(z, c, img)
    const moved = imageRect(panBy(z, 40, 0, c, img), c, img)
    expect(moved.left - before.left).toBeCloseTo(40)
  })

  it('toggles between fit and 100%', () => {
    const h = toggleFit100(FIT_VIEW, c, img)
    expect(h.zoom).toBeCloseTo(8)
    expect(toggleFit100(h, c, img)).toEqual(FIT_VIEW)
  })
})
