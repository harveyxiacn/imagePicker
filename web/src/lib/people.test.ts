import { describe, expect, it } from 'vitest'
import { displayName, unnamedNumbers, withUnnamedNumbers } from './people'

const people = [
  { id: 2, name: null },
  { id: 91, name: 'Anna' },
  { id: 1, name: null },
  { id: 60, name: '  ' },
]

describe('people display names', () => {
  it('numbers unnamed people 1..N in list order, skipping named ones', () => {
    expect([...unnamedNumbers(people)]).toEqual([
      [2, 1],
      [1, 2],
      [60, 3],
    ])
  })

  it('never leaks the cluster id', () => {
    const labelled = withUnnamedNumbers(people)
    expect(labelled.map((p) => displayName(p, '未命名'))).toEqual(['未命名 1', 'Anna', '未命名 2', '未命名 3'])
    expect(JSON.stringify(labelled.map((p) => displayName(p, '未命名')))).not.toMatch(/91|60/)
  })

  it('is stable for the same data and leaves stored fields alone', () => {
    expect(withUnnamedNumbers(people)).toEqual(withUnnamedNumbers(people))
    expect(withUnnamedNumbers(people)[1]).toEqual(people[1])
    expect(withUnnamedNumbers(people)[0]).toMatchObject({ id: 2, name: null, unnamed_no: 1 })
  })

  it('falls back to a bare label when the person is not in the list', () => {
    expect(displayName({ name: null }, 'Unnamed')).toBe('Unnamed')
  })
})
