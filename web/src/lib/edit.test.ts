import { describe, expect, it } from 'vitest'
import type { EditStack, GlobalOp } from '@/api/types'
import {
  addLocal,
  applyPreset,
  applySections,
  aspectRatio,
  beforeRequest,
  cropToolStack,
  defaultMask,
  diffAdjust,
  dragCropRect,
  emptyStack,
  extractSections,
  fitAspect,
  getAdjust,
  getCrop,
  getGlobal,
  isEmptyStack,
  localOps,
  mergeAuto,
  normalizeCropRect,
  normalizeStack,
  removeLocal,
  setCrop,
  setCurve,
  setGlobalField,
  setHsl,
  setLocalField,
  setSharpen,
  stackKey,
  stacksEqual,
  updateGlobal,
  updateLocal,
} from './edit'

const global = (s: EditStack) => getGlobal(s) as GlobalOp

describe('global field reducer', () => {
  it('creates the global op on first change and clamps to the slider range', () => {
    let s = setGlobalField(emptyStack(), 'exposure', 0.35)
    expect(global(s)).toMatchObject({ type: 'global', exposure: 0.35 })
    s = setGlobalField(s, 'exposure', 99)
    expect(global(s).exposure).toBe(5)
    s = setGlobalField(s, 'temp', -9999)
    expect(global(s).temp).toBe(-3000)
    s = setGlobalField(s, 'contrast', 500)
    expect(global(s).contrast).toBe(100)
  })

  it('does not mutate its input', () => {
    const s = setGlobalField(emptyStack(), 'contrast', 12)
    const before = JSON.stringify(s)
    setGlobalField(s, 'contrast', 40)
    expect(JSON.stringify(s)).toBe(before)
  })

  it('returns to the empty stack when every slider is back at neutral', () => {
    let s = setGlobalField(emptyStack(), 'exposure', 1)
    s = setGlobalField(s, 'saturation', -20)
    s = setGlobalField(s, 'exposure', 0)
    s = setGlobalField(s, 'saturation', 0)
    expect(s.ops).toEqual([])
    expect(isEmptyStack(s)).toBe(true)
  })

  it('keeps HSL / curve / grading-only edits and drops identity ones', () => {
    let s = updateGlobal(emptyStack(), (a) => setHsl(a, 'orange', 's', 30))
    expect(global(s).hsl).toEqual({ orange: { h: 0, s: 30, l: 0 } })
    s = updateGlobal(s, (a) => setHsl(a, 'orange', 's', 0))
    expect(s.ops).toEqual([])
    s = updateGlobal(emptyStack(), (a) =>
      setCurve(a, 'rgb', [
        [0, 0],
        [1, 1],
      ]),
    )
    expect(s.ops).toEqual([])
    s = updateGlobal(emptyStack(), (a) =>
      setCurve(a, 'rgb', [
        [0, 0],
        [0.5, 0.65],
        [1, 1],
      ]),
    )
    expect(global(s).curve?.rgb).toHaveLength(3)
  })

  it('preserves unknown (M4/M5) ops verbatim', () => {
    const warp = { type: 'warp', mesh: [1, 2, 3] }
    const s = setGlobalField({ version: 1, ops: [warp] }, 'exposure', 0.5)
    expect(s.ops).toContainEqual(warp)
    expect(s.ops.find((o) => o.type === 'global')).toBeTruthy()
  })

  it('orders ops crop -> global -> local -> lut -> sharpen regardless of edit order', () => {
    let s = setSharpen(emptyStack(), 20)
    s = addLocal(s, defaultMask('radial'))
    s = setGlobalField(s, 'exposure', 0.2)
    s = setCrop(s, { rect: [0.1, 0.1, 0.8, 0.8] })
    expect(s.ops.map((o) => o.type)).toEqual(['crop', 'global', 'local', 'output_sharpen'])
  })
})

