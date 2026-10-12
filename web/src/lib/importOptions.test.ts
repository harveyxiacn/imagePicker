import { describe, expect, it } from 'vitest'
import { parseExcludes } from './importOptions'

describe('parseExcludes', () => {
  it('splits lines, trims, drops blanks and duplicates', () => {
    expect(parseExcludes('backup/**\n  *.dng \r\n\nbackup/**\nrejects')).toEqual(['backup/**', '*.dng', 'rejects'])
  })
  it('is empty for blank input', () => {
    expect(parseExcludes('  \n\n')).toEqual([])
  })
})
