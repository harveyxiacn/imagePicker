import type { FaceSearchResponse } from '@/api/types'

/**
 * State machine of the "search by face" dialog.
 *
 *   idle --pick(file)--> searching --result--> results --selectFace(i)--> searching --result--> results
 *                            |--(no faces)--> noFace            reset --> idle        (any state --fail--> error)
 */
export type FaceSearchState =
  | { status: 'idle' }
  | { status: 'searching'; file: File; faceIndex: number | null; faces: FaceSearchResponse['faces_detected'] }
  | { status: 'results'; file: File; faceIndex: number; resp: FaceSearchResponse }
  | { status: 'noFace'; file: File }
  | { status: 'error'; file: File | null; message: string }

export type FaceSearchEvent =
  | { type: 'pick'; file: File }
  | { type: 'result'; resp: FaceSearchResponse }
  | { type: 'selectFace'; index: number }
  | { type: 'fail'; message: string }
  | { type: 'reset' }

export const initialFaceSearch: FaceSearchState = { status: 'idle' }

const fileOf = (s: FaceSearchState): File | null => (s.status === 'idle' ? null : s.file)

export function faceSearchReducer(s: FaceSearchState, e: FaceSearchEvent): FaceSearchState {
  switch (e.type) {
    case 'pick':
      return { status: 'searching', file: e.file, faceIndex: null, faces: [] }
    case 'result': {
      if (s.status !== 'searching') return s
      if (e.resp.faces_detected.length === 0) return { status: 'noFace', file: s.file }
      const idx = Math.min(Math.max(0, s.faceIndex ?? e.resp.query_face), e.resp.faces_detected.length - 1)
      return { status: 'results', file: s.file, faceIndex: idx, resp: e.resp }
    }
    case 'selectFace':
      if (s.status !== 'results' || e.index === s.faceIndex || e.index < 0 || e.index >= s.resp.faces_detected.length) return s
      return { status: 'searching', file: s.file, faceIndex: e.index, faces: s.resp.faces_detected }
    case 'fail':
      return { status: 'error', file: fileOf(s), message: e.message }
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