describe('local adjustments', () => {
  it('adds, edits and removes locals by index among locals', () => {
    let s = addLocal(emptyStack(), { kind: 'ai', target: 'sky' })
    s = addLocal(s, defaultMask('linear'))
    expect(localOps(s)).toHaveLength(2)
    s = setLocalField(s, 1, 'exposure', 0.4)
    expect(localOps(s)[1].op.adjust.exposure).toBe(0.4)
    expect(localOps(s)[0].op.adjust.exposure).toBeUndefined()
    s = updateLocal(s, 0, (o) => ({ ...o, invert: true, amount: 0.5 }))
    expect(localOps(s)[0].op).toMatchObject({ invert: true, amount: 0.5 })
    s = removeLocal(s, 0)
    expect(localOps(s)).toHaveLength(1)
    expect(localOps(s)[0].op.mask.kind).toBe('linear')
    expect(removeLocal(s, 7)).toBe(s)
  })

  it('keeps locals independent of the global op', () => {
    let s = setGlobalField(emptyStack(), 'contrast', 10)
    s = addLocal(s, { kind: 'ai', target: 'subject' }, { exposure: 0.2 })
    s = setGlobalField(s, 'contrast', 0)
    expect(s.ops.map((o) => o.type)).toEqual(['local'])
  })
})

describe('crop normalisation', () => {
  it('clamps the rect into the unit square with a minimum size', () => {
    expect(normalizeCropRect([-0.2, 0.1, 0.5, 0.5])).toEqual([0, 0.1, 0.5, 0.5])
    expect(normalizeCropRect([0.8, 0.8, 0.5, 0.5])).toEqual([0.5, 0.5, 0.5, 0.5])
    const [, , w, h] = normalizeCropRect([0.2, 0.2, 0, -3])
    expect(w).toBeGreaterThan(0)
    expect(h).toBeGreaterThan(0)
    expect(normalizeCropRect([NaN, 0, 2, 2])).toEqual([0, 0, 1, 1])
  })

  it('drops an identity crop and clamps the angle to +-45', () => {
    let s = setCrop(emptyStack(), { rect: [0, 0, 1, 1], angle: 0 })
    expect(getCrop(s)).toBeUndefined()
    s = setCrop(emptyStack(), { angle: 90 })
    expect(getCrop(s)?.angle).toBe(45)
    s = setCrop(s, { angle: -120 })
    expect(getCrop(s)?.angle).toBe(-45)
    s = setCrop(s, { angle: 0 })
    expect(getCrop(s)).toBeUndefined()
  })

  it('keeps rect / angle / aspect hint when updating one of them', () => {
    let s = setCrop(emptyStack(), { rect: [0.1, 0.1, 0.6, 0.6], aspect: '1:1' })
    s = setCrop(s, { angle: 2 })
    expect(getCrop(s)).toEqual({ type: 'crop', rect: [0.1, 0.1, 0.6, 0.6], angle: 2, aspect: '1:1' })
    expect(getCrop(setCrop(s, null))).toBeUndefined()
  })

  it('aspect ratios flip for portrait images and free has none', () => {
    expect(aspectRatio('4:5', 1.5)).toBeCloseTo(1.25)
    expect(aspectRatio('4:5', 0.75)).toBeCloseTo(0.8)
    expect(aspectRatio('16:9', 1.5)).toBeCloseTo(16 / 9)
    expect(aspectRatio('free', 1.5)).toBeNull()
    expect(aspectRatio('original', 1.5)).toBe(1.5)
  })

  it('fitAspect yields a centred rect with the requested pixel ratio', () => {
    const imgAspect = 1.5
    const r = fitAspect([0, 0, 1, 1], 1, imgAspect)
    // pixel ratio = (w * imgAspect) / h
    expect((r[2] * imgAspect) / r[3]).toBeCloseTo(1)
    expect(r[0] + r[2] / 2).toBeCloseTo(0.5)
    expect(r[1] + r[3] / 2).toBeCloseTo(0.5)
    expect(r[3]).toBeCloseTo(1)
  })

  it('dragging a corner with a locked ratio keeps the ratio and stays in bounds', () => {
    const imgAspect = 1.5
    const start = fitAspect([0, 0, 1, 1], 1, imgAspect)
    const r = dragCropRect(start, 'se', 0.2, 0.05, 1, imgAspect)
    expect((r[2] * imgAspect) / r[3]).toBeCloseTo(1, 3)
    expect(r[0] + r[2]).toBeLessThanOrEqual(1 + 1e-9)
    expect(r[1] + r[3]).toBeLessThanOrEqual(1 + 1e-9)
    // north-west keeps the south-east corner fixed
    const nw = dragCropRect([0.3, 0.3, 0.4, 0.4], 'nw', 0.05, 0.05, null, imgAspect)
    expect(nw[0] + nw[2]).toBeCloseTo(0.7)
    expect(nw[1] + nw[3]).toBeCloseTo(0.7)
  })

  it('free drag never inverts the rect and move keeps the size', () => {
    const r = dragCropRect([0.4, 0.4, 0.2, 0.2], 'e', -0.9, 0, null, 1.5)
    expect(r[2]).toBeGreaterThan(0)
    const m = dragCropRect([0.4, 0.4, 0.2, 0.2], 'move', 5, -5, null, 1.5)
    expect(m).toEqual([0.8, 0, 0.2, 0.2])
  })

  it('crop tool renders the full frame with only the straighten angle', () => {
    const s = setCrop(setGlobalField(emptyStack(), 'exposure', 0.3), { rect: [0.1, 0.1, 0.5, 0.5], angle: 3 })
    const t = cropToolStack(s)
    expect(getCrop(t)).toMatchObject({ rect: [0, 0, 1, 1], angle: 3 })
    expect(global(t).exposure).toBe(0.3)
    expect(getCrop(cropToolStack(setCrop(s, { angle: 0 })))).toBeUndefined()
  })

  it('before-request keeps geometry only when a crop exists', () => {
    expect(beforeRequest(setGlobalField(emptyStack(), 'exposure', 1))).toEqual({ original: true })
    const c = setCrop(setGlobalField(emptyStack(), 'exposure', 1), { rect: [0, 0, 0.5, 1] })
    expect(beforeRequest(c).stack?.ops).toEqual([getCrop(c)])
  })
})

