import { describe, expect, it } from 'vitest'
import type { EditStack, GlobalOp, LutOp, OutputSharpenOp } from '@/api/types'
import { appliedPreset, applyPreset, getAdjust, parsePresetSource, PRESET_AMOUNT_MAX, scalePresetStack } from './edit'

const preset: EditStack = {
  version: 1,
  ops: [
    {
      type: 'global',
      exposure: 0.4,
      contrast: -14,
      temp: -170,
      curve: { rgb: [[0, 0.05], [0.5, 0.6], [1, 1]] },
      hsl: { green: { h: -12, s: -18, l: 8 } },
      grading: { shadows: [205, 0.4], midtones: [0, 0], highlights: [45, 0.2], balance: 10 },
      source: 'preset:japanese_clean;amount=100',
    },
    { type: 'lut', file: 'film_warm', amount: 0.8 },
    { type: 'output_sharpen', amount: 8 },
  ],
}

const glob = (s: EditStack) => s.ops.find((o) => o.type === 'global') as GlobalOp
const lut = (s: EditStack) => s.ops.find((o) => o.type === 'lut') as LutOp | undefined

describe('preset strength', () => {
  it('100 % is the preset itself', () => {
    expect(scalePresetStack(preset, 1)).toBe(preset)
  })

  it('scales sliders, curves, HSL, grading and amounts', () => {
    const half = scalePresetStack(preset, 0.5)
    const g = glob(half)
    expect(g.exposure).toBeCloseTo(0.2)
    expect(g.contrast).toBe(-7)
    expect(g.temp).toBe(-85)
    expect(g.curve?.rgb?.[0][1]).toBeCloseTo(0.025)
    expect(g.curve?.rgb?.[1][1]).toBeCloseTo(0.55)
    expect(g.hsl?.green).toEqual({ h: -6, s: -9, l: 4 })
    expect(g.grading?.shadows).toEqual([205, 0.2])
    expect(g.grading?.balance).toBe(5)
    expect(lut(half)?.amount).toBeCloseTo(0.4)
    expect((half.ops.find((o) => o.type === 'output_sharpen') as OutputSharpenOp).amount).toBe(4)
    // provenance is kept (the editor rewrites the amount)
    expect(g.source).toBe('preset:japanese_clean;amount=100')
  })

  it('150 % stays inside the valid ranges and 0 % removes the look', () => {
    const strong = scalePresetStack(preset, 9)
    expect(glob(strong).exposure).toBeCloseTo(0.4 * PRESET_AMOUNT_MAX)
    expect(lut(strong)?.amount).toBe(1)
    expect(glob(strong).grading?.shadows?.[1]).toBeCloseTo(0.6)
    expect(scalePresetStack(preset, 0).ops).toEqual([])
  })

  it('parses the provenance the server writes', () => {
    expect(parsePresetSource('preset:japanese_clean;amount=80')).toEqual({ id: 'japanese_clean', amount: 0.8 })
    expect(parsePresetSource('preset:user_3')).toEqual({ id: 'user_3', amount: 1 })
    expect(parsePresetSource('preset')).toBeNull()
    expect(parsePresetSource('ai_auto@1')).toBeNull()
    expect(parsePresetSource(undefined)).toBeNull()
    expect(parsePresetSource('preset:x;amount=abc')).toBeNull()
  })

  it('appliedPreset reads the stack the panel drives the slider from', () => {
    const base: EditStack = { version: 1, ops: [{ type: 'crop', rect: [0, 0, 0.5, 0.5], angle: 0 }] }
    expect(appliedPreset(base)).toBeNull()
    const s = applyPreset(base, scalePresetStack(preset, 0.5))
    expect(appliedPreset(s)).toEqual({ id: 'japanese_clean', amount: 1 })
    expect(getAdjust(s).exposure).toBeCloseTo(0.2)
  })
})
