import { describe, expect, it } from 'vitest'
import { buildPhotosQuery, DEFAULT_FILTER, DEFAULT_PERSON_FILTER, type FilterState } from './filter'
import { canonicalQuery, collectionActive, filterToQuery, queryToFilter } from './collections'

const f = (p: Partial<FilterState>): FilterState => ({ ...DEFAULT_FILTER, ...p })

describe('collection query <-> filter', () => {
  it('the default filter is the empty query', () => {
    expect(filterToQuery(DEFAULT_FILTER)).toBe('')
    expect(queryToFilter('')).toEqual(DEFAULT_FILTER)
  })

  it('never stores session / paging params', () => {
    const q = filterToQuery(f({ ratingGte: 3 }))
    expect(q).toBe('rating_gte=3')
    expect(q).not.toMatch(/session_id|cursor|limit/)
  })

  it('parses the four built-in collections', () => {
    expect(queryToFilter('burst_best_only=1')).toEqual(f({ bestOnly: true }))
    expect(queryToFilter('issues_any=closed_eyes')).toEqual(f({ issueMode: 'any', issues: ['closed_eyes'] }))
    expect(queryToFilter('flag=unflagged')).toEqual(f({ flag: 'unflagged' }))
    expect(queryToFilter('has_edits=1')).toEqual(f({ edited: true }))
  })

  it('round-trips a rich filter (filter -> query -> filter -> query)', () => {
    const rich = f({
      ratingGte: 4,
      aiRatingGte: 3.5,
      flag: 'not_rejected',
      color: 'green',
      issueMode: 'any',
      issues: ['blurry', 'noisy'],
      sceneType: 'group',
      bestOnly: true,
      edited: true,
      sort: 'ai',
      person: { ...DEFAULT_PERSON_FILTER, include: [3, 9], exclude: [4], mode: 'any', states: ['eyes_open', 'smiling'], facesMin: 2, facesMax: 5, includeBackground: true },
    })
    const q = filterToQuery(rich)
    const back = queryToFilter(q)
    expect(back).toEqual(rich)
    expect(filterToQuery(back)).toBe(q)
    // and it is exactly the /api/photos query minus session_id
    expect(new URLSearchParams(q).get('persons')).toBe('3,9')
    expect(buildPhotosQuery(1, back)).toEqual(buildPhotosQuery(1, rich))
  })

  it('"any issue" serialises to every issue and parses back to the empty selection', () => {
    const anyIssue = f({ issueMode: 'any', issues: [] })
    const back = queryToFilter(filterToQuery(anyIssue))
    expect(back.issueMode).toBe('any')
    expect(back.issues).toEqual([])
    expect(filterToQuery(back)).toBe(filterToQuery(anyIssue))
  })

  it('ignores unknown params and invalid values', () => {
    const back = queryToFilter('foo=1&flag=bogus&rating_gte=abc&persons=1,x,2&sort=nope')
    expect(back.flag).toBe('all')
    expect(back.ratingGte).toBe(0)
    expect(back.person.include).toEqual([1, 2])
    expect(back.sort).toBe(DEFAULT_FILTER.sort)
  })

  it('persons states are only meaningful with included persons', () => {
    expect(queryToFilter('person_state=smiling').person.states).toEqual([])
    expect(queryToFilter('persons=2&person_state=smiling').person.states).toEqual(['smiling'])
  })

  it('canonicalises parameter order so the active collection is detected', () => {
    expect(canonicalQuery('flag=picked&rating_gte=3')).toBe(canonicalQuery('rating_gte=3&flag=picked'))
    expect(collectionActive('flag=picked&rating_gte=3', f({ ratingGte: 3, flag: 'picked' }))).toBe(true)
    expect(collectionActive('flag=picked', f({ ratingGte: 3, flag: 'picked' }))).toBe(false)
    expect(collectionActive('has_edits=1', DEFAULT_FILTER)).toBe(false)
  })
})
