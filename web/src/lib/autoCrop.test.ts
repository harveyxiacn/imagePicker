import { describe, expect, it } from 'vitest'
import type { AutoCropResult, EditStack } from '@/api/types'
import { applyAutoCrop, autoCropBody, fmtDeg } from './autoCrop'
import { emptyStack, getCrop, inscribedRect, setCrop } from './edit'

const res = (over: Partial<AutoCropResult> = {}): AutoCropResult => ({
  crop: { rect: [0.05, 0.08, 0.9, 0.84], angle: -3 },
  source: 'stored',
  tilt_deg: -3,
  confidence: 0.7,
  level: false,
  ...over,
})

describe('autoCropBody', () => {
  it('sends the pixel ratio of the chosen aspect (4:5 on a landscape photo is 5:4) and the current crop', () => {
    expect(autoCropBody('straighten', undefined, 1.5)).toEqual({ kind: 'straighten', ratio: 1.5 })
    const st = setCrop(emptyStack(), { rect: [0.1, 0.1, 0.5, 0.5], angle: 2, aspect: '4:5' })
    expect(autoCropBody('compose', getCrop(st), 1.5)).toEqual({ kind: 'compose', ratio: 1.25, rect: [0.1, 0.1, 0.5, 0.5], angle: 2 })
  })
  it('leaves the ratio out for free crops and flips fixed ratios on portrait photos', () => {
    const free = setCrop(emptyStack(), { rect: [0, 0, 0.6, 0.6], aspect: 'free' })
    expect(autoCropBody('straighten', getCrop(free), 1.5).ratio).toBeUndefined()
    const sq = setCrop(emptyStack(), { rect: [0, 0, 1, 0.9], aspect: '16:9' })
    expect(autoCropBody('compose', getCrop(sq), 0.75).ratio).toBeCloseTo(9 / 16, 6)
  })
})

describe('applyAutoCrop', () => {
  it('sets rect and angle, keeping the chosen aspect (default original)', () => {
    const { stack, outcome } = applyAutoCrop(emptyStack(), res())
    expect(outcome).toBe('applied')
    expect(getCrop(stack)).toMatchObject({ rect: [0.05, 0.08, 0.9, 0.84], angle: -3, aspect: 'original' })
    const had = setCrop(emptyStack(), { rect: [0, 0, 0.9, 0.9], aspect: '1:1' })
    expect(getCrop(applyAutoCrop(had, res()).stack)?.aspect).toBe('1:1')
  })
  it('does nothing without a confident tilt', () => {
    const r = applyAutoCrop(emptyStack(), res({ crop: null, source: 'none', tilt_deg: null, confidence: null }))
    expect(r.outcome).toBe('none')
    expect(r.stack).toEqual(emptyStack())
  })
  it('leaves level photos untouched but removes a leftover rotation', () => {
    const level = res({ crop: { rect: [0, 0, 1, 1], angle: 0 }, level: true, tilt_deg: 0.05 })
    const r = applyAutoCrop(emptyStack(), level)
    expect(r.outcome).toBe('level')
    expect(r.stack.ops).toHaveLength(0)
    const rotated: EditStack = setCrop(emptyStack(), { rect: [0.1, 0.1, 0.8, 0.8], angle: 4 })
    const fixed = applyAutoCrop(rotated, level)
    expect(fixed.outcome).toBe('applied')
    expect(getCrop(fixed.stack)).toBeUndefined() // a full, unrotated crop is the same as no crop
  })
  it('reports an identical suggestion as unchanged', () => {
    const first = applyAutoCrop(emptyStack(), res()).stack
    expect(applyAutoCrop(first, res()).outcome).toBe('unchanged')
  })
})

describe('inscribedRect (mock server)', () => {
  it('matches the closed form', () => {
    expect(inscribedRect(300, 200, 0, 1.5)).toEqual([0, 0, 1, 1])
    const r = inscribedRect(1000, 1000, 45, 1)
    expect(r[2]).toBeCloseTo(1 / Math.SQRT2, 9)
  })
  it('keeps the aspect and stays inside the rotated image', () => {
    for (const deg of [-10, -2, 3, 7]) {
      const [x, y, w, h] = inscribedRect(1200, 800, deg, 1.5)
      expect((w * 1200) / (h * 800)).toBeCloseTo(1.5, 9)
      const th = (deg * Math.PI) / 180
      for (const [fx, fy] of [[x, y], [x + w, y], [x, y + h], [x + w, y + h]]) {
        const dx = (fx - 0.5) * 1200
        const dy = (fy - 0.5) * 800
        expect(Math.abs(dx * Math.cos(th) - dy * Math.sin(th))).toBeLessThanOrEqual(600 + 1e-6)
        expect(Math.abs(dx * Math.sin(th) + dy * Math.cos(th))).toBeLessThanOrEqual(400 + 1e-6)
      }
    }
  })
})

describe('fmtDeg', () => {
  it('rounds to one decimal without trailing zeros', () => {
    expect(fmtDeg(-3)).toBe('-3')
    expect(fmtDeg(2.449)).toBe('2.4')
  })
})
