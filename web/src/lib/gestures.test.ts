import { describe, expect, it } from 'vitest'
import {
  classifySwipe,
  clampPan,
  dominantAxis,
  isTap,
  liveDirection,
  nextTap,
  pinchZoom,
  swipeProgress,
  zoomAround,
  DEFAULT_SWIPE,
} from './gestures'

describe('classifySwipe', () => {
  it('commits on distance in each direction', () => {
    expect(classifySwipe({ dx: -90, dy: 5, dt: 400 })).toBe('left')
    expect(classifySwipe({ dx: 90, dy: -5, dt: 400 })).toBe('right')
    expect(classifySwipe({ dx: 4, dy: -90, dt: 400 })).toBe('up')
    expect(classifySwipe({ dx: -4, dy: 90, dt: 400 })).toBe('down')
  })

  it('ignores short slow drags', () => {
    expect(classifySwipe({ dx: -40, dy: 0, dt: 600 })).toBeNull()
    expect(classifySwipe({ dx: 0, dy: -63, dt: 900 })).toBeNull()
  })

  it('commits a short fast flick by velocity but not below the minimum flick distance', () => {
    expect(classifySwipe({ dx: -30, dy: 0, dt: 50 })).toBe('left') // 0.6 px/ms
    expect(classifySwipe({ dx: -20, dy: 0, dt: 10 })).toBeNull() // too short to be intentional
    expect(classifySwipe({ dx: -30, dy: 0, dt: 200 })).toBeNull() // too slow
  })

  it('rejects diagonal movement (axis dominance)', () => {
    expect(classifySwipe({ dx: -80, dy: 70, dt: 300 })).toBeNull()
    expect(dominantAxis(80, 70)).toBeNull()
    expect(dominantAxis(100, 60)).toBe('x')
    expect(dominantAxis(40, -100)).toBe('y')
    expect(dominantAxis(0, 0)).toBeNull()
  })

  it('conflicts: pinch, zoomed-in pan, scrollable region and long-press never swipe', () => {
    expect(classifySwipe({ dx: -120, dy: 0, dt: 200, pointers: 2 })).toBeNull()
    expect(classifySwipe({ dx: -120, dy: 0, dt: 200, zoom: 2.5 })).toBeNull()
    expect(classifySwipe({ dx: -120, dy: 0, dt: 200, zoom: 1 })).toBe('left')
    expect(classifySwipe({ dx: 0, dy: -120, dt: 200, scrollable: true })).toBeNull()
    expect(classifySwipe({ dx: 120, dy: 0, dt: 700, longPressed: true })).toBeNull()
  })

  it('honours custom thresholds', () => {
    const strict = { ...DEFAULT_SWIPE, distance: 200 }
    expect(classifySwipe({ dx: -120, dy: 0, dt: 600 }, strict)).toBeNull()
    expect(classifySwipe({ dx: -220, dy: 0, dt: 600 }, strict)).toBe('left')
  })
})

describe('live feedback', () => {
  it('reports progress toward the committing distance', () => {
    expect(swipeProgress(0, -32, 'up')).toBeCloseTo(0.5)
    expect(swipeProgress(0, -200, 'up')).toBe(1)
    expect(swipeProgress(0, 40, 'up')).toBe(0)
    expect(swipeProgress(50, 0, 'up')).toBe(0) // wrong axis
    expect(swipeProgress(0, 64, 'down')).toBe(1)
  })

  it('liveDirection waits for a clear direction', () => {
    expect(liveDirection(3, 2)).toBeNull()
    expect(liveDirection(-30, 4)).toBe('left')
    expect(liveDirection(0, 30)).toBe('down')
    expect(liveDirection(30, 28)).toBeNull()
  })
})

describe('taps', () => {
  it('a still short press is a tap, a long or moving one is not', () => {
    expect(isTap(2, 3, 120)).toBe(true)
    expect(isTap(2, 3, 700)).toBe(false)
    expect(isTap(30, 0, 100)).toBe(false)
  })

  it('detects double tap only for close, quick taps and then resets', () => {
    let r = nextTap(null, { x: 100, y: 100, t: 1000 })
    expect(r.double).toBe(false)
    r = nextTap(r.prev, { x: 110, y: 104, t: 1200 })
    expect(r.double).toBe(true)
    expect(r.prev).toBeNull()
    r = nextTap(r.prev, { x: 100, y: 100, t: 1300 })
    expect(r.double).toBe(false)
    const far = nextTap({ x: 0, y: 0, t: 1000 }, { x: 200, y: 0, t: 1100 })
    expect(far.double).toBe(false)
    const slow = nextTap({ x: 0, y: 0, t: 1000 }, { x: 0, y: 0, t: 1600 })
    expect(slow.double).toBe(false)
  })
})

describe('pinch / pan math', () => {
  it('scales proportionally and clamps', () => {
    expect(pinchZoom(1, 100, 200)).toBe(2)
    expect(pinchZoom(2, 100, 50)).toBe(1)
    expect(pinchZoom(1, 100, 1000)).toBe(5)
    expect(pinchZoom(1.5, 0, 50)).toBe(1.5)
  })

  it('clamps panning to the zoomed image bounds', () => {
    expect(clampPan(500, -500, 2, { w: 400, h: 800 })).toEqual({ tx: 200, ty: -400 })
    expect(clampPan(30, 30, 1, { w: 400, h: 800 })).toEqual({ tx: 0, ty: 0 })
  })

  it('zooms around the focus point', () => {
    // zooming 1 -> 2 about the container centre keeps the offset at zero
    expect(zoomAround(0, 0, 1, 2, { x: 0, y: 0 })).toEqual({ tx: 0, ty: 0 })
    // about a point 100px right of centre the image shifts left so that point stays put
    expect(zoomAround(0, 0, 1, 2, { x: 100, y: 0 }).tx).toBe(-100)
  })
})