describe('sections, presets, auto', () => {
  const base = (): EditStack => {
    let s = setGlobalField(emptyStack(), 'exposure', 0.5)
    s = addLocal(s, { kind: 'ai', target: 'sky' })
    s = setCrop(s, { rect: [0, 0, 0.5, 0.5] })
    return setSharpen(s, 30)
  }

  it('extractSections copies only the chosen sections', () => {
    const e = extractSections(base(), ['global', 'output_sharpen'])
    expect(e.ops.map((o) => o.type)).toEqual(['global', 'output_sharpen'])
  })

  it('applySections replaces those sections and leaves the rest alone', () => {
    const target = setGlobalField(setCrop(emptyStack(), { rect: [0.2, 0.2, 0.5, 0.5] }), 'contrast', 40)
    const out = applySections(target, base(), ['global', 'local'])
    expect(global(out).exposure).toBe(0.5)
    expect(global(out).contrast).toBeUndefined()
    expect(getCrop(out)?.rect).toEqual([0.2, 0.2, 0.5, 0.5])
    expect(localOps(out)).toHaveLength(1)
    // a section absent in the source is cleared in the target
    expect(applySections(base(), emptyStack(), ['local']).ops.some((o) => o.type === 'local')).toBe(false)
  })

  it('applyPreset keeps the crop but replaces global / local / lut / sharpen', () => {
    const preset: EditStack = { version: 1, ops: [{ type: 'global', contrast: 20 }, { type: 'lut', file: 'film_warm', amount: 0.5 }] }
    const out = applyPreset(base(), preset)
    expect(out.ops.map((o) => o.type)).toEqual(['crop', 'global', 'lut'])
    expect(global(out).contrast).toBe(20)
    expect(global(out).exposure).toBeUndefined()
  })

  it('mergeAuto replaces provided fields, clamps and marks the source; diff lists changes', () => {
    const cur = setGlobalField(emptyStack(), 'exposure', 0.1)
    const next = mergeAuto(cur, { exposure: 0.65, contrast: 120, shadows: 14, source: 'ai_auto@1' })
    expect(global(next)).toMatchObject({ exposure: 0.65, contrast: 100, shadows: 14, source: 'ai_auto@1' })
    const d = diffAdjust(getAdjust(cur), getAdjust(next))
    expect(d.map((x) => x.key)).toEqual(['exposure', 'contrast', 'shadows'])
    expect(d[0]).toEqual({ key: 'exposure', from: 0.1, to: 0.65 })
  })

  it('stackKey / stacksEqual ignore key order and neutral values', () => {
    const a: EditStack = { version: 1, ops: [{ type: 'global', exposure: 0.5, contrast: 0 }] }
    const b: EditStack = { version: 1, ops: [{ contrast: 0, exposure: 0.5, type: 'global' }] }
    expect(stackKey(a)).toBe(stackKey(b))
    expect(stacksEqual(a, b)).toBe(true)
    expect(stacksEqual(a, emptyStack())).toBe(false)
    expect(stacksEqual({ version: 1, ops: [{ type: 'global', exposure: 0 }] }, emptyStack())).toBe(true)
    expect(normalizeStack(a).ops[0]).toEqual({ type: 'global', exposure: 0.5 })
  })
})
