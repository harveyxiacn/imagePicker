import { QueryClient } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { acceptedRating, canAccept, expressionTone, issueBadges, planAcceptAi, starFills } from './ai'
import { acceptAiRatings, undo } from './actions'
import { qk, type PhotosData } from './cache'
import { makePhoto } from './fixtures'
import { groupPatches, useHistory } from './history'

describe('issueBadges', () => {
  it('maps issues to the doc 04 glyphs in priority order', () => {
    const b = issueBadges(['underexposed', 'closed_eyes', 'blurry', 'overexposed'])
    expect(b.map((x) => x.glyph)).toEqual(['⚠', '🌫', '☀', '🌑'])
    expect(b.map((x) => x.label)).toEqual(['closed_eyes', 'blurry', 'overexposed', 'underexposed'])
  })

  it('handles empty / unanalysed input and drops unknown tags', () => {
    expect(issueBadges([])).toEqual([])
    expect(issueBadges(undefined)).toEqual([])
    expect(issueBadges(['future_tag' as never])).toEqual([])
  })
})

describe('accept AI rating planning', () => {
  it('rounds half stars up and clamps', () => {
    expect(acceptedRating(4.5)).toBe(5)
    expect(acceptedRating(4)).toBe(4)
    expect(acceptedRating(0.5)).toBe(1)
    expect(acceptedRating(2.5)).toBe(3)
    expect(acceptedRating(0)).toBe(0)
    expect(acceptedRating(7)).toBe(5)
  })

  it('only analysed photos with an AI rating can be accepted', () => {
    expect(canAccept(makePhoto(1))).toBe(false)
    expect(canAccept(makePhoto(1, { analyzed: true, ai_rating: null }))).toBe(false)
    expect(canAccept(makePhoto(1, { analyzed: true, ai_rating: 3 }))).toBe(true)
  })

  it('skips unanalysed photos and no-ops, records before values', () => {
    const changes = planAcceptAi([
      makePhoto(1, { analyzed: true, ai_rating: 4.5, user_rating: 2 }),
      makePhoto(2, { analyzed: true, ai_rating: 4, user_rating: 4 }), // already equal
      makePhoto(3, { analyzed: false, ai_rating: null }),
      makePhoto(4, { analyzed: true, ai_rating: 3.5, user_rating: null }),
    ])
    expect(changes).toEqual([
      { id: 1, before: { user_rating: 2 }, after: { user_rating: 5 } },
      { id: 4, before: { user_rating: null }, after: { user_rating: 4 } },
    ])
  })

  it('groups undo into one PATCH per distinct previous rating', () => {
    const changes = planAcceptAi([
      makePhoto(1, { analyzed: true, ai_rating: 4, user_rating: 1 }),
      makePhoto(2, { analyzed: true, ai_rating: 4, user_rating: 1 }),
      makePhoto(3, { analyzed: true, ai_rating: 4, user_rating: null }),
      makePhoto(4, { analyzed: true, ai_rating: 2, user_rating: 1 }),
    ])
    expect(groupPatches(changes, 'after')).toEqual([
      { ids: [1, 2, 3], user_rating: 4 },
      { ids: [4], user_rating: 2 },
    ])
    const undo = groupPatches(changes, 'before')
    expect(undo).toHaveLength(2)
    expect(undo).toContainEqual({ ids: [1, 2, 4], user_rating: 1 })
    expect(undo).toContainEqual({ ids: [3], user_rating: null })
  })
})

describe('acceptAiRatings (cache + history)', () => {
  const calls: { url: string; method: string; body: unknown }[] = []
  beforeEach(() => {
    calls.length = 0
    useHistory.getState().clear()
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string, init?: RequestInit) => {
        calls.push({ url, method: init?.method ?? 'GET', body: init?.body ? JSON.parse(String(init.body)) : undefined })
        return Promise.resolve(new Response(JSON.stringify({ updated: 1 }), { status: 200, headers: { 'Content-Type': 'application/json' } }))
      }),
    )
  })
  afterEach(() => vi.unstubAllGlobals())

  it('is one undo entry; undo restores the previous ratings via PATCH', async () => {
    const qc = new QueryClient()
    const key = qk.photos(1, { session_id: 1 })
    qc.setQueryData<PhotosData>(key, {
      photos: [
        makePhoto(1, { analyzed: true, ai_rating: 4.5, user_rating: 2 }),
        makePhoto(2, { analyzed: true, ai_rating: 3, user_rating: null }),
        makePhoto(3),
      ],
      total: 3,
    })
    const n = await acceptAiRatings(qc, [1, 2, 3], 'Accept AI')
    expect(n).toBe(2)
    expect(qc.getQueryData<PhotosData>(key)!.photos.map((p) => p.user_rating)).toEqual([5, 3, null])
    expect(calls).toEqual([{ url: '/api/photos/accept-ai', method: 'POST', body: { ids: [1, 2] } }])
    expect(useHistory.getState().undoStack).toHaveLength(1)

    calls.length = 0
    await undo(qc)
    expect(qc.getQueryData<PhotosData>(key)!.photos.map((p) => p.user_rating)).toEqual([2, null, null])
    expect(calls.map((c) => c.body).sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)))).toEqual([
      { ids: [1], user_rating: 2 },
      { ids: [2], user_rating: null },
    ])
    expect(useHistory.getState().undoStack).toHaveLength(0)
  })

  it('does nothing (no history entry, no request) when nothing is acceptable', async () => {
    const qc = new QueryClient()
    qc.setQueryData<PhotosData>(qk.photos(1, { session_id: 1 }), { photos: [makePhoto(1)], total: 1 })
    expect(await acceptAiRatings(qc, [1], 'x')).toBe(0)
    expect(calls).toHaveLength(0)
    expect(useHistory.getState().undoStack).toHaveLength(0)
  })
})

describe('presentation helpers', () => {
  it('maps 0.5-step AI ratings to star fills', () => {
    expect(starFills(null)).toEqual(['empty', 'empty', 'empty', 'empty', 'empty'])
    expect(starFills(3.5)).toEqual(['full', 'full', 'full', 'half', 'empty'])
    expect(starFills(5)).toEqual(['full', 'full', 'full', 'full', 'full'])
    expect(starFills(0.5)).toEqual(['half', 'empty', 'empty', 'empty', 'empty'])
  })

  it('colours expression cells green / yellow / red', () => {
    expect(expressionTone(null)).toBe('none')
    expect(expressionTone({ eyes_open: 0.9, smile: 0.8, sharpness: 0.9 })).toBe('good')
    expect(expressionTone({ eyes_open: 0.9, smile: 0.2, sharpness: 0.9 })).toBe('ok')
    expect(expressionTone({ eyes_open: 0.2, smile: 0.9, sharpness: 0.9 })).toBe('bad')
    expect(expressionTone({ eyes_open: 0.9, smile: 0.9, sharpness: 0.1 })).toBe('bad')
    expect(expressionTone({ eyes_open: null, smile: null, sharpness: null })).toBe('ok')
  })
})
