import { describe, expect, it } from 'vitest'
import { splitContributions, tasteFeatureLabel, tasteLead, tasteNeeds, tastePercent, tasteStateText } from './taste'
import type { TasteMetrics } from '@/api/types'

describe('tasteNeeds', () => {
  it('counts what is missing in each class', () => {
    expect(tasteNeeds({ positives: 0, negatives: 0, min_per_class: 30 })).toEqual({ keep: 30, reject: 30, fraction: 0 })
    expect(tasteNeeds({ positives: 18, negatives: 25, min_per_class: 30 })).toEqual({ keep: 12, reject: 5, fraction: 43 / 60 })
    expect(tasteNeeds({ positives: 300, negatives: 31, min_per_class: 30 })).toEqual({ keep: 0, reject: 0, fraction: 1 })
  })
  it('tolerates odd input', () => {
    expect(tasteNeeds({ positives: -3, negatives: 2.7, min_per_class: 0 })).toEqual({ keep: 1, reject: 0, fraction: 0.5 })
  })
})

describe('taste helpers', () => {
  it('reports the lead over the generic score', () => {
    const m = { auc: 0.784, base_auc: 0.612 } as TasteMetrics
    expect(tasteLead(m)).toBe(17)
    expect(tasteLead({ ...m, auc: 0.6 })).toBe(-1)
  })
  it('labels features', () => {
    expect(tasteFeatureLabel('eyes_open')).toEqual({ key: 'taste.feat.eyes_open' })
    expect(tasteFeatureLabel('scene_food')).toEqual({ key: 'taste.feat.scene', params: { scene: 'food' } })
  })
  it('splits contributions', () => {
    const c = (feature: string, delta: number) => ({ feature, delta, direction: 'high' as const })
    const { pro, con } = splitContributions([c('a', 0.2), c('b', -0.5), c('c', 0.9), c('d', -0.1)])
    expect(pro.map((x) => x.feature)).toEqual(['c', 'a'])
    expect(con.map((x) => x.feature)).toEqual(['b', 'd'])
  })
  it('clamps percentages', () => {
    expect(tastePercent(0.826)).toBe(83)
    expect(tastePercent(1.2)).toBe(100)
    expect(tastePercent(-1)).toBe(0)
  })
})

describe('tasteStateText', () => {
  const base = { alpha: 0, positives: 10, negatives: 40, min_per_class: 30 }
  it('explains the cold start with what is missing', () => {
    expect(tasteStateText({ ...base, state: 'collecting' })).toEqual({ title: 'taste.inactive', hint: 'taste.needs', params: { keep: 20, reject: 0 } })
  })
  it('names the other states', () => {
    expect(tasteStateText({ ...base, state: 'active', alpha: 0.314 })).toEqual({ title: 'taste.active', hint: 'taste.activeHint', params: { alpha: 31 } })
    expect(tasteStateText({ ...base, state: 'not_better' }).title).toBe('taste.state_not_better')
    expect(tasteStateText({ ...base, state: 'disabled' }).hint).toBe('taste.state_disabled_hint')
    expect(tasteStateText({ ...base, state: 'ready' }).title).toBe('taste.state_ready')
  })
})
