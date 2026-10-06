import { describe, expect, it } from 'vitest'
import { appendPoint, clampRadius, containerToSource, finishStroke, frameToSource, MAX_RADIUS, radiusToDisplay, rectToFrame, sourceToFrame, strokesBody, strokesBounds } from './strokes'
import { imageRect, zoomAt, FIT_VIEW, type ViewState } from './zoom'

const CONTAINER = { w: 1000, h: 600 }
const IMG = { w: 3000, h: 2000 } // aspect 1.5
const ASPECT = 1.5
const close = (a: number[], b: number[], eps = 1e-6) => a.forEach((v, i) => expect(v).toBeCloseTo(b[i], -Math.log10(eps)))

describe('stroke capture -> normalised source coordinates', () => {
  it('maps the pane centre to the image centre at fit zoom', () => {
    close(containerToSource(FIT_VIEW, CONTAINER, IMG, undefined, ASPECT, 500, 300), [0.5, 0.5])
    // image fits by height: 900 x 600 centred -> left edge at x = 50
    close(containerToSource(FIT_VIEW, CONTAINER, IMG, undefined, ASPECT, 50, 0), [0, 0])
    close(containerToSource(FIT_VIEW, CONTAINER, IMG, undefined, ASPECT, 950, 600), [1, 1])
  })

  it('follows zoom and pan (zoomed view over the top-left quarter)', () => {
    // 2x zoom with the view centred on image point (0.25, 0.25)
    const view: ViewState = { zoom: 2, cx: 0.25, cy: 0.25 }
    // the pane centre shows the image point (0.25, 0.25)
    close(containerToSource(view, CONTAINER, IMG, undefined, ASPECT, 500, 300), [0.25, 0.25])
    // one displayed px = 1/ (fitScale*zoom*img.w) of the image width
    const r = imageRect(view, CONTAINER, IMG)
    close(containerToSource(view, CONTAINER, IMG, undefined, ASPECT, r.left + r.width * 0.4, r.top + r.height * 0.6), [0.4, 0.6])
  })

  it('stays consistent with the DOM-rect measurement used by the overlay', () => {
    const view = zoomAt(FIT_VIEW, 2.5, 420, 210, CONTAINER, IMG)
    const r = imageRect(view, CONTAINER, IMG)
    const [px, py] = [610, 333]
    const dom = rectToFrame(r, px, py) // overlay measures its on-screen rect
    const pure = containerToSource(view, CONTAINER, IMG, undefined, ASPECT, px, py)
    close(dom, pure)
  })

  it('maps through an un-rotated crop', () => {
    const crop = { rect: [0.2, 0.1, 0.5, 0.6] as [number, number, number, number], angle: 0 }
    close(frameToSource(0, 0, crop, ASPECT), [0.2, 0.1])
    close(frameToSource(1, 1, crop, ASPECT), [0.7, 0.7])
    close(frameToSource(0.5, 0.5, crop, ASPECT), [0.45, 0.4])
    // crop + zoom: frame image is the crop window, so the pane centre at fit is the window centre
    close(containerToSource(FIT_VIEW, CONTAINER, { w: 1500, h: 1200 }, crop, ASPECT, 500, 300), [0.45, 0.4])
  })

  it('round-trips source <-> frame with crop and straighten', () => {
    const crop = { rect: [0.1, 0.15, 0.7, 0.6] as [number, number, number, number], angle: 7 }
    for (const [x, y] of [
      [0.5, 0.5],
      [0.3, 0.4],
      [0.62, 0.55],
    ]) {
      const [u, v] = sourceToFrame(x, y, crop, ASPECT)
      close(frameToSource(u, v, crop, ASPECT), [x, y], 1e-9)
    }
  })

  it('a straighten rotation about the centre leaves the centre fixed and moves other points', () => {
    const crop = { rect: [0, 0, 1, 1] as [number, number, number, number], angle: 10 }
    close(frameToSource(0.5, 0.5, crop, ASPECT), [0.5, 0.5])
    const [x, y] = frameToSource(0.8, 0.5, crop, ASPECT)
    expect(y).not.toBeCloseTo(0.5, 2)
    expect(x).toBeLessThan(0.8)
  })

  it('clamps points outside the image', () => {
    expect(frameToSource(-0.2, 1.4, undefined, ASPECT)).toEqual([0, 1])
  })

  it('converts the radius to display px relative to the crop window', () => {
    // radius 0.05 of the source width on a frame showing half of the width: 0.1 of the frame
    expect(radiusToDisplay(0.05, { rect: [0, 0, 0.5, 1] }, 800)).toBeCloseTo(80)
    expect(radiusToDisplay(0.05, undefined, 800)).toBeCloseTo(40)
  })
})

describe('stroke building', () => {
  it('drops points closer than a fraction of the radius and keeps the spacing otherwise', () => {
    let pts = appendPoint([], [0.5, 0.5], 0.02, ASPECT)
    pts = appendPoint(pts, [0.5005, 0.5], 0.02, ASPECT) // too close
    pts = appendPoint(pts, [0.52, 0.5], 0.02, ASPECT)
    expect(pts).toHaveLength(2)
  })

  it('rounds coordinates to 4 decimals and clamps the radius', () => {
    const s = finishStroke([[0.123456789, 0.987654321]], 5)!
    expect(s.points[0]).toEqual([0.1235, 0.9877])
    expect(s.radius).toBe(MAX_RADIUS)
    expect(finishStroke([], 0.02)).toBeNull()
    expect(clampRadius(0)).toBeGreaterThan(0)
  })

  it('computes the bounds of all strokes (radius is a fraction of the width)', () => {
    const b = strokesBounds([{ points: [[0.5, 0.5]], radius: 0.1 }], 1.5)!
    expect(b[0]).toBeCloseTo(0.4)
    expect(b[2]).toBeCloseTo(0.2)
    // y extent = radius * aspect on each side
    expect(b[3]).toBeCloseTo(0.3)
    expect(strokesBounds([], 1.5)).toBeNull()
  })

  it('builds the API body without empty strokes', () => {
    expect(strokesBody([{ points: [], radius: 0.02 }, { points: [[0.1, 0.2]], radius: 0.02 }])).toEqual({ strokes: [{ points: [[0.1, 0.2]], radius: 0.02 }] })
  })
})
