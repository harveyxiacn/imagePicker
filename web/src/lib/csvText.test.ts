import { describe, expect, it } from 'vitest'
import { joinCsvText, splitCsvText } from './csvText'

describe('csv text', () => {
  it('joins with commas and escapes commas and backslashes', () => {
    expect(joinCsvText(['a', 'b'])).toBe('a,b')
    expect(joinCsvText(['RF 24-70mm F2.8, L'])).toBe('RF 24-70mm F2.8\\, L')
    expect(joinCsvText(['x\\y'])).toBe('x\\\\y')
    expect(joinCsvText([])).toBe('')
  })

  it('splits what it joined', () => {
    for (const items of [['a'], ['a', 'b'], ['a,b', 'c'], ['x\\y', 'z,'], ['\\,']]) expect(splitCsvText(joinCsvText(items))).toEqual(items)
  })

  it('drops blanks, trims and keeps a trailing backslash', () => {
    expect(splitCsvText(null)).toEqual([])
    expect(splitCsvText('')).toEqual([])
    expect(splitCsvText(' a ,, b,')).toEqual(['a', 'b'])
    expect(splitCsvText('a\\')).toEqual(['a\\'])
  })
})
