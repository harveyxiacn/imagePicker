import { describe, expect, it } from 'vitest'
import type { EditStack, HairDonor, PatchOp, PhotoPerson } from '@/api/types'
import { bestDonor, defaultHairFace, donorReasonKeys, hairFaceIds, hairLayerOf, removeHair, removeSweat, scorePct, setHairEnabled, setHairStrength, setSweatStrength, sweatLayerOf, timeOffset } from './hair'

const hair = (face: number, amount = 1): PatchOp => ({ type: 'patch', kind: 'hair', asset: `hair_${face}`, rect: [0.1, 0.1, 0.3, 0.4], amount, enabled: true, base_face_id: face, source_photo_id: 7, source_face_id: 70 })
const stack = (): EditStack => ({
  version: 1,
  ops: [
    { type: 'patch', kind: 'best_take', asset: 'bt', rect: [0, 0, 0.2, 0.2], base_face_id: 11, person_id: 3 },
    hair(11, 0.8),
    hair(12),
    { type: 'global', exposure: 0.3 },
  ],
})

const donor = (o: Partial<HairDonor>): HairDonor => ({
  photo_id: 1,
  face_id: 10,
  file_name: 'a.jpg',
  taken_at: null,
  minutes_from_target: null,
  same_session: true,
  composable: true,
  reason: null,
  pose_delta: { yaw: 0, pitch: 0, roll: 0 },
  scale: 1,
  score: 0.8,
  cues: [],
  metrics: null,
  ...o,
})

describe('hair layers', () => {
  it('finds, changes and removes one face’s layer only', () => {
    const s = stack()
    expect(hairFaceIds(s)).toEqual([11, 12])
    expect(hairLayerOf(s, 11)?.amount).toBe(0.8)
    expect(hairLayerOf(s, 13)).toBeUndefined()
    const t = setHairStrength(s, 11, 0.456)
    expect(hairLayerOf(t, 11)?.amount).toBe(0.46)
    expect(hairLayerOf(t, 12)?.amount).toBe(1)
    expect(hairLayerOf(s, 11)?.amount).toBe(0.8) // input untouched
    expect(setHairStrength(s, 11, 4).ops.length).toBe(4)
    expect(hairLayerOf(setHairStrength(s, 11, 4), 11)?.amount).toBe(1)
    expect(hairLayerOf(setHairEnabled(s, 12, false), 12)?.enabled).toBe(false)
    const r = removeHair(s, 11)
    expect(hairFaceIds(r)).toEqual([12])
    expect(r.ops.some((o) => o.type === 'patch' && o.kind === 'best_take')).toBe(true)
    expect(removeHair(s, 99)).toBe(s)
  })
})

describe('hair panel helpers', () => {
  it('starts on the largest subject face', () => {
    const p = (face_id: number, w: number, is_subject: boolean): PhotoPerson => ({ face_id, person_id: face_id, person_name: null, face_box: [0, 0, w, w], is_subject, has_pose: true, has_profile: false })
    expect(defaultHairFace([p(1, 0.3, false), p(2, 0.1, true), p(3, 0.2, true)])).toBe(3)
    expect(defaultHairFace([p(1, 0.1, false), p(2, 0.2, false)])).toBe(2)
    expect(defaultHairFace([])).toBeUndefined()
  })

  it('explains each donor', () => {
    expect(donorReasonKeys(donor({ cues: ['fuller', 'less_shine'] }))).toEqual(['hair.cue_fuller', 'hair.cue_less_shine'])
    expect(donorReasonKeys(donor({ composable: false, reason: 'pose_too_different', cues: ['same_session'] }))).toEqual(['hair.reason_pose_too_different'])
    expect(bestDonor([donor({ photo_id: 1, composable: false }), donor({ photo_id: 2 })])?.photo_id).toBe(2)
    expect(scorePct(0.834)).toBe(83)
    expect(scorePct(null)).toBeNull()
  })

  it('formats the time offset', () => {
    expect(timeOffset(null)).toBeNull()
    expect(timeOffset(-125)).toEqual({ key: 'hair.earlier', h: 2, m: 5 })
    expect(timeOffset(40)).toEqual({ key: 'hair.later', h: 0, m: 40 })
    expect(timeOffset(0)?.key).toBe('hair.sameTime')
  })
})

describe('sweat cleanup layers', () => {
  it('changes and removes the deshine layer only', () => {
    const s: EditStack = {
      version: 1,
      ops: [
        { type: 'patch', kind: 'deshine', asset: 'ds_1', rect: [0, 0, 0.3, 0.3], amount: 0.5 },
        { type: 'patch', kind: 'flyaway', asset: 'fa_1', rect: [0, 0, 0.3, 0.3], amount: 0.7 },
        hair(11),
      ],
    }
    expect(sweatLayerOf(s, 'deshine')?.amount).toBe(0.5)
    const t = setSweatStrength(s, 'deshine', 0.333)
    expect(sweatLayerOf(t, 'deshine')?.amount).toBe(0.33)
    expect(sweatLayerOf(t, 'flyaway')?.amount).toBe(0.7)
    const r = removeSweat(s, 'deshine')
    expect(sweatLayerOf(r, 'deshine')).toBeUndefined()
    expect(sweatLayerOf(r, 'flyaway')).toBeDefined()
    expect(hairLayerOf(r, 11)).toBeDefined()
    expect(removeSweat(r, 'deshine')).toBe(r)
  })
})
