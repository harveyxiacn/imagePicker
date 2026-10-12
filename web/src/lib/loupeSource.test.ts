import { describe, expect, it } from 'vitest'
import { cssImageSize, loupeSource, snapRect, UPSCALE_SLACK } from './loupeSource'
import { clampView, FIT_VIEW, imageRect, toggleFit100, viewAt100, zoomAt } from './zoom'

const c = { w: 1600, h: 900 }
const native = { w: 6000, h: 4000 } // 24 MP

/** What the loupe shows for a view, as ZoomPane computes it. */
function shown(view: { zoom: number; cx: number; cy: number }, dpr: number, nat = native) {
  const img = cssImageSize(nat, dpr)
  const v = clampView(view, c, img)
  const rect = snapRect(imageRect(v, c, img), dpr)
  return { rect, img, source: loupeSource(v.zoom, Math.max(rect.width, rect.height) * dpr, Math.max(nat.w, nat.h)) }
}

describe('loupe source', () => {
  it('stays on the preloaded 2048 preview at fit, whatever the screen', () => {
    expect(loupeSource(1, 1350, 6000)).toBe(2048)
    expect(loupeSource(1, 3000, 6000)).toBe(2048) // fit on a HiDPI screen
    expect(loupeSource(1, 1350, null)).toBe(2048)
    expect(shown(FIT_VIEW, 2).source).toBe(2048)
  })

  it('moves up to the 4096 preview, then to the full image, as the zoom grows', () => {
    expect(loupeSource(1.2, 2048 * UPSCALE_SLACK, 6000)).toBe(2048)
    expect(loupeSource(1.5, 2048 * UPSCALE_SLACK + 1, 6000)).toBe(4096)
    expect(loupeSource(3, 4096 * UPSCALE_SLACK, 6000)).toBe(4096)
    expect(loupeSource(3.5, 4096 * UPSCALE_SLACK + 1, 6000)).toBe('full')
    expect(loupeSource(9, 24000, 6000)).toBe('full') // 400%
  })

  it('always shows the photo’s own pixels at 100%, even just above a preview size', () => {
    // 4300 px is within the upscale slack of the 4096 preview, but 100% must be native
    expect(loupeSource(4, 4300, 4300)).toBe('full')
    expect(loupeSource(4, 4250, 4300)).toBe(4096) // 99%: a 5% enlargement is fine
    // the 4096 preview of a 12 MP photo IS the photo: no full-resolution request
    expect(loupeSource(4, 4000, 4000)).toBe(4096)
    expect(loupeSource(8, 8000, 4000)).toBe(4096)
    // small photos never need more than the 2048 preview
    expect(loupeSource(3, 1600, 1600)).toBe(2048)
    // unknown size: fetch the full image once past the 4096 preview
    expect(loupeSource(4, 4096 * UPSCALE_SLACK + 1, null)).toBe('full')
  })
})

describe('100% is one image pixel per device pixel', () => {
  for (const dpr of [1, 1.25, 1.5, 2]) {
    it(`at devicePixelRatio ${dpr}`, () => {
      const img = cssImageSize(native, dpr)
      const v = toggleFit100(FIT_VIEW, c, img, 700, 300)
      const { rect, source } = shown(v, dpr)
      expect(rect.width * dpr).toBeCloseTo(6000, 6)
      expect(rect.height * dpr).toBeCloseTo(4000, 6)
      // whole device pixels: the browser draws the full image without resampling
      expect(Number.isInteger(Math.round(rect.left * dpr * 1e6) / 1e6)).toBe(true)
      expect(Number.isInteger(Math.round(rect.top * dpr * 1e6) / 1e6)).toBe(true)
      expect(source).toBe('full')
      // the zoom badge (rect.width / img.w) reads 100%
      expect(Math.round((rect.width / img.w) * 100)).toBe(100)
      // hold-to-zoom (Z) is the same 100%
      expect(imageRect(viewAt100(0.3, 0.6, c, img), c, img).width * dpr).toBeCloseTo(6000, 6)
    })
  }

  it('keeps wheel zoom anchored and synced views comparable', () => {
    const img = cssImageSize(native, 1.5)
    const v = zoomAt(FIT_VIEW, 2, 400, 300, c, img)
    expect(v.zoom).toBeCloseTo(2)
    // a view is relative to fit, so Compare can share it between photos of different sizes
    const other = cssImageSize({ w: 4000, h: 3000 }, 1.5)
    expect(clampView(v, c, other).zoom).toBeCloseTo(2)
  })
})

describe('snapRect / cssImageSize', () => {
  it('rounds to device pixels', () => {
    expect(snapRect({ left: 10.3, top: -0.6, width: 99.9, height: 50.5 }, 1)).toEqual({ left: 10, top: -1, width: 100, height: 51 })
    expect(snapRect({ left: 10.3, top: 0.26, width: 99.9, height: 50.1 }, 2)).toEqual({ left: 10.5, top: 0.5, width: 100, height: 50 })
  })

  it('snaps to the page grid when the container sits between device pixels', () => {
    const origin = { x: 300.25, y: 45.34375 }
    for (const dpr of [1, 1.25, 1.5, 2]) {
      const r = snapRect({ left: -1280.4, top: -879.7, width: 4000.2, height: 2666.4 }, dpr, origin)
      for (const v of [(origin.x + r.left) * dpr, (origin.y + r.top) * dpr, r.width * dpr, r.height * dpr]) {
        expect(Math.abs(v - Math.round(v))).toBeLessThan(1e-6)
      }
      // moved by less than a device pixel
      expect(Math.abs(r.top + 879.7) * dpr).toBeLessThanOrEqual(0.5 + 1e-9)
    }
  })

  it('ignores a missing or broken ratio', () => {
    expect(cssImageSize(native, 0)).toEqual(native)
    expect(cssImageSize(native, Number.NaN)).toEqual(native)
    expect(snapRect({ left: 1.4, top: 0, width: 2, height: 2 }, Number.NaN).left).toBe(1)
  })
})
