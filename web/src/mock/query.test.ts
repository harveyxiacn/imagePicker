import { describe, expect, it } from 'vitest'
import { generatePhotos } from './db'
import { deviceList, filterPhotos } from './query'

const photos = generatePhotos(1, '/p', 300, 1, 1_700_000_000_000)

describe('mock device filter', () => {
  it('lists devices sorted by count and filters by id / none', () => {
    const devs = deviceList(photos)
    expect(devs.length).toBeGreaterThanOrEqual(2)
    expect(devs[0].photo_count).toBeGreaterThanOrEqual(devs[1].photo_count)
    const first = filterPhotos(photos, new URLSearchParams(`device=${devs[0].id}`))
    expect(first).toHaveLength(devs[0].photo_count)
    const none = filterPhotos(photos, new URLSearchParams('device=none'))
    const total = devs.reduce((n, d) => n + d.photo_count, 0)
    expect(none.length).toBe(photos.length - total)
    const both = filterPhotos(photos, new URLSearchParams(`device=${devs[0].id},none`))
    expect(both.length).toBe(first.length + none.length)
  })
})
