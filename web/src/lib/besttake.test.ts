import { describe, expect, it } from 'vitest'
import type { BestTakeCandidate, BestTakePlan, EditStack, PatchOp } from '@/api/types'
import { alreadyBest, autoChoices, baseFaceIdFor, buildPlanView, choiceFor, scoreOf, sortedWarnings, visibleCandidates } from './besttake'

const cand = (photo_id: number, score: number, composable = true, reason: string | null = null): BestTakeCandidate => ({
  photo_id,
  face_id: photo_id * 10 + 1,
  expression_score: score,
  composable,
  reason: composable ? null : (reason ?? 'face_occluded'),
})

// Base = photo 3. Alice is best in #5 (composable); Bob's best frame (#2) is not composable, next best #4; Carol is already best in the base.
const plan: BestTakePlan = {
  base_photo_id: 3,
  people: [
    { track_id: 1, person_id: 11, person_name: 'Alice', base_face_id: 31, best_photo_id: 5, candidates: [cand(5, 0.92), cand(1, 0.8), cand(3, 0.4), cand(2, 0.3)] },
    { track_id: 2, person_id: 12, person_name: 'Bob', base_face_id: 32, best_photo_id: 4, candidates: [cand(2, 0.95, false), cand(4, 0.85), cand(3, 0.5), cand(1, 0.2)] },
    { track_id: 3, person_id: null, person_name: null, base_face_id: 33, best_photo_id: 3, candidates: [cand(3, 0.9), cand(1, 0.6), cand(2, 0.5, false, 'face_not_aligned')] },
  ],
}
const order = [1, 2, 3, 4, 5]

const patch = (person_id: number, source_photo_id: number, enabled = true): PatchOp => ({ type: 'patch', kind: 'best_take', asset: `a${person_id}`, rect: [0, 0, 0.1, 0.1], person_id, source_photo_id, enabled })
const stackOf = (...ops: PatchOp[]): EditStack => ({ version: 1, ops })

describe('besttake plan view-model', () => {
  const view = buildPlanView(plan, { order })

  it('sorts candidates by expression score and numbers frames in shot order', () => {
    expect(view[0].candidates.map((c) => c.photo_id)).toEqual([5, 1, 3, 2])
    expect(view[0].candidates.map((c) => c.frame)).toEqual([5, 1, 3, 2])
    expect(scoreOf({ expression_score: 0.926 })).toBe(93)
    expect(view[0].candidates[0].score).toBe(92)
  })

  it('selects the best composable candidate, skipping non-composable ones', () => {
    expect(view[0].best?.photo_id).toBe(5)
    // Bob's top-scoring frame cannot be composited: the best is the next one
    expect(view[1].candidates[0]).toMatchObject({ photo_id: 2, composable: false })
    expect(view[1].best?.photo_id).toBe(4)
    expect(view[1].bestSwap?.photo_id).toBe(4)
  })

  it('has no swap for a person already at their best in the base', () => {
    expect(view[2].best?.photo_id).toBe(3)
    expect(view[2].best?.isBase).toBe(true)
    expect(view[2].bestSwap).toBeUndefined()
    expect(alreadyBest(view).map((p) => p.track_id)).toEqual([3])
  })

  it('auto choices paste only the improvable people, with the right face ids', () => {
    expect(autoChoices(view)).toEqual([
      { base_face_id: 31, source_photo_id: 5, source_face_id: 51 },
      { base_face_id: 32, source_photo_id: 4, source_face_id: 41 },
    ])
    expect(choiceFor(view[0], view[0].candidates[1])).toEqual({ base_face_id: 31, source_photo_id: 1, source_face_id: 11 })
  })

  it('never offers a non-composable candidate as a swap even when it scores highest', () => {
    expect(view[1].candidates.find((c) => c.photo_id === 2)).toMatchObject({ composable: false, isBest: false })
    expect(autoChoices(view).some((c) => c.source_photo_id === 2)).toBe(false)
  })

  it('marks the base candidate and the face currently pasted', () => {
    const v = buildPlanView(plan, { order, stack: stackOf(patch(11, 1)) })
    expect(v[0].candidates.find((c) => c.isBase)?.photo_id).toBe(3)
    expect(v[0].replaced).toBe(true)
    expect(v[0].current?.photo_id).toBe(1)
    expect(v[1].replaced).toBe(false)
  })

  it('treats a disabled patch layer as not replaced', () => {
    const v = buildPlanView(plan, { order, stack: stackOf(patch(11, 1, false)) })
    expect(v[0].replaced).toBe(false)
  })

  it('has no swap when the base is already better than every other composable frame', () => {
    const v = buildPlanView(plan, { order, baseId: 5 })
    // Alice: the base (#5) is her best, nothing to paste
    expect(v[0].bestSwap).toBeUndefined()
    // Bob: #4 beats #5's 0.0 (missing candidate -> no base entry -> any composable best counts)
    expect(v[1].bestSwap?.photo_id).toBe(4)
  })

  it('shows the top candidates plus the applied one, or everything on request', () => {
    const v = buildPlanView(plan, { order, stack: stackOf(patch(11, 2)) })
    expect(visibleCandidates(v[0], false, 2).map((c) => c.photo_id)).toEqual([5, 1, 2])
    expect(visibleCandidates(v[0], false).map((c) => c.photo_id)).toEqual([5, 1, 3, 2])
    expect(visibleCandidates(v[0], true, 1)).toHaveLength(4)
    expect(visibleCandidates(view[0], false, 2).map((c) => c.photo_id)).toEqual([5, 1])
  })

  it('resolves the person face id in another base photo', () => {
    const p = plan.people[0]
    expect(baseFaceIdFor(p, plan, 3)).toBe(31)
    expect(baseFaceIdFor(p, plan, 5)).toBe(51) // from the candidate list
    expect(baseFaceIdFor({ ...p, candidates: [] }, plan, 5, (track, photo) => track * 1000 + photo)).toBe(1005)
    expect(buildPlanView(plan, { order, baseId: 1 })[0].baseFaceId).toBe(11)
  })

  it('orders warnings for display and drops duplicates', () => {
    expect(sortedWarnings(['seam', 'large_pose_change', 'seam', 'camera_moved'])).toEqual(['large_pose_change', 'camera_moved', 'seam'])
    expect(sortedWarnings(undefined)).toEqual([])
  })
})
