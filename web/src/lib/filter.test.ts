import { describe, expect, it } from 'vitest'
import { photosQueryString } from '@/api/client'
import {
  applyFacesPreset,
  buildPhotosQuery,
  clampFaceSimilarity,
  cyclePerson,
  DEFAULT_FACE_SIMILARITY,
  DEFAULT_FILTER,
  DEFAULT_PERSON_FILTER,
  facesPreset,
  isFilterActive,
  isIsoDate,
  msToDate,
  personTri,
  setDateRange,
  setPersonTri,
  toggleDevice,
  toggleState,
  toggleValue,
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

describe('lens / date / keyword / sharpness / similar-faces filters', () => {
  it('sends nothing by default', () => {
    const q = buildPhotosQuery(1, DEFAULT_FILTER)
    for (const k of ['lens', 'taken_from', 'taken_to', 'tags', 'tag_mode', 'sharp', 'face_ids', 'face_similarity'])
      expect(q).not.toHaveProperty(k)
  })

  it('maps each control to its param', () => {
    const q = buildPhotosQuery(
      3,
      { ...DEFAULT_FILTER, lens: ['50mm', 'RF 24-70mm F2.8, L'], takenFrom: '2024-05-01', takenTo: '2024-05-31', tags: ['kyoto', 'a\\b'], tagMode: 'all', sharp: 'blurry', faceIds: [4, 9], faceSimilarity: 0.5 },
    )
    expect(q).toEqual({
      session_id: 3,
      lens: ['50mm', 'RF 24-70mm F2.8, L'],
      taken_from: '2024-05-01',
      taken_to: '2024-05-31',
      tags: ['kyoto', 'a\\b'],
      tag_mode: 'all',
      sharp: 'blurry',
      face_ids: [4, 9],
      face_similarity: 0.5,
    })
  })

  it('omits the tag mode for "any" and the similarity for the default threshold', () => {
    const q = buildPhotosQuery(1, { ...DEFAULT_FILTER, tags: ['x'], faceIds: [1] })
    expect(q.tag_mode).toBeUndefined()
    expect(q.face_similarity).toBeUndefined()
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, tagMode: 'all' }).tag_mode).toBeUndefined()
    expect(buildPhotosQuery(1, { ...DEFAULT_FILTER, faceSimilarity: 0.6 }).face_similarity).toBeUndefined()
  })

  it('serialises to the query string with commas inside names escaped', () => {
    const qs = photosQueryString(buildPhotosQuery(1, { ...DEFAULT_FILTER, lens: ['a,b', 'c'], tags: ['x\\y'], takenFrom: '2024-01-02', sharp: 'sharp', faceIds: [2, 3], faceSimilarity: 0.42 }))
    const p = new URLSearchParams(qs)
    expect(p.get('lens')).toBe('a\\,b,c')
    expect(p.get('tags')).toBe('x\\\\y')
    expect(p.get('taken_from')).toBe('2024-01-02')
    expect(p.get('taken_to')).toBeNull()
    expect(p.get('sharp')).toBe('sharp')
    expect(p.get('face_ids')).toBe('2,3')
    expect(p.get('face_similarity')).toBe('0.42')
  })

  it('counts as an active filter', () => {
    for (const patch of [{ lens: ['x'] }, { takenFrom: '2024-01-01' }, { takenTo: '2024-01-01' }, { tags: ['x'] }, { sharp: 'unknown' as const }, { faceIds: [1] }])
      expect(isFilterActive({ ...DEFAULT_FILTER, ...patch })).toBe(true)
    expect(isFilterActive({ ...DEFAULT_FILTER, tagMode: 'all', faceSimilarity: 0.6 })).toBe(false)
  })

  it('toggles values', () => {
    expect(toggleValue([], 'a')).toEqual(['a'])
    expect(toggleValue(['a', 'b'], 'a')).toEqual(['b'])
  })

  it('validates dates', () => {
    expect(isIsoDate('2024-02-29')).toBe(true)
    expect(isIsoDate('2023-02-29')).toBe(false)
    expect(isIsoDate('2024-13-01')).toBe(false)
    expect(isIsoDate('2024-1-1')).toBe(false)
    expect(isIsoDate('')).toBe(false)
    expect(isIsoDate(null)).toBe(false)
    expect(msToDate(1_700_000_000_000)).toBe('2023-11-14')
  })

  it('keeps the date range ordered', () => {
    const none = { takenFrom: null, takenTo: null }
    expect(setDateRange(none, 'from', '2024-05-10')).toEqual({ takenFrom: '2024-05-10', takenTo: null })
    expect(setDateRange({ takenFrom: '2024-05-10', takenTo: null }, 'to', '2024-05-20')).toEqual({ takenFrom: '2024-05-10', takenTo: '2024-05-20' })
    // moving one end past the other drags the other along
    expect(setDateRange({ takenFrom: '2024-05-10', takenTo: '2024-05-20' }, 'from', '2024-06-01')).toEqual({ takenFrom: '2024-06-01', takenTo: '2024-06-01' })
    expect(setDateRange({ takenFrom: '2024-05-10', takenTo: '2024-05-20' }, 'to', '2024-05-01')).toEqual({ takenFrom: '2024-05-01', takenTo: '2024-05-01' })
    // clearing one end, and ignoring garbage
    expect(setDateRange({ takenFrom: '2024-05-10', takenTo: '2024-05-20' }, 'from', '')).toEqual({ takenFrom: null, takenTo: '2024-05-20' })
    expect(setDateRange({ takenFrom: '2024-05-10', takenTo: null }, 'to', '2024-99-99')).toEqual({ takenFrom: '2024-05-10', takenTo: null })
  })

  it('clamps the face similarity to the slider range', () => {
    expect(DEFAULT_FACE_SIMILARITY).toBe(0.38)
    expect(clampFaceSimilarity(0.05)).toBe(0.2)
    expect(clampFaceSimilarity(0.9)).toBe(0.8)
    expect(clampFaceSimilarity(0.4567)).toBe(0.46)
  })
})
