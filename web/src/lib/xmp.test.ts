import { describe, expect, it } from 'vitest'
import { ratingLabel, sessionFromPath } from './xmp'

const tr = (k: string, o?: Record<string, unknown>) => (o ? `${k}:${JSON.stringify(o)}` : k)

describe('xmp conflict helpers', () => {
  it('labels xmp:Rating values (-1 = rejected)', () => {
    expect(ratingLabel(-1, tr)).toBe('xmp.rejected')
    expect(ratingLabel(0, tr)).toBe('xmp.unrated')
    expect(ratingLabel(null, tr)).toBe('xmp.unrated')
    expect(ratingLabel(undefined, tr)).toBe('xmp.unrated')
    expect(ratingLabel(4, tr)).toBe('xmp.stars:{"n":4}')
  })
  it('finds the library session in the URL', () => {
    expect(sessionFromPath('/s/12/edit/5')).toBe(12)
    expect(sessionFromPath('/s/3')).toBe(3)
    expect(sessionFromPath('/settings')).toBeNull()
  })
})
