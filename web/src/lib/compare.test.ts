import { describe, expect, it } from 'vitest'
import type { Photo } from '@/api/types'
import { compareColumns, compareSlots, compareUsesSelection } from './compare'

const photos = Array.from({ length: 10 }, (_, i) => ({ id: i + 1 }) as Photo)
const ids = (ps: Photo[]) => ps.map((p) => p.id)

describe('compareColumns', () => {
  it('lays out 1/2/3/4 photos as 1, 2, 3 and 2 columns', () => {
    expect([1, 2, 3, 4].map(compareColumns)).toEqual([1, 2, 3, 2])
  })
})

describe('compareSlots without a selection', () => {
  it('pins A, then the cursor, then the following candidates', () => {
    expect(ids(compareSlots(photos, 2, 5, 2))).toEqual([2, 5])
    expect(ids(compareSlots(photos, 2, 5, 3))).toEqual([2, 5, 6])
    expect(ids(compareSlots(photos, 2, 5, 4))).toEqual([2, 5, 6, 7])
  })
  it('skips A among the candidates and stops at the end of the list', () => {
    expect(ids(compareSlots(photos, 9, 8, 4))).toEqual([9, 8, 10])
  })
  it('treats a single selected photo as no selection', () => {
    expect(ids(compareSlots(photos, 2, 5, 3, new Set([7])))).toEqual([2, 5, 6])
  })
})

describe('compareSlots from the selection', () => {
  const sel = new Set([8, 2, 5, 9, 4])
  it('takes the selected photos in list order', () => {
    expect(ids(compareSlots(photos, null, 5, 3, new Set([8, 2, 5])))).toEqual([2, 5, 8])
  })
  it('pages through a larger selection with the cursor', () => {
    expect(ids(compareSlots(photos, null, 2, 2, sel))).toEqual([2, 4])
    expect(ids(compareSlots(photos, null, 5, 2, sel))).toEqual([5, 8])
    expect(ids(compareSlots(photos, null, 9, 2, sel))).toEqual([8, 9])
    expect(ids(compareSlots(photos, null, 9, 4, sel))).toEqual([4, 5, 8, 9])
  })
  it('shows the first page when the cursor is outside the selection', () => {
    expect(ids(compareSlots(photos, 1, 10, 3, sel))).toEqual([2, 4, 5])
  })
  it('ignores the pinned A while the selection is shown', () => {
    expect(ids(compareSlots(photos, 1, 2, 2, new Set([2, 3])))).toEqual([2, 3])
  })
  it('reports whether the selection drives compare', () => {
    expect(compareUsesSelection(photos, new Set([1]))).toBe(false)
    expect(compareUsesSelection(photos, new Set([1, 3]))).toBe(true)
    expect(compareUsesSelection(photos, new Set([1, 99]))).toBe(false)
  })
})
