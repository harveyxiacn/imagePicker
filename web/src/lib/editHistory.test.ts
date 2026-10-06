import { beforeEach, describe, expect, it } from 'vitest'
import type { EditStack } from '@/api/types'
import { COALESCE_MS, coalesceEntry, useHistory, type HistoryEntry } from './history'

const stackWith = (e: number): EditStack => ({ version: 1, ops: e ? [{ type: 'global', exposure: e }] : [] })
const edit = (label: string, from: number, to: number, over: Partial<HistoryEntry> = {}): HistoryEntry => ({
  label,
  changes: [],
  edits: [{ photoId: 7, before: stackWith(from), after: stackWith(to) }],
  ...over,
})

describe('undo grouping for edits', () => {
  beforeEach(() => useHistory.getState().clear())

  it('merges quick successive commits of the same control into one undo step', () => {
    const h = useHistory.getState()
    h.push(edit('exposure', 0, 0.1, { group: 'adjust:exposure', at: 1000 }))
    h.push(edit('exposure', 0.1, 0.2, { group: 'adjust:exposure', at: 1300 }))
    h.push(edit('exposure', 0.2, 0.3, { group: 'adjust:exposure', at: 1700 }))
    const stack = useHistory.getState().undoStack
    expect(stack).toHaveLength(1)
    // undo goes back to the very first `before`, redo to the latest `after`
    expect(stack[0].edits?.[0].before).toEqual(stackWith(0))
    expect(stack[0].edits?.[0].after).toEqual(stackWith(0.3))
  })

  it('does not merge across groups, long pauses or non-grouped entries', () => {
    const h = useHistory.getState()
    h.push(edit('exposure', 0, 0.1, { group: 'adjust:exposure', at: 1000 }))
    h.push(edit('contrast', 0, 0.1, { group: 'adjust:contrast', at: 1100 }))
    h.push(edit('contrast', 0.1, 0.2, { group: 'adjust:contrast', at: 1100 + COALESCE_MS + 1 }))
    h.push(edit('preset', 0, 0.5, { at: 5000 }))
    h.push(edit('preset', 0.5, 0.6, { at: 5001 }))
    expect(useHistory.getState().undoStack).toHaveLength(5)
  })

  it('does not merge across photos', () => {
    const a = edit('exposure', 0, 0.1, { group: 'adjust:exposure', at: 1000 })
    const b = edit('exposure', 0, 0.1, { group: 'adjust:exposure', at: 1100 })
    b.edits = [{ ...b.edits![0], photoId: 8 }]
    expect(coalesceEntry(a, b)).toBeNull()
  })

  it('never merges multi-photo entries (paste / sync)', () => {
    const multi: HistoryEntry = {
      label: 'paste',
      changes: [],
      group: 'g',
      at: 1,
      edits: [
        { photoId: 1, before: stackWith(0), after: stackWith(1) },
        { photoId: 2, before: stackWith(0), after: stackWith(1) },
      ],
    }
    expect(coalesceEntry(multi, { ...multi, at: 2 })).toBeNull()
    expect(coalesceEntry(undefined, multi)).toBeNull()
  })

  it('undo / redo hand back the edit changes (the caller PUTs the previous / next stack)', () => {
    useHistory.getState().push(edit('exposure', 0, 0.5))
    const u = useHistory.getState().undo()
    expect(u?.edits?.[0].before).toEqual(stackWith(0))
    const r = useHistory.getState().redo()
    expect(r?.edits?.[0].after).toEqual(stackWith(0.5))
  })

  it('a rating entry and an edit entry share one stack in order', () => {
    const h = useHistory.getState()
    h.push({ label: 'rating', changes: [{ id: 1, before: { user_rating: null }, after: { user_rating: 3 } }] })
    h.push(edit('exposure', 0, 0.5))
    expect(useHistory.getState().undo()?.label).toBe('exposure')
    expect(useHistory.getState().undo()?.label).toBe('rating')
  })
})
