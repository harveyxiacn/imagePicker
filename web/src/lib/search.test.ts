import { afterEach, describe, expect, it, vi } from 'vitest'
import { fetchSearchPhotos, photosQueryString, searchQueryString } from '@/api/client'
import { buildPhotosQuery, DEFAULT_FILTER, isFilterActive } from './filter'
import { makePhoto } from './fixtures'
import { MAX_QUERY_CHARS, normalizeSearch, orderByHits, scorePercent, TEXT_MODEL, textModelState } from './search'

describe('normalizeSearch', () => {
  it('trims and collapses whitespace, in any script', () => {
    expect(normalizeSearch('  sunset \t at\nthe beach ')).toBe('sunset at the beach')
    expect(normalizeSearch(' 海边　日落 ')).toBe('海边 日落')
    expect(normalizeSearch('   ')).toBe('')
  })
  it('caps at the server limit by characters, not UTF-16 units', () => {
    const long = '猫'.repeat(MAX_QUERY_CHARS + 40)
    expect(Array.from(normalizeSearch(long))).toHaveLength(MAX_QUERY_CHARS)
    const emoji = '🌅'.repeat(MAX_QUERY_CHARS + 1)
    expect(Array.from(normalizeSearch(emoji))).toHaveLength(MAX_QUERY_CHARS)
  })
})

describe('search in the photo filter', () => {
  it('is off by default and does not change the default query', () => {
    expect(buildPhotosQuery(1, DEFAULT_FILTER)).toEqual({ session_id: 1 })
    expect(isFilterActive(DEFAULT_FILTER)).toBe(false)
  })
  it('adds the normalised text to the query key and makes the filter active', () => {
    const f = { ...DEFAULT_FILTER, search: '  海边   日落 ', ratingGte: 3 }
    expect(buildPhotosQuery(2, f)).toEqual({ session_id: 2, rating_gte: 3, search: '海边 日落' })
    expect(isFilterActive(f)).toBe(true)
    expect(isFilterActive({ ...DEFAULT_FILTER, search: '   ' })).toBe(false)
  })
  it('is never sent to GET /api/photos, but ids are', () => {
    const q = { ...buildPhotosQuery(2, { ...DEFAULT_FILTER, search: 'dog' }), ids: [5, 3] }
    const qs = new URLSearchParams(photosQueryString(q))
    expect(qs.get('search')).toBeNull()
    expect(qs.get('ids')).toBe('5,3')
    expect(new URLSearchParams(photosQueryString({ session_id: 1, ids: [] })).get('ids')).toBe('')
    expect(new URLSearchParams(photosQueryString({ session_id: 1 })).has('ids')).toBe(false)
  })
})

describe('searchQueryString', () => {
  it('carries the filters and the text, but not sort, paging or ids', () => {
    const q = {
      ...buildPhotosQuery(7, { ...DEFAULT_FILTER, flag: 'picked', sort: 'rating', search: 'sunset' }),
      cursor: 'abc',
      limit: 50,
      ids: [1],
    }
    const qs = new URLSearchParams(searchQueryString(q, 'sunset at the beach', 300))
    expect(qs.get('q')).toBe('sunset at the beach')
    expect(qs.get('session_id')).toBe('7')
    expect(qs.get('flag')).toBe('picked')
    expect(qs.get('limit')).toBe('300')
    for (const k of ['sort', 'cursor', 'ids', 'search']) expect(qs.has(k)).toBe(false)
  })
  it('encodes Chinese text', () => {
    const s = searchQueryString({ session_id: 1 }, '海边日落', 10)
    expect(new URLSearchParams(s).get('q')).toBe('海边日落')
    expect(s).toContain('%E6%B5%B7')
  })
})

describe('orderByHits', () => {
  const photos = [makePhoto(1), makePhoto(2), makePhoto(3), makePhoto(4)]
  it('follows the ranking and drops photos that are not hits', () => {
    const hits = [{ photo_id: 3 }, { photo_id: 1 }, { photo_id: 99 }]
    expect(orderByHits(photos, hits).map((p) => p.id)).toEqual([3, 1])
  })
  it('is empty without hits', () => {
    expect(orderByHits(photos, [])).toEqual([])
  })
})

describe('textModelState', () => {
  it('is unknown until the model list is known', () => {
    expect(textModelState(undefined)).toBe('unknown')
    expect(textModelState([{ id: 'yunet', installed: true }])).toBe('unknown')
  })
  it('tells installed from missing', () => {
    expect(textModelState([{ id: TEXT_MODEL, installed: true }])).toBe('ready')
    expect(textModelState([{ id: TEXT_MODEL, installed: false }])).toBe('missing')
  })
})

it('formats a score as a percentage', () => {
  expect(scorePercent(0.987)).toBe(99)
})

describe('fetchSearchPhotos', () => {
  afterEach(() => vi.unstubAllGlobals())
  const json = (body: unknown, status = 200) =>
    Promise.resolve(new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } }))

  it('ranks through /api/search, then loads the hits by id in rank order', async () => {
    const urls: string[] = []
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) => {
        urls.push(url)
        if (url.startsWith('/api/search')) {
          return json({
            query: 'dog',
            model: 'm',
            min_score: 0.05,
            hits: [
              { photo_id: 3, score: 0.99, similarity: 0.3 },
              { photo_id: 1, score: 0.5, similarity: 0.2 },
            ],
            total_matched: 2,
            searched: 4,
            unindexed: 1,
          })
        }
        return json({ photos: [makePhoto(1), makePhoto(3)], total: 2, next_cursor: null })
      }),
    )
    const r = await fetchSearchPhotos({ session_id: 9, flag: 'picked', sort: 'name', search: 'dog' })
    expect(r.photos.map((p) => p.id)).toEqual([3, 1])
    expect(r.total).toBe(2)
    expect(r.search).toEqual({ total_matched: 2, searched: 4, unindexed: 1 })
    expect(urls).toHaveLength(2)
    expect(urls[0]).toContain('/api/search?')
    expect(urls[0]).toContain('q=dog')
    expect(urls[0]).toContain('flag=picked')
    expect(urls[0]).not.toContain('sort=')
    expect(urls[1]).toContain('/api/photos?')
    expect(urls[1]).toContain('ids=3%2C1')
  })

  it('skips the photo request when nothing matches', async () => {
    const f = vi.fn(() =>
      json({ query: 'x', model: 'm', min_score: 0.05, hits: [], total_matched: 0, searched: 0, unindexed: 7 }),
    )
    vi.stubGlobal('fetch', f)
    const r = await fetchSearchPhotos({ session_id: 1, search: 'x' })
    expect(r).toEqual({ photos: [], total: 0, search: { total_matched: 0, searched: 0, unindexed: 7 } })
    expect(f).toHaveBeenCalledTimes(1)
  })

  it('surfaces models_missing as an ApiError with the model ids', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => json({ error: { code: 'models_missing', message: 'required models are not installed: siglip2-base-text' }, models: [TEXT_MODEL] }, 409)),
    )
    await expect(fetchSearchPhotos({ session_id: 1, search: 'x' })).rejects.toMatchObject({ status: 409, code: 'models_missing' })
  })
})
