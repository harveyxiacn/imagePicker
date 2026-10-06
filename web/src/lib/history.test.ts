import { beforeEach, describe, expect, it } from 'vitest'
import { computeChanges, groupPatches, useHistory } from './history'

const photo = (id: number, user_rating: number | null = null, flag: -1 | 0 | 1 = 0) => ({
  id,
  user_rating,
  flag,
  color_label: null as null,
})

describe('computeChanges', () => {
  it('records before/after and skips no-ops', () => {
    const changes = computeChanges([photo(1, 2), photo(2, 4), photo(3, null)], { user_rating: 4 })
    expect(changes).toEqual([
      { id: 1, before: { user_rating: 2 }, after: { user_rating: 4 } },
      { id: 3, before: { user_rating: null }, after: { user_rating: 4 } },
    ])
  })

  it('handles clearing a rating to null and multiple fields', () => {
    const [c] = computeChanges([photo(1, 3, 1)], { user_rating: null, flag: -1 })
    expect(c.before).toEqual({ user_rating: 3, flag: 1 })
    expect(c.after).toEqual({ user_rating: null, flag: -1 })
  })
})

describe('groupPatches', () => {
  it('groups photos with identical patches into one PATCH body', () => {
    const changes = computeChanges([photo(1, 1), photo(2, 1), photo(3, 5)], { user_rating: 4 })
    expect(groupPatches(changes, 'after')).toEqual([{ ids: [1, 2, 3], user_rating: 4 }])
    const undo = groupPatches(changes, 'before')
    expect(undo).toHaveLength(2)
    expect(undo).toContainEqual({ ids: [1, 2], user_rating: 1 })
    expect(undo).toContainEqual({ ids: [3], user_rating: 5 })
  })
})

describe('history store', () => {
  beforeEach(() => useHistory.getState().clear())

  const entry = (label: string, id = 1) => ({
    label,
    changes: computeChanges([photo(id, null)], { user_rating: 3 }),
  })

  it('undo moves entries to redo and back', () => {
    const h = useHistory.getState()
    h.push(entry('a'))
    h.push(entry('b'))
    expect(useHistory.getState().undo()?.label).toBe('b')
    expect(useHistory.getState().undo()?.label).toBe('a')
    expect(useHistory.getState().undo()).toBeUndefined()
    expect(useHistory.getState().redoStack).toHaveLength(2)
    expect(useHistory.getState().redo()?.label).toBe('a')
    expect(useHistory.getState().redo()?.label).toBe('b')
    expect(useHistory.getState().redo()).toBeUndefined()
    expect(useHistory.getState().undoStack).toHaveLength(2)
  })

  it('a new action clears the redo stack', () => {
    const h = useHistory.getState()
    h.push(entry('a'))
    useHistory.getState().undo()
    expect(useHistory.getState().redoStack).toHaveLength(1)
    useHistory.getState().push(entry('b'))
    expect(useHistory.getState().redoStack).toHaveLength(0)
  })

  it('caps history length', () => {
    for (let i = 0; i < 250; i++) useHistory.getState().push(entry(`e${i}`))
    expect(useHistory.getState().undoStack).toHaveLength(200)
    expect(useHistory.getState().undoStack.at(-1)?.label).toBe('e249')
  })
})
