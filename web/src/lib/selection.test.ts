import { describe, expect, it } from 'vitest'
import { clickSelect, EMPTY_SELECTION, pruneSelection, selectAll } from './selection'

const ids = [10, 11, 12, 13, 14, 15]
const set = (s: { ids: ReadonlySet<number> }) => [...s.ids].sort((a, b) => a - b)

describe('selection', () => {
  it('plain click selects a single item and sets the anchor', () => {
    const s = clickSelect(EMPTY_SELECTION, ids, 12)
    expect(set(s)).toEqual([12])
    expect(s.anchorId).toBe(12)
    expect(set(clickSelect(s, ids, 14))).toEqual([14])
  })

  it('ctrl toggles items and moves the anchor', () => {
    let s = clickSelect(EMPTY_SELECTION, ids, 10)
    s = clickSelect(s, ids, 12, { ctrl: true })
    expect(set(s)).toEqual([10, 12])
    s = clickSelect(s, ids, 10, { ctrl: true })
    expect(set(s)).toEqual([12])
    expect(s.anchorId).toBe(10)
  })

  it('shift selects the range from the anchor in either direction', () => {
    let s = clickSelect(EMPTY_SELECTION, ids, 12)
    s = clickSelect(s, ids, 14, { shift: true })
    expect(set(s)).toEqual([12, 13, 14])
    expect(s.anchorId).toBe(12)
    s = clickSelect(s, ids, 10, { shift: true })
    expect(set(s)).toEqual([10, 11, 12])
  })

  it('ctrl+shift extends the existing selection with the range', () => {
    let s = clickSelect(EMPTY_SELECTION, ids, 10)
    s = clickSelect(s, ids, 14, { ctrl: true })
    s = clickSelect(s, ids, 15, { shift: true, ctrl: true })
    expect(set(s)).toEqual([10, 14, 15])
  })

  it('shift without an anchor behaves like a plain click', () => {
    expect(set(clickSelect(EMPTY_SELECTION, ids, 13, { shift: true }))).toEqual([13])
  })

  it('selectAll selects everything (Ctrl+A)', () => {
    expect(set(selectAll(ids))).toEqual(ids)
  })

  it('pruneSelection drops ids that left the list and keeps identity when unchanged', () => {
    const s = selectAll(ids)
    expect(pruneSelection(s, ids)).toBe(s)
    const pruned = pruneSelection(s, [10, 11])
    expect(set(pruned)).toEqual([10, 11])
  })
})
