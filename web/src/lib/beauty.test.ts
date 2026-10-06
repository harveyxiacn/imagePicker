import { describe, expect, it } from 'vitest'
import type { BeautyProfile, EditStack } from '@/api/types'
import {
  applyProfile,
  getBeauty,
  getLevel,
  getWarpBody,
  getWarpFace,
  hasPortrait,
  isUnnatural,
  portraitPeople,
  profileFromStack,
  removeKind,
  removePerson,
  setBeautyField,
  setBlemish,
  setBodyField,
  setFaceField,
  setLevel,
  setProtectBackground,
} from './beauty'
import { addLocal, defaultMask, emptyStack, isEmptyStack, normalizeStack, setGlobalField, setLut, stacksEqual } from './edit'

const types = (s: EditStack) => s.ops.map((o) => (o.type === 'warp' ? `warp:${(o as { kind: string }).kind}` : o.type))

describe('beauty ops per person', () => {
  it('creates one beauty op per person and updates it in place', () => {
    let s = setBeautyField(emptyStack(), 12, 'smooth', 30)
    s = setBeautyField(s, 12, 'whiten', 20)
    s = setBeautyField(s, 12, 'smooth', 45)
    expect(s.ops).toHaveLength(1)
    expect(getBeauty(s, 12)).toMatchObject({ type: 'beauty', person_id: 12, level: 'standard', smooth: 45, whiten: 20 })
  })

  it('keeps people independent and null means "everyone" (no person_id on the wire)', () => {
    let s = setBeautyField(emptyStack(), 1, 'smooth', 10)
    s = setBeautyField(s, 2, 'smooth', 60)
    s = setBeautyField(s, null, 'smooth', 5)
    expect(s.ops).toHaveLength(3)
    expect(getBeauty(s, 1)?.smooth).toBe(10)
    expect(getBeauty(s, 2)?.smooth).toBe(60)
    const all = getBeauty(s, null)
    expect(all?.smooth).toBe(5)
    expect('person_id' in (all as object)).toBe(false)
    expect(portraitPeople(s)).toEqual([1, 2, null])
  })

  it('enforces one op per kind per person even for hand-edited stacks with duplicates', () => {
    const dup: EditStack = {
      version: 1,
      ops: [
        { type: 'beauty', person_id: 3, level: 'standard', smooth: 10, whiten: 0, blemish: false, eye_brighten: 0, teeth_whiten: 0, dark_circles: 0 },
        { type: 'beauty', person_id: 3, level: 'standard', smooth: 80, whiten: 0, blemish: false, eye_brighten: 0, teeth_whiten: 0, dark_circles: 0 },
      ],
    }
    const s = setBeautyField(dup, 3, 'whiten', 40)
    expect(s.ops.filter((o) => o.type === 'beauty')).toHaveLength(1)
    // the UI edits the FIRST op (contract A)
    expect(getBeauty(s, 3)).toMatchObject({ smooth: 10, whiten: 40 })
  })

  it('face and body warps are separate ops of the same person', () => {
    let s = setFaceField(emptyStack(), 5, 'slim', 25)
    s = setBodyField(s, 5, 'legs', 15)
    s = setFaceField(s, 5, 'eyes', -10)
    expect(types(s).sort()).toEqual(['warp:body', 'warp:face'])
    expect(getWarpFace(s, 5)).toMatchObject({ slim: 25, eyes: -10, chin: 0, nose: 0 })
    expect(getWarpBody(s, 5)).toMatchObject({ legs: 15, protect_background: true })
  })

  it('clamps amounts to the contract ranges (face is symmetric, others 0..100)', () => {
    let s = setFaceField(emptyStack(), 1, 'slim', -250)
    expect(getWarpFace(s, 1)?.slim).toBe(-100)
    s = setBeautyField(s, 1, 'smooth', -5)
    expect(getBeauty(s, 1)).toBeUndefined() // clamped to 0 -> neutral -> dropped
    s = setBodyField(s, 1, 'waist', 400)
    expect(getWarpBody(s, 1)?.waist).toBe(100)
  })

  it('drops an op once every amount is neutral, and the stack becomes empty', () => {
    let s = setBeautyField(emptyStack(), 7, 'smooth', 30)
    expect(isEmptyStack(s)).toBe(false)
    s = setBeautyField(s, 7, 'smooth', 0)
    expect(s.ops).toHaveLength(0)
    expect(isEmptyStack(s)).toBe(true)
    // blemish alone is a real edit; protect_background alone is not
    expect(isEmptyStack(setBlemish(emptyStack(), 7, true))).toBe(false)
    expect(isEmptyStack(setProtectBackground(emptyStack(), 7, false))).toBe(true)
  })

  it('removes one person or one kind without touching the rest', () => {
    let s = setBeautyField(emptyStack(), 1, 'smooth', 30)
    s = setFaceField(s, 1, 'slim', 20)
    s = setBeautyField(s, 2, 'whiten', 30)
    expect(types(removeKind(s, 'face', 1)).sort()).toEqual(['beauty', 'beauty'])
    const r = removePerson(s, 1)
    expect(r.ops).toHaveLength(1)
    expect(getBeauty(r, 2)).toBeDefined()
    expect(hasPortrait(r, 1)).toBe(false)
    expect(hasPortrait(r, 2)).toBe(true)
  })
})

