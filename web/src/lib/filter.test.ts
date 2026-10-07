import { describe, expect, it } from 'vitest'
import { photosQueryString } from '@/api/client'
import {
  applyFacesPreset,
  buildPhotosQuery,
  cyclePerson,
  DEFAULT_FILTER,
  DEFAULT_PERSON_FILTER,
  facesPreset,
  isFilterActive,
  personTri,
  setPersonTri,
  toggleDevice,
  toggleState,
  type FilterState,
} from './filter'

const withPerson = (p: Partial<FilterState['person']>): FilterState => ({ ...DEFAULT_FILTER, person: { ...DEFAULT_PERSON_FILTER, ...p } })

describe('buildPhotosQuery', () => {
  it('omits defaults', () => {
    expect(buildPhotosQuery(7, DEFAULT_FILTER)).toEqual({ session_id: 7 })
  })

  it('maps every M1 filter to contract params', () => {
    const q = buildPhotosQuery(1, { ...DEFAULT_FILTER, ratingGte: 4, flag: 'not_rejected', color: 'red', sort: 'rating' })
    expect(q).toEqual({ session_id: 1, rating_gte: 4, flag: 'not_rejected', color_label: 'red', sort: 'rating' })
  })

  it('keeps a non-default sort even without filters (incl. the new ai sort)', () => {
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, sort: '-taken_at' })).toEqual({ session_id: 1, sort: '-taken_at' })
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, sort: 'ai' })).toEqual({ session_id: 1, sort: 'ai' })
  })

  it('maps AI rating, issues and scene', () => {
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, aiRatingGte: 4.5, sceneType: 'group' })).toEqual({
      session_id: 1,
      ai_rating_gte: 4.5,
      scene_type: 'group',
    })
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, issueMode: 'none' })).toEqual({ session_id: 1, issues_none: true })
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, issueMode: 'any', issues: ['closed_eyes', 'blurry'] })).toEqual({
      session_id: 1,
      issues_any: ['closed_eyes', 'blurry'],
    })
    // "any issue" with no explicit selection lists every tag
    const all = buildPhotosQuery(1, { ...DEFAULT_FILTER, issueMode: 'any' }).issues_any
    expect(all).toContain('closed_eyes')
    expect(all).toHaveLength(6)
  })
})

