import { describe, expect, it } from 'vitest'
import type { TimelineEvent } from '@/api/types'
import { eventIndexAt, eventLabel, eventStillExists, formatCoord, toggleEvent } from './timeline'

const H = 3_600_000
/** 2024-05-03T00:00:00Z */
const D = Date.UTC(2024, 4, 3)

const ev = (over: Partial<TimelineEvent> = {}): TimelineEvent => ({
  id: 1,
  name: '2024-05-03 11:30-15:10',
  title: null,
  place: null,
  start_at: D + 2 * H + 30 * 60_000,
  end_at: D + 6 * H + 10 * 60_000,
  offset_min: 540,
  gps_lat: null,
  gps_lon: null,
  photo_count: 12,
  scenes: [],
  ...over,
})

describe('formatCoord', () => {
  it('uses hemispheres', () => {
    expect(formatCoord(35.6762, 139.6503)).toBe('35.68°N 139.65°E')
    expect(formatCoord(-33.8688, -70.5)).toBe('33.87°S 70.50°W')
  })
})

describe('eventLabel', () => {
  it('shows the local time span, not the UTC one', () => {
    const l = eventLabel(ev(), 'en')
    expect(l.secondary).toBe('11:30–15:10')
    expect(l.primary).toContain('2024')
    expect(l.primary).toContain('3')
  })

  it('takes the date at the photos offset (UTC date and local date differ)', () => {
    // 20:00 UTC on May 3 is 05:00 on May 4 in Tokyo
    const l = eventLabel(ev({ start_at: D + 20 * H, end_at: D + 20 * H + 5 * 60_000 }), 'en')
    expect(l.primary).toContain('4')
    expect(l.secondary).toBe('05:00–05:05')
    // unknown offset: the stored value is the wall clock already
    expect(eventLabel(ev({ offset_min: null, start_at: D + 20 * H, end_at: D + 21 * H }), 'en').secondary).toBe('20:00–21:00')
  })

  it('adds the place: coordinates, or the place name once set', () => {
    expect(eventLabel(ev({ gps_lat: 35.6762, gps_lon: 139.6503 }), 'en').secondary).toBe('11:30–15:10 · 35.68°N 139.65°E')
    expect(eventLabel(ev({ gps_lat: 35.6762, gps_lon: 139.6503, place: 'Kyoto' }), 'en').secondary).toBe('11:30–15:10 · Kyoto')
  })

  it('prefers the title and shows a single instant once', () => {
    expect(eventLabel(ev({ title: ' Trip ' }), 'zh-CN').primary).toBe('Trip')
    expect(eventLabel(ev({ end_at: ev().start_at }), 'en').secondary).toBe('11:30')
  })

  it('spells out events that cross midnight', () => {
    const l = eventLabel(ev({ start_at: D + 14 * H, end_at: D + 18 * H }), 'en')
    expect(l.secondary).not.toContain('–15')
    expect(l.secondary).toMatch(/23:00 – .*03:00/)
  })
})

describe('event selection', () => {
  it('toggles', () => {
    expect(toggleEvent(null, 4)).toBe(4)
    expect(toggleEvent(4, 4)).toBeNull()
    expect(toggleEvent(3, 4)).toBe(4)
    expect(toggleEvent(undefined, 4)).toBe(4)
  })

  it('notices a vanished event', () => {
    const list = [ev({ id: 1 }), ev({ id: 2 })]
    expect(eventStillExists(list, null)).toBe(true)
    expect(eventStillExists(list, 2)).toBe(true)
    expect(eventStillExists(list, 9)).toBe(false)
  })

  it('finds the event of a capture time', () => {
    const list = [ev({ id: 1, start_at: 100, end_at: 200 }), ev({ id: 2, start_at: 1000, end_at: 2000 })]
    expect(eventIndexAt(list, 150)).toBe(0)
    expect(eventIndexAt(list, 200)).toBe(0)
    expect(eventIndexAt(list, 500)).toBe(-1)
    expect(eventIndexAt(list, 1000)).toBe(1)
    expect(eventIndexAt(list, 5000)).toBe(-1)
    expect(eventIndexAt(list, 50)).toBe(-1)
    expect(eventIndexAt([], 50)).toBe(-1)
  })
})
