import type { FaceSearchResponse, FaceSearchSimilarFace } from '@/api/types'

/** What is searched: an uploaded photo, or a face already in the library (face context menu). */
export type FaceQuery = { kind: 'image'; file: File } | { kind: 'face'; faceId: number; photoId: number }

/**
 * State machine of the "search by face" dialog.
 *
 *   idle --pick(file)/pickFace--> searching --result--> results --selectFace(i)--> searching --result--> results
 *                                     |--(no faces)--> noFace           reset --> idle   (any state --fail--> error)
 *
 * In `results`, `chosen` are the similar faces that "this is a new person" would take (toggleChosen); a face
 * query always brings its own face along.
 */
export type FaceSearchState =
  | { status: 'idle' }
  | { status: 'searching'; query: FaceQuery; faceIndex: number | null; faces: FaceSearchResponse['faces_detected'] }
  | { status: 'results'; query: FaceQuery; faceIndex: number; resp: FaceSearchResponse; chosen: ReadonlySet<number> }
  | { status: 'noFace'; query: FaceQuery }
  | { status: 'error'; query: FaceQuery | null; message: string }

export type FaceSearchEvent =
  | { type: 'pick'; file: File }
  | { type: 'pickFace'; faceId: number; photoId: number }
  | { type: 'result'; resp: FaceSearchResponse }
  | { type: 'selectFace'; index: number }
  | { type: 'toggleChosen'; faceId: number }
  | { type: 'fail'; message: string }
  | { type: 'reset' }

export const initialFaceSearch: FaceSearchState = { status: 'idle' }

/** Initial state of a dialog opened on a library face (or idle). */
export function initFaceSearch(face: { id: number; photo_id: number } | null): FaceSearchState {
  return face ? { status: 'searching', query: { kind: 'face', faceId: face.id, photoId: face.photo_id }, faceIndex: null, faces: [] } : initialFaceSearch
}

const queryOf = (s: FaceSearchState): FaceQuery | null => (s.status === 'idle' ? null : s.query)

/** Similar faces at least this close are preselected for "this is a new person". */
export const PRESELECT_SIMILARITY = 0.45

/** Photo whose face is the query itself: no other face of it can join the same person. */
export const queryPhoto = (q: FaceQuery): number | null => (q.kind === 'face' ? q.photoId : null)

/** Default "new person" selection: close matches, best first, at most one face per photo. */
export function defaultChosen(similar: FaceSearchSimilarFace[], query: FaceQuery, min = PRESELECT_SIMILARITY): ReadonlySet<number> {
  const taken = new Set<number>()
  const qp = queryPhoto(query)
  if (qp !== null) taken.add(qp)
  const out = new Set<number>()
  for (const f of [...similar].sort((a, b) => b.similarity - a.similarity)) {
    if (f.similarity < min || taken.has(f.photo_id)) continue
    taken.add(f.photo_id)
    out.add(f.face_id)
  }
  return out
}

/** Toggles one face; choosing it drops another chosen face of the same photo (a person appears once per photo). */
export function toggleChosen(chosen: ReadonlySet<number>, similar: FaceSearchSimilarFace[], faceId: number, query: FaceQuery): ReadonlySet<number> {
  const f = similar.find((x) => x.face_id === faceId)
  if (!f || f.photo_id === queryPhoto(query)) return chosen
  const out = new Set(chosen)
  if (out.has(faceId)) {
    out.delete(faceId)
    return out
  }
  for (const o of similar) if (o.photo_id === f.photo_id) out.delete(o.face_id)
  out.add(faceId)
  return out
}

/** Faces sent to `POST /api/people`: the query face (if it is a library face) and the chosen similar ones. */
export function newPersonFaceIds(query: FaceQuery, similar: FaceSearchSimilarFace[], chosen: ReadonlySet<number>): number[] {
  const ids = similar.filter((f) => chosen.has(f.face_id)).map((f) => f.face_id)
  return query.kind === 'face' ? [query.faceId, ...ids.filter((id) => id !== query.faceId)] : ids
}

export function faceSearchReducer(s: FaceSearchState, e: FaceSearchEvent): FaceSearchState {
  switch (e.type) {
    case 'pick':
      return { status: 'searching', query: { kind: 'image', file: e.file }, faceIndex: null, faces: [] }
    case 'pickFace':
      return initFaceSearch({ id: e.faceId, photo_id: e.photoId })
    case 'result': {
      if (s.status !== 'searching') return s
      if (e.resp.faces_detected.length === 0) return { status: 'noFace', query: s.query }
      const idx = Math.min(Math.max(0, s.faceIndex ?? e.resp.query_face), e.resp.faces_detected.length - 1)
      return { status: 'results', query: s.query, faceIndex: idx, resp: e.resp, chosen: defaultChosen(e.resp.similar_faces, s.query) }
    }
    case 'selectFace':
      if (s.status !== 'results' || s.query.kind !== 'image' || e.index === s.faceIndex || e.index < 0 || e.index >= s.resp.faces_detected.length) return s
      return { status: 'searching', query: s.query, faceIndex: e.index, faces: s.resp.faces_detected }
    case 'toggleChosen': {
      if (s.status !== 'results') return s
      const chosen = toggleChosen(s.chosen, s.resp.similar_faces, e.faceId, s.query)
      return chosen === s.chosen ? s : { ...s, chosen }
    }
    case 'fail':
      return { status: 'error', query: queryOf(s), message: e.message }
    case 'reset':
      return initialFaceSearch
  }
}

/** First image among dropped / pasted files. */
export function firstImage(files: ArrayLike<File> | null | undefined): File | null {
  if (!files) return null
  for (const f of Array.from(files)) if (f.type.startsWith('image/')) return f
  return null
}

/** Similarity 0..1 -> whole percent. */
export const similarityPct = (v: number): number => Math.round(Math.min(1, Math.max(0, v)) * 100)