describe('level', () => {
  it('uses the given level for new ops and setLevel rewrites all ops of that person only', () => {
    let s = setBeautyField(emptyStack(), 1, 'smooth', 30, 'natural')
    s = setFaceField(s, 1, 'slim', 20, 'natural')
    s = setBeautyField(s, 2, 'smooth', 30, 'standard')
    expect(getLevel(s, 1)).toBe('natural')
    const r = setLevel(s, 1, 'refined')
    expect(getBeauty(r, 1)?.level).toBe('refined')
    expect(getWarpFace(r, 1)?.level).toBe('refined')
    expect(getBeauty(r, 2)?.level).toBe('standard')
    expect(getLevel(emptyStack(), 9, 'refined')).toBe('refined')
  })

  it('warns about naturalness only for Refined + high values', () => {
    expect(isUnnatural(setBeautyField(emptyStack(), 1, 'smooth', 90, 'refined'), 1)).toBe(true)
    expect(isUnnatural(setFaceField(emptyStack(), 1, 'slim', -80, 'refined'), 1)).toBe(true)
    expect(isUnnatural(setBeautyField(emptyStack(), 1, 'smooth', 90, 'standard'), 1)).toBe(false)
    expect(isUnnatural(setBeautyField(emptyStack(), 1, 'smooth', 40, 'refined'), 1)).toBe(false)
    expect(isUnnatural(setBeautyField(emptyStack(), 1, 'smooth', 90, 'refined'), 2)).toBe(false)
  })
})

describe('render-order normalisation', () => {
  it('orders crop -> warp -> global -> local -> beauty -> lut -> sharpen', () => {
    let s = setBeautyField(emptyStack(), 1, 'smooth', 30)
    s = setLut(s, { file: 'film_warm' })
    s = addLocal(s, defaultMask('radial'), { exposure: 0.5 })
    s = setGlobalField(s, 'contrast', 10)
    s = setFaceField(s, 1, 'slim', 20)
    expect(types(s)).toEqual(['warp:face', 'global', 'local', 'beauty', 'lut'])
  })

  it('portrait edits survive normalisation and compare equal regardless of key order', () => {
    const a = setBeautyField(setFaceField(emptyStack(), 1, 'slim', 10), 1, 'smooth', 10)
    const b = setFaceField(setBeautyField(emptyStack(), 1, 'smooth', 10), 1, 'slim', 10)
    expect(stacksEqual(a, b)).toBe(true)
    expect(normalizeStack(a).ops).toHaveLength(2)
  })
})

describe('profiles', () => {
  const stack = (() => {
    let s = setBeautyField(emptyStack(), 12, 'smooth', 35, 'natural')
    s = setBeautyField(s, 12, 'whiten', 20, 'natural')
    s = setBlemish(s, 12, true, 'natural')
    s = setFaceField(s, 12, 'slim', 25, 'natural')
    s = setBodyField(s, 12, 'legs', 15, 'natural')
    s = setBeautyField(s, 99, 'smooth', 80)
    return s
  })()

  it('extracts the person look without person_id / type / kind', () => {
    const p = profileFromStack(stack, 12)
    expect(p).toEqual({
      beauty: { level: 'natural', smooth: 35, whiten: 20, blemish: true, eye_brighten: 0, teeth_whiten: 0, dark_circles: 0 },
      face: { level: 'natural', slim: 25, chin: 0, eyes: 0, nose: 0 },
      body: { level: 'natural', arms: 0, legs: 15, waist: 0, lengthen_legs: 0, protect_background: true },
    })
    expect(profileFromStack(stack, 5)).toBeNull()
  })

  it('applies a profile to another person: replaces their ops, fills person_id, leaves others alone', () => {
    const profile = profileFromStack(stack, 12) as BeautyProfile
    let target = setBeautyField(emptyStack(), 7, 'smooth', 90) // existing look of person 7 gets replaced
    target = setBeautyField(target, 99, 'whiten', 10)
    const r = applyProfile(target, profile, 7)
    expect(getBeauty(r, 7)).toMatchObject({ type: 'beauty', person_id: 7, smooth: 35, whiten: 20, blemish: true, level: 'natural' })
    expect(getWarpFace(r, 7)).toMatchObject({ person_id: 7, slim: 25 })
    expect(getWarpBody(r, 7)).toMatchObject({ person_id: 7, legs: 15 })
    expect(getBeauty(r, 99)?.whiten).toBe(10)
    expect(r.ops.filter((o) => o.type === 'beauty')).toHaveLength(2)
  })

  it('a profile without a kind clears that kind for the person; round-trips through the stack', () => {
    const onlyFace: BeautyProfile = { face: { level: 'standard', slim: 10, chin: 0, eyes: 0, nose: 0 } }
    let s = setBeautyField(emptyStack(), 4, 'smooth', 50)
    s = applyProfile(s, onlyFace, 4)
    expect(getBeauty(s, 4)).toBeUndefined()
    expect(profileFromStack(s, 4)).toEqual(onlyFace)
  })
})
