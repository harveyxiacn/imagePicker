import { describe, expect, it } from 'vitest'
import type { EditStack, PatchKind, PatchOp } from '@/api/types'
import { isEmptyStack, normalizeStack, orderOps, setGlobalField } from './edit'
import {
  addPatch,
  bestTakeOf,
  bestTakePersonIds,
  hasActivePatch,
  mergePatches,
  patchEntries,
  patchesOfKind,
  removeBestTake,
  removePatch,
  replaceByKind,
  setPatchEnabled,
  updateBestTake,
} from './patches'

const p = (kind: PatchKind, asset: string, extra: Partial<PatchOp> = {}): PatchOp => ({ type: 'patch', kind, asset, rect: [0.1, 0.1, 0.2, 0.2], feather: 0.08, amount: 1, enabled: true, ...extra })
const stack = (...ops: EditStack['ops']): EditStack => ({ version: 1, ops })
const assets = (s: EditStack) => patchEntries(s).map((e) => e.op.asset)

describe('patch layer reducers', () => {
  it('enables / disables a layer without touching the others', () => {
    const s = stack(p('inpaint', 'a'), p('denoise', 'b'))
    const off = setPatchEnabled(s, 1, false)
    expect(patchEntries(off).map((e) => e.op.enabled)).toEqual([true, false])
    expect(off).not.toBe(s)
    expect(patchEntries(s)[1].op.enabled).toBe(true) // input untouched
    expect(patchEntries(setPatchEnabled(off, 1, true))[1].op.enabled).toBe(true)
    expect(setPatchEnabled(s, 9, false)).toBe(s)
  })

  it('removes a layer by its patch index', () => {
    const s = stack(p('inpaint', 'a'), { type: 'global', exposure: 0.5 }, p('denoise', 'b'), p('inpaint', 'c'))
    const r = removePatch(s, 1)
    expect(assets(r)).toEqual(['a', 'c'])
    expect(r.ops.some((o) => o.type === 'global')).toBe(true)
    expect(removePatch(s, 7)).toBe(s)
  })

  it('appends inpaint layers (they accumulate)', () => {
    let s = addPatch(stack(), p('inpaint', 'a'))
    s = addPatch(s, p('inpaint', 'b'))
    expect(assets(s)).toEqual(['a', 'b'])
  })

  it('replaces the layer of the same kind (denoise)', () => {
    const s = stack(p('denoise', 'old'), p('inpaint', 'x'))
    const r = replaceByKind(s, 'denoise', [p('denoise', 'new', { amount: 0.4 })])
    expect(assets(r)).toEqual(['new', 'x'])
    expect(patchesOfKind(r, 'denoise')[0].amount).toBe(0.4)
  })

  it('replaces every face-restore layer with the new set (one patch per face)', () => {
    const s = stack(p('face_restore', 'f1'), p('inpaint', 'x'), p('face_restore', 'f2'))
    const r = replaceByKind(s, 'face_restore', [p('face_restore', 'g1'), p('face_restore', 'g2'), p('face_restore', 'g3')])
    expect(assets(r)).toEqual(['g1', 'g2', 'g3', 'x'])
    // nothing to replace: new layers go on top
    expect(assets(replaceByKind(stack(p('inpaint', 'x')), 'face_restore', [p('face_restore', 'g1')]))).toEqual(['x', 'g1'])
  })

  it('replaces a best-take by person only', () => {
    const s = stack(p('best_take', 'a', { person_id: 1, source_photo_id: 5 }), p('best_take', 'b', { person_id: 2, source_photo_id: 6 }))
    const r = replaceByKind(s, 'best_take', [p('best_take', 'c', { person_id: 1, source_photo_id: 7 })], (x) => x.person_id === 1)
    expect(assets(r)).toEqual(['c', 'b'])
    expect(bestTakeOf(r, 1)?.source_photo_id).toBe(7)
    expect(bestTakeOf(r, 2)?.asset).toBe('b')
  })

  it('removes / edits one person best take ("还原此人")', () => {
    const s = stack(p('best_take', 'a', { person_id: 1 }), p('best_take', 'b', { person_id: 2 }), p('inpaint', 'x'))
    expect(assets(removeBestTake(s, 1))).toEqual(['b', 'x'])
    expect(assets(removeBestTake(s, 9))).toEqual(['a', 'b', 'x'])
    const u = updateBestTake(s, 2, (o) => ({ ...o, feather: 0.3 }))
    expect(bestTakeOf(u, 2)?.feather).toBe(0.3)
    expect(bestTakeOf(u, 1)?.feather).toBe(0.08)
    expect(bestTakePersonIds(setPatchEnabled(s, 1, false))).toEqual([1])
  })

  it('keeps patches ahead of the other ops (render order: patch -> crop -> ...)', () => {
    const s = stack({ type: 'crop', rect: [0, 0, 0.5, 0.5], angle: 0 }, { type: 'global', exposure: 1 }, p('inpaint', 'a'))
    expect(orderOps(s.ops).map((o) => o.type)).toEqual(['patch', 'crop', 'global'])
    const added = addPatch(s, p('denoise', 'b'))
    expect(added.ops.map((o) => o.type)).toEqual(['patch', 'patch', 'crop', 'global'])
    // order among patches is the stack order
    expect(assets(added)).toEqual(['a', 'b'])
  })

  it('a disabled patch does not make the stack "edited", an enabled one does', () => {
    expect(isEmptyStack(stack(p('inpaint', 'a', { enabled: false })))).toBe(true)
    expect(isEmptyStack(stack(p('inpaint', 'a')))).toBe(false)
    expect(isEmptyStack(setPatchEnabled(stack(p('inpaint', 'a')), 0, false))).toBe(true)
    expect(hasActivePatch(stack(p('inpaint', 'a'), p('denoise', 'b', { enabled: false })), 'denoise')).toBe(false)
  })

  it('normalising keeps patches verbatim', () => {
    const s = stack(p('inpaint', 'a', { enabled: false }))
    expect(normalizeStack(s).ops).toHaveLength(1)
    // other sliders returning to neutral does not drop the layer
    expect(setGlobalField(s, 'exposure', 0).ops.filter((o) => o.type === 'patch')).toHaveLength(1)
  })

  it('merge: patches from the server, everything else from the live stack', () => {
    const live = stack({ type: 'global', exposure: 0.7 }, p('inpaint', 'old'))
    const server = stack(p('inpaint', 'old'), p('inpaint', 'new'))
    const m = mergePatches(live, server)
    expect(assets(m)).toEqual(['old', 'new'])
    expect(m.ops.find((o) => o.type === 'global')).toMatchObject({ exposure: 0.7 })
  })
})
