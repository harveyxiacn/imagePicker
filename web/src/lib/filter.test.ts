import { describe, expect, it } from 'vitest'
import { photosQueryString } from '@/api/client'
import { buildPhotosQuery, DEFAULT_FILTER, isFilterActive } from './filter'

describe('buildPhotosQuery', () => {
  it('omits defaults', () => {
    expect(buildPhotosQuery(7, DEFAULT_FILTER)).toEqual({ session_id: 7 })
  })

  it('maps every filter to contract params', () => {
    const q = buildPhotosQuery(1, { ratingGte: 4, flag: 'not_rejected', color: 'red', sort: 'rating' })
    expect(q).toEqual({ session_id: 1, rating_gte: 4, flag: 'not_rejected', color_label: 'red', sort: 'rating' })
  })

  it('keeps a non-default sort even without filters', () => {
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, sort: '-taken_at' })).toEqual({ session_id: 1, sort: '-taken_at' })
  })

  it('detects active filters (sort alone is not a filter)', () => {
    expect(isFilterActive(DEFAULT_FILTER)).toBe(false)
    expect(isFilterActive({ ...DEFAULT_FILTER, sort: 'name' })).toBe(false)
    expect(isFilterActive({ ...DEFAULT_FILTER, ratingGte: 1 })).toBe(true)
    expect(isFilterActive({ ...DEFAULT_FILTER, color: 'blue' })).toBe(true)
  })
})

describe('photosQueryString', () => {
  it('serializes session, filters, cursor and limit', () => {
    const qs = new URLSearchParams(
      photosQueryString({ session_id: 3, rating_gte: 2, flag: 'picked', sort: 'name', cursor: '5000', limit: 5000 }),
    )
    expect(Object.fromEntries(qs)).toEqual({
      session_id: '3',
      rating_gte: '2',
      flag: 'picked',
      sort: 'name',
      cursor: '5000',
      limit: '5000',
    })
  })

  it('sends rating_gte=0 only when explicitly set', () => {
    expect(photosQueryString({ session_id: 1 })).toBe('session_id=1')
  })
})
