import { describe, expect, it } from 'vitest'
import { generatePhotos } from './db'
import { deviceList, filterOptions, filterPhotos, mockSharp } from './query'

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

describe('mock lens / date / sharpness filters', () => {
  it('filters by lens and lists the lenses', () => {
    const o = filterOptions(photos)
    expect(o.lenses.length).toBeGreaterThan(1)
    expect(o.lenses[0].photo_count).toBeGreaterThanOrEqual(o.lenses[1].photo_count)
    expect(o.lenses.reduce((n, l) => n + l.photo_count, 0)).toBe(photos.filter((p) => p.lens).length)
    const first = filterPhotos(photos, new URLSearchParams({ lens: o.lenses[0].lens }))
    expect(first).toHaveLength(o.lenses[0].photo_count)
  })

  it('filters by an inclusive date range', () => {
    const o = filterOptions(photos)
    const day = (ms: number) => new Date(ms).toISOString().slice(0, 10)
    const all = filterPhotos(photos, new URLSearchParams({ taken_from: day(o.taken_min!), taken_to: day(o.taken_max!) }))
    expect(all).toHaveLength(photos.filter((p) => p.taken_at !== null).length)
    const none = filterPhotos(photos, new URLSearchParams({ taken_from: '2999-01-01' }))
    expect(none).toHaveLength(0)
  })

  it('splits photos into sharp / blurry / unknown', () => {
    const n = (s: string) => filterPhotos(photos, new URLSearchParams({ sharp: s })).length
    expect(n('sharp') + n('blurry') + n('unknown')).toBe(photos.length)
    expect(photos.every((p) => ['sharp', 'blurry', 'unknown'].includes(mockSharp(p)))).toBe(true)
  })
})
