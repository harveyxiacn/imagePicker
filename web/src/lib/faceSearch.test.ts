import { describe, expect, it } from 'vitest'
import type { FaceSearchResponse, FaceSearchSimilarFace } from '@/api/types'
import {
  defaultChosen,
  faceSearchReducer,
  firstImage,
  initFaceSearch,
  initialFaceSearch,
  newPersonFaceIds,
  similarityPct,
  toggleChosen,
  type FaceQuery,
  type FaceSearchState,
} from './faceSearch'

const file = new File(['x'], 'q.png', { type: 'image/png' })
const similar = (...rows: [number, number, number][]): FaceSearchSimilarFace[] =>
  rows.map(([face_id, photo_id, similarity]) => ({ face_id, photo_id, similarity, person_id: 7, person_name: null }))
const resp = (n: number, queryFace = 0, similar_faces: FaceSearchSimilarFace[] = []): FaceSearchResponse => ({
  faces_detected: Array.from({ length: n }, (_, i) => [0.1 * i, 0.1, 0.1, 0.1] as [number, number, number, number]),
  query_face: queryFace,
  candidates: [{ person_id: 1, person_name: 'A', similarity: 0.9 }],
  similar_faces,
})
const image: FaceQuery = { kind: 'image', file }
const libraryFace: FaceQuery = { kind: 'face', faceId: 50, photoId: 5 }

describe('face search state machine', () => {
  it('idle -> searching -> results with the server-chosen face', () => {
    let s = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    expect(s).toMatchObject({ status: 'searching', faceIndex: null, query: image })
    s = faceSearchReducer(s, { type: 'result', resp: resp(3, 1) })
    expect(s).toMatchObject({ status: 'results', faceIndex: 1 })
  })

  it('picking another face re-searches that face and keeps the detected boxes', () => {
    let s: FaceSearchState = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    s = faceSearchReducer(s, { type: 'result', resp: resp(3) })
    s = faceSearchReducer(s, { type: 'selectFace', index: 2 })
    expect(s).toMatchObject({ status: 'searching', faceIndex: 2 })
    expect((s as Extract<FaceSearchState, { status: 'searching' }>).faces).toHaveLength(3)
    s = faceSearchReducer(s, { type: 'result', resp: resp(3, 2) })
    expect(s).toMatchObject({ status: 'results', faceIndex: 2 })
  })

  it('ignores selecting the current / an invalid face and events in the wrong state', () => {
    let s: FaceSearchState = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    expect(faceSearchReducer(s, { type: 'selectFace', index: 0 })).toBe(s) // not in results
    expect(faceSearchReducer(s, { type: 'toggleChosen', faceId: 1 })).toBe(s)
    s = faceSearchReducer(s, { type: 'result', resp: resp(2) })
    expect(faceSearchReducer(s, { type: 'selectFace', index: 0 })).toBe(s)
    expect(faceSearchReducer(s, { type: 'selectFace', index: 5 })).toBe(s)
    expect(faceSearchReducer(s, { type: 'toggleChosen', faceId: 404 })).toBe(s)
    expect(faceSearchReducer(initialFaceSearch, { type: 'result', resp: resp(1) })).toBe(initialFaceSearch)
  })

  it('reports an image without faces', () => {
    let s: FaceSearchState = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    s = faceSearchReducer(s, { type: 'result', resp: resp(0) })
    expect(s).toEqual({ status: 'noFace', query: image })
  })

  it('fails from any state and can restart', () => {
    let s: FaceSearchState = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    s = faceSearchReducer(s, { type: 'fail', message: 'boom' })
    expect(s).toEqual({ status: 'error', query: image, message: 'boom' })
    expect(faceSearchReducer(s, { type: 'pick', file })).toMatchObject({ status: 'searching' })
    expect(faceSearchReducer(s, { type: 'reset' })).toBe(initialFaceSearch)
    expect(faceSearchReducer(initialFaceSearch, { type: 'fail', message: 'x' })).toEqual({ status: 'error', query: null, message: 'x' })
  })

  it('starts on a library face (face menu) and never re-picks a face of it', () => {
    expect(initFaceSearch(null)).toBe(initialFaceSearch)
    let s = initFaceSearch({ id: 50, photo_id: 5 })
    expect(s).toEqual({ status: 'searching', query: libraryFace, faceIndex: null, faces: [] })
    expect(faceSearchReducer(initialFaceSearch, { type: 'pickFace', faceId: 50, photoId: 5 })).toEqual(s)
    s = faceSearchReducer(s, { type: 'result', resp: resp(1, 0, similar([51, 6, 0.8])) })
    expect(s).toMatchObject({ status: 'results', faceIndex: 0, query: libraryFace })
    expect(faceSearchReducer(s, { type: 'selectFace', index: 0 })).toBe(s)
  })

  it('picks the first image of dropped files and formats similarity', () => {
    const txt = new File(['t'], 'a.txt', { type: 'text/plain' })
    expect(firstImage([txt, file])).toBe(file)
    expect(firstImage([txt])).toBeNull()
    expect(firstImage(null)).toBeNull()
    expect(similarityPct(0.874)).toBe(87)
    expect(similarityPct(1.4)).toBe(100)
  })
})

describe('"this is a new person" selection', () => {
  // face, photo, similarity
  const faces = similar([1, 10, 0.9], [2, 10, 0.95], [3, 11, 0.6], [4, 12, 0.3], [5, 5, 0.99])

  it('preselects close matches, best first, one face per photo', () => {
    expect([...defaultChosen(faces, image)].sort()).toEqual([2, 3, 5])
    // a library face query already occupies its own photo
    expect([...defaultChosen(faces, libraryFace)].sort()).toEqual([2, 3])
    expect(defaultChosen(faces, image, 0.97).size).toBe(1)
    expect(defaultChosen([], image).size).toBe(0)
  })

  it('toggles faces and keeps one face per photo', () => {
    let chosen = defaultChosen(faces, image)
    chosen = toggleChosen(chosen, faces, 1, image) // same photo as 2: replaces it
    expect([...chosen].sort()).toEqual([1, 3, 5])
    chosen = toggleChosen(chosen, faces, 3, image)
    expect([...chosen].sort()).toEqual([1, 5])
    chosen = toggleChosen(chosen, faces, 4, image)
    expect([...chosen].sort()).toEqual([1, 4, 5])
    expect(toggleChosen(chosen, faces, 99, image)).toBe(chosen) // unknown face
    expect(toggleChosen(chosen, faces, 5, libraryFace)).toBe(chosen) // the query face's own photo
  })

  it('sends the query face first, then the chosen faces in result order', () => {
    const chosen = new Set([3, 1])
    expect(newPersonFaceIds(image, faces, chosen)).toEqual([1, 3])
    expect(newPersonFaceIds(libraryFace, faces, chosen)).toEqual([50, 1, 3])
    expect(newPersonFaceIds(libraryFace, faces, new Set())).toEqual([50])
  })

  it('the reducer preselects on results and toggles in place', () => {
    let s: FaceSearchState = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    s = faceSearchReducer(s, { type: 'result', resp: resp(1, 0, faces) })
    expect(s.status === 'results' && [...s.chosen].sort()).toEqual([2, 3, 5])
    s = faceSearchReducer(s, { type: 'toggleChosen', faceId: 4 })
    expect(s.status === 'results' && [...s.chosen].sort()).toEqual([2, 3, 4, 5])
  })
})
