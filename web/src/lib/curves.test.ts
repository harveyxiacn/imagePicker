import { describe, expect, it } from 'vitest'
import type { Point } from '@/api/types'
import {
  addCurvePoint,
  curveFn,
  curvePath,
  hitCurvePoint,
  IDENTITY_CURVE,
  isIdentityCurve,
  moveCurvePoint,
  removeCurvePoint,
  sampleCurve,
} from './curves'

describe('curve interpolation', () => {
  it('is the identity without points or with the default diagonal', () => {
    for (const x of [0, 0.25, 0.5, 1]) {
      expect(curveFn(undefined)(x)).toBeCloseTo(x)
      expect(curveFn([])(x)).toBeCloseTo(x)
      expect(curveFn(IDENTITY_CURVE)(x)).toBeCloseTo(x)
    }
    expect(isIdentityCurve([])).toBe(true)
    expect(isIdentityCurve(IDENTITY_CURVE)).toBe(true)
    expect(isIdentityCurve([[0, 0.1], [1, 1]])).toBe(false)
  })

  it('passes exactly through every control point', () => {
    const pts: Point[] = [
      [0, 0.05],
      [0.3, 0.2],
      [0.6, 0.7],
      [1, 0.95],
    ]
    const f = curveFn(pts)
    for (const [x, y] of pts) expect(f(x)).toBeCloseTo(y, 6)
  })

  it('is monotone for monotone control points (no overshoot)', () => {
    const pts: Point[] = [
      [0, 0],
      [0.1, 0.02],
      [0.5, 0.9],
      [0.55, 0.92],
      [1, 1],
    ]
    const ys = sampleCurve(pts, 257)
    for (let i = 1; i < ys.length; i++) expect(ys[i]).toBeGreaterThanOrEqual(ys[i - 1] - 1e-9)
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(0)
    expect(Math.max(...ys)).toBeLessThanOrEqual(1)
  })

  it('extends flat outside the first / last point and sorts unsorted input', () => {
    const f = curveFn([
      [0.8, 0.9],
      [0.2, 0.3],
    ])
    expect(f(0)).toBeCloseTo(0.3)
    expect(f(1)).toBeCloseTo(0.9)
    expect(f(0.5)).toBeGreaterThan(0.3)
  })

  it('draws an SVG path from the first to the last sample', () => {
    const d = curvePath(IDENTITY_CURVE, 100, 100, 4)
    expect(d.startsWith('M0.00 100.00')).toBe(true)
    expect(d.endsWith('L100.00 0.00')).toBe(true)
    expect(d.match(/L/g)).toHaveLength(4)
  })
})

describe('curve point editing', () => {
  it('adds points sorted by x and snaps onto an existing neighbour', () => {
    const { points, index } = addCurvePoint([], 0.4, 0.6)
    expect(points.map((p) => p[0])).toEqual([0, 0.4, 1])
    expect(index).toBe(1)
    const again = addCurvePoint(points, 0.405, 0.1)
    expect(again.points).toHaveLength(3)
    expect(again.index).toBe(1)
  })

  it('moves a point between its neighbours and clamps y; end points keep x', () => {
    const pts: Point[] = [
      [0, 0],
      [0.4, 0.5],
      [0.6, 0.6],
      [1, 1],
    ]
    expect(moveCurvePoint(pts, 1, 0.9, 2)[1]).toEqual([0.59, 1])
    expect(moveCurvePoint(pts, 1, -1, -1)[1]).toEqual([0.01, 0])
    expect(moveCurvePoint(pts, 0, 0.5, 0.3)[0]).toEqual([0, 0.3])
    expect(moveCurvePoint(pts, 3, 0.2, 0.7)[3]).toEqual([1, 0.7])
    // input untouched
    expect(pts[1]).toEqual([0.4, 0.5])
  })

  it('removes interior points only', () => {
    const pts: Point[] = [
      [0, 0],
      [0.5, 0.6],
      [1, 1],
    ]
    expect(removeCurvePoint(pts, 1)).toHaveLength(2)
    expect(removeCurvePoint(pts, 0)).toBe(pts)
    expect(removeCurvePoint(pts, 2)).toBe(pts)
  })

  it('hit-tests the nearest point within a radius', () => {
    const pts: Point[] = [
      [0, 0],
      [0.5, 0.5],
      [1, 1],
    ]
    expect(hitCurvePoint(pts, 0.52, 0.49, 0.05)).toBe(1)
    expect(hitCurvePoint(pts, 0.3, 0.3, 0.05)).toBe(-1)
  })
})
