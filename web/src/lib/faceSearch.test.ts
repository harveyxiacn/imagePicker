import { describe, expect, it } from 'vitest'
import type { FaceSearchResponse } from '@/api/types'
import { faceSearchReducer, firstImage, initialFaceSearch, similarityPct, type FaceSearchState } from './faceSearch'

const file = new File(['x'], 'q.png', { type: 'image/png' })
const resp = (n: number, queryFace = 0): FaceSearchResponse => ({
  faces_detected: Array.from({ length: n }, (_, i) => [0.1 * i, 0.1, 0.1, 0.1] as [number, number, number, number]),
  query_face: queryFace,
  candidates: [{ person_id: 1, person_name: 'A', similarity: 0.9 }],
  similar_faces: [],
})

describe('face search state machine', () => {
  it('idle -> searching -> results with the server-chosen face', () => {
    let s = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    expect(s).toMatchObject({ status: 'searching', faceIndex: null })
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
    s = faceSearchReducer(s, { type: 'result', resp: resp(2) })
    expect(faceSearchReducer(s, { type: 'selectFace', index: 0 })).toBe(s)
    expect(faceSearchReducer(s, { type: 'selectFace', index: 5 })).toBe(s)
    expect(faceSearchReducer(initialFaceSearch, { type: 'result', resp: resp(1) })).toBe(initialFaceSearch)
  })

  it('reports an image without faces', () => {
    let s: FaceSearchState = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    s = faceSearchReducer(s, { type: 'result', resp: resp(0) })
    expect(s).toEqual({ status: 'noFace', file })
  })

  it('fails from any state and can restart', () => {
    let s: FaceSearchState = faceSearchReducer(initialFaceSearch, { type: 'pick', file })
    s = faceSearchReducer(s, { type: 'fail', message: 'boom' })
    expect(s).toEqual({ status: 'error', file, message: 'boom' })
    expect(faceSearchReducer(s, { type: 'pick', file })).toMatchObject({ status: 'searching' })
    expect(faceSearchReducer(s, { type: 'reset' })).toBe(initialFaceSearch)
    expect(faceSearchReducer(initialFaceSearch, { type: 'fail', message: 'x' })).toEqual({ status: 'error', file: null, message: 'x' })
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
