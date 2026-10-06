import { describe, expect, it } from 'vitest'
import { formatDate } from './format'

describe('formatDate', () => {
  // EXIF: 2025:05:13 15:50:43 with OffsetTimeOriginal -03:00 -> stored as 18:50:43 UTC.
  const takenAt = Date.UTC(2025, 4, 13, 18, 50, 43)

  it('shows the wall clock where the photo was taken, not the viewer time zone', () => {
    const s = formatDate(takenAt, 'en-GB', -180)
    expect(s).toContain('13/05/2025')
    expect(s).toContain('15:50:43')
    expect(s).toContain('UTC−03:00')
  })

  it('renders naive (offset-less) times as recorded', () => {
    const naive = Date.UTC(2025, 4, 13, 15, 50, 43)
    const s = formatDate(naive, 'en-GB', null)
    expect(s).toContain('15:50:43')
    expect(s).not.toContain('UTC')
  })

  it('handles missing time', () => {
    expect(formatDate(null)).toBe('—')
  })
})
