import { describe, expect, it } from 'vitest'
import type { TimelineBucket } from '@/api/types'
import { DEFAULT_FILTER } from './filter'
import {
  activeBucketKey,
  barRatio,
  breadcrumbs,
  bucketLabel,
  bucketRange,
  bucketShortLabel,
  keyDate,
  moveFocus,
  ROOT_SCOPE,
  timelineQueryFor,
  toggleBucketRange,
  zoomInScope,
  zoomKey,
  zoomOutScope,
} from './timelineScrub'

const b = (key: string, from: string, to: string, count = 1): TimelineBucket => ({ key, from, to, count, photo_ids: [] })
const YEAR = b('2024', '2024-01-01', '2024-12-31')
const MAY = b('2024-05', '2024-05-01', '2024-05-31')
const DAY = b('2024-05-03', '2024-05-03', '2024-05-03')

describe('bucket <-> date range', () => {
  it('maps a bucket to the grid filter dates', () => {
    expect(bucketRange(YEAR)).toEqual({ takenFrom: '2024-01-01', takenTo: '2024-12-31' })
    expect(bucketRange(DAY)).toEqual({ takenFrom: '2024-05-03', takenTo: '2024-05-03' })
  })

  it('toggles: a click on the active bucket clears the range, another bucket replaces it', () => {
    const none = { takenFrom: null, takenTo: null }
    expect(toggleBucketRange(MAY, none)).toEqual(bucketRange(MAY))
    expect(toggleBucketRange(MAY, bucketRange(MAY))).toEqual(none)
    expect(toggleBucketRange(DAY, bucketRange(MAY))).toEqual(bucketRange(DAY))
  })

  it('finds the active bucket only on an exact match', () => {
    const list = [YEAR, b('2025', '2025-01-01', '2025-12-31')]
    expect(activeBucketKey(list, bucketRange(list[1]))).toBe('2025')
    expect(activeBucketKey(list, { takenFrom: '2024-01-01', takenTo: '2024-06-30' })).toBeNull()
    expect(activeBucketKey(list, { takenFrom: null, takenTo: null })).toBeNull()
  })
})

describe('zoom', () => {
  it('zooms year -> month -> day inside the bucket, and no further', () => {
    const m = zoomInScope(YEAR, 'year')
    expect(m).toEqual({ granularity: 'month', from: '2024-01-01', to: '2024-12-31' })
    expect(zoomInScope(MAY, 'month')).toEqual({ granularity: 'day', from: '2024-05-01', to: '2024-05-31' })
    expect(zoomInScope(DAY, 'day')).toBeNull()
  })

  it('zooms out one level at a time', () => {
    const days = { granularity: 'day' as const, from: '2024-05-01', to: '2024-05-31' }
    const months = zoomOutScope(days)
    expect(months).toEqual({ granularity: 'month', from: '2024-01-01', to: '2024-12-31' })
    expect(zoomOutScope(months)).toEqual(ROOT_SCOPE)
    expect(zoomOutScope(ROOT_SCOPE)).toEqual(ROOT_SCOPE)
  })

  it('builds the breadcrumb of a scope', () => {
    expect(breadcrumbs(ROOT_SCOPE)).toEqual([{ scope: null, granularity: 'all', key: null }])
    const months = zoomInScope(YEAR, 'year')!
    expect(breadcrumbs(months)).toEqual([
      { scope: ROOT_SCOPE, granularity: 'all', key: null },
      { scope: null, granularity: 'year', key: '2024' },
    ])
    const days = zoomInScope(MAY, 'month')!
    const crumbs = breadcrumbs(days)
    expect(crumbs.map((c) => c.key)).toEqual([null, '2024', '2024-05'])
    // going back up through the year crumb lands on that year's months
    expect(crumbs[1].scope).toEqual(months)
    expect(crumbs[2].scope).toBeNull()
  })
})

describe('labels', () => {
  it('formats keys by level, in UTC so the zone cannot shift the day', () => {
    expect(bucketLabel('2024', 'year', 'en')).toBe('2024')
    expect(bucketLabel('2024-05', 'month', 'en')).toBe('May 2024')
    expect(bucketLabel('2024-05-03', 'day', 'en')).toBe('May 3, 2024')
    expect(bucketShortLabel('2024-05', 'month', 'en')).toBe('May')
    expect(bucketShortLabel('2024-05-03', 'day', 'en')).toMatch(/3/)
    expect(bucketLabel('2024-05', 'month', 'zh-CN')).toContain('2024')
    expect(bucketLabel('garbage', 'month', 'en')).toBe('garbage')
  })

  it('reads keys', () => {
    expect(keyDate('2024')).toBe(Date.UTC(2024, 0, 1))
    expect(keyDate('2024-05')).toBe(Date.UTC(2024, 4, 1))
    expect(keyDate('2024-05-03')).toBe(Date.UTC(2024, 4, 3))
    expect(keyDate('24-05')).toBeNull()
  })
})

describe('keyboard and bars', () => {
  it('moves the focus inside the list and stops at the ends', () => {
    expect(moveFocus('ArrowRight', 0, 3)).toBe(1)
    expect(moveFocus('ArrowRight', 2, 3)).toBe(2)
    expect(moveFocus('ArrowLeft', 0, 3)).toBe(0)
    expect(moveFocus('ArrowLeft', 2, 3)).toBe(1)
    expect(moveFocus('Home', 2, 3)).toBe(0)
    expect(moveFocus('End', 0, 3)).toBe(2)
    expect(moveFocus('a', 0, 3)).toBeNull()
    expect(moveFocus('Home', 0, 0)).toBeNull()
  })

  it('maps zoom keys', () => {
    expect(zoomKey('+')).toBe('in')
    expect(zoomKey('ArrowDown')).toBe('in')
    expect(zoomKey('-')).toBe('out')
    expect(zoomKey('Backspace')).toBe('out')
    expect(zoomKey('Enter')).toBeNull()
  })

  it('scales bars between a minimum and the full height', () => {
    expect(barRatio(0, 10)).toBe(0)
    expect(barRatio(10, 10)).toBe(1)
    expect(barRatio(1, 100)).toBe(0.1)
    expect(barRatio(1, 10000)).toBe(0.08)
    expect(barRatio(5, 0)).toBe(0)
  })
})

describe('request', () => {
  it('carries the grid filters but not its date range, event or sort', () => {
    const f = { ...DEFAULT_FILTER, ratingGte: 4, takenFrom: '2024-05-01', takenTo: '2024-05-31', eventId: 7, sort: 'name' as const }
    expect(timelineQueryFor(f, ROOT_SCOPE, null)).toEqual({ rating_gte: 4, granularity: 'year' })
  })

  it('adds the window and the session', () => {
    const scope = zoomInScope(MAY, 'month')!
    expect(timelineQueryFor(DEFAULT_FILTER, scope, 3)).toEqual({ granularity: 'day', from: '2024-05-01', to: '2024-05-31', session_id: 3 })
  })
})