describe('person filter -> query params', () => {
  it('serialises includes, mode and states only when a person is included', () => {
    const q = buildPhotosQuery(1, withPerson({ include: [3, 5], mode: 'any', states: ['eyes_open', 'smiling'] }))
    expect(q).toMatchObject({ persons: [3, 5], person_mode: 'any', person_state: ['eyes_open', 'smiling'] })
    // AND is the contract default and is omitted
    expect(buildPhotosQuery(1, withPerson({ include: [3] })).person_mode).toBeUndefined()
    // states without any included person are meaningless
    expect(buildPhotosQuery(1, withPerson({ states: ['smiling'] })).person_state).toBeUndefined()
  })

  it('serialises excludes, faces range and include_background', () => {
    const q = buildPhotosQuery(1, withPerson({ include: [1], exclude: [9], facesMin: 2, facesMax: 3, includeBackground: true }))
    expect(q).toMatchObject({ persons: [1], exclude_persons: [9], faces_min: 2, faces_max: 3, include_background: true })
  })

  it('keeps faces_max=0 (no people) and ignores include_background without person conditions', () => {
    const q = buildPhotosQuery(1, withPerson({ facesMax: 0, includeBackground: true }))
    expect(q.faces_max).toBe(0)
    expect(q.faces_min).toBeUndefined()
    expect(q.include_background).toBeUndefined()
  })

  it('builds the URL query string', () => {
    const qs = new URLSearchParams(
      photosQueryString(buildPhotosQuery(2, withPerson({ include: [4, 6], exclude: [8], mode: 'any', states: ['subject'], facesMin: 1, facesMax: 1 }))),
    )
    expect(Object.fromEntries(qs)).toEqual({
      session_id: '2',
      persons: '4,6',
      person_mode: 'any',
      person_state: 'subject',
      exclude_persons: '8',
      faces_min: '1',
      faces_max: '1',
    })
    expect(photosQueryString({ session_id: 1, issues_none: true, burst_best_only: true, burst_id: 12, ai_rating_gte: 4, issues_any: ['blurry'] })).toBe(
      'session_id=1&ai_rating_gte=4&issues_none=1&issues_any=blurry&burst_best_only=1&burst_id=12',
    )
  })

  it('tri-state cycles off -> include -> exclude -> off', () => {
    let p = DEFAULT_PERSON_FILTER
    expect(personTri(p, 3)).toBe('off')
    p = cyclePerson(p, 3)
    expect(personTri(p, 3)).toBe('include')
    expect(p.include).toEqual([3])
    p = cyclePerson(p, 3)
    expect(personTri(p, 3)).toBe('exclude')
    expect(p.include).toEqual([])
    expect(p.exclude).toEqual([3])
    p = cyclePerson(p, 3)
    expect(personTri(p, 3)).toBe('off')
    expect(p.exclude).toEqual([])
  })

  it('setPersonTri moves a person between lists without duplicates', () => {
    let p = setPersonTri(DEFAULT_PERSON_FILTER, 5, 'include')
    p = setPersonTri(p, 5, 'exclude')
    expect(p).toMatchObject({ include: [], exclude: [5] })
    p = setPersonTri(p, 5, 'off')
    expect(p).toMatchObject({ include: [], exclude: [] })
  })

  it('toggles person states', () => {
    let p = toggleState(DEFAULT_PERSON_FILTER, 'eyes_open')
    p = toggleState(p, 'smiling')
    expect(p.states).toEqual(['eyes_open', 'smiling'])
    expect(toggleState(p, 'eyes_open').states).toEqual(['smiling'])
  })

  it('maps face-count presets both ways', () => {
    for (const preset of ['any', 'single', 'few', 'none'] as const) {
      expect(facesPreset(applyFacesPreset(DEFAULT_PERSON_FILTER, preset))).toBe(preset)
    }
    const many = applyFacesPreset(DEFAULT_PERSON_FILTER, 'many', 5)
    expect(many).toMatchObject({ facesMin: 5, facesMax: null })
    expect(facesPreset(many)).toBe('many')
    expect(facesPreset({ ...DEFAULT_PERSON_FILTER, facesMin: 2, facesMax: 9 })).toBe('custom')
    expect(applyFacesPreset(DEFAULT_PERSON_FILTER, 'none')).toMatchObject({ facesMin: null, facesMax: 0 })
  })
})

describe('isFilterActive', () => {
  it('detects active filters (sort alone is not a filter)', () => {
    expect(isFilterActive(DEFAULT_FILTER)).toBe(false)
    expect(isFilterActive({ ...DEFAULT_FILTER, sort: 'name' })).toBe(false)
    expect(isFilterActive({ ...DEFAULT_FILTER, ratingGte: 1 })).toBe(true)
    expect(isFilterActive({ ...DEFAULT_FILTER, color: 'blue' })).toBe(true)
    expect(isFilterActive({ ...DEFAULT_FILTER, aiRatingGte: 3 })).toBe(true)
    expect(isFilterActive({ ...DEFAULT_FILTER, issueMode: 'none' })).toBe(true)
    expect(isFilterActive({ ...DEFAULT_FILTER, sceneType: 'night' })).toBe(true)
    expect(isFilterActive(withPerson({ exclude: [1] }))).toBe(true)
    expect(isFilterActive(withPerson({ facesMax: 0 }))).toBe(true)
    expect(isFilterActive(withPerson({ mode: 'any' }))).toBe(false)
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

describe('device filter', () => {
  it('maps device ids and none to the device query param', () => {
    expect(buildPhotosQuery(1, DEFAULT_FILTER).device).toBeUndefined()
    const q = buildPhotosQuery(1, { ...DEFAULT_FILTER, device: [3, 5] })
    expect(q.device).toEqual([3, 5])
    expect(photosQueryString(q)).toBe('session_id=1&device=3%2C5')
    expect(photosQueryString(buildPhotosQuery(1, { ...DEFAULT_FILTER, device: [2, 'none'] }))).toBe('session_id=1&device=2%2Cnone')
  })

  it('counts as an active filter and toggles', () => {
    expect(isFilterActive({ ...DEFAULT_FILTER, device: ['none'] })).toBe(true)
    expect(toggleDevice([], 3)).toEqual([3])
    expect(toggleDevice([3, 'none'], 3)).toEqual(['none'])
  })
})
