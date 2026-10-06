import type {
  ApiErrorBody,
  ExportBody,
  FsList,
  ImportBody,
  Photo,
  PhotoPatchBody,
  PhotosQuery,
  PhotosResponse,
  Session,
} from './types'

export class ApiError extends Error {
  status: number
  code: string
  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

const BASE = '/api'

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(BASE + path, {
    method,
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  if (!res.ok) {
    let code = 'http_error'
    let message = `${res.status} ${res.statusText}`
    try {
      const j = (await res.json()) as ApiErrorBody
      code = j.error.code
      message = j.error.message
    } catch {
      /* non-JSON error body */
    }
    throw new ApiError(res.status, code, message)
  }
  if (res.status === 204) return undefined as T
  return (await res.json()) as T
}

/** Builds the query string for GET /api/photos. Omits unset values. */
export function photosQueryString(q: PhotosQuery): string {
  const p = new URLSearchParams()
  p.set('session_id', String(q.session_id))
  if (q.rating_gte !== undefined) p.set('rating_gte', String(q.rating_gte))
  if (q.flag) p.set('flag', q.flag)
  if (q.color_label) p.set('color_label', q.color_label)
  if (q.sort) p.set('sort', q.sort)
  if (q.cursor) p.set('cursor', q.cursor)
  if (q.limit) p.set('limit', String(q.limit))
  return p.toString()
}

export const api = {
  health: () => request<{ ok: boolean; version: string }>('GET', '/health'),
  importFolder: (body: ImportBody) => request<{ session: Session }>('POST', '/import', body),
  sessions: () => request<{ sessions: Session[] }>('GET', '/sessions'),
  session: (id: number) => request<{ session: Session }>('GET', `/sessions/${id}`),
  deleteSession: (id: number) => request<void>('DELETE', `/sessions/${id}`),
  photos: (q: PhotosQuery) => request<PhotosResponse>('GET', `/photos?${photosQueryString(q)}`),
  photo: (id: number) => request<{ photo: Photo }>('GET', `/photos/${id}`),
  patchPhotos: (body: PhotoPatchBody) => request<{ updated: number }>('PATCH', '/photos', body),
  viewport: (ids: number[]) => request<void>('POST', '/viewport', { ids }),
  exportPhotos: (body: ExportBody) => request<{ task_id: string }>('POST', '/export', body),
  fsRoots: () => request<{ roots: string[] }>('GET', '/fs/roots'),
  fsList: (path?: string) =>
    request<FsList>('GET', `/fs/list${path ? `?path=${encodeURIComponent(path)}` : ''}`),
}

/** Page size used when pulling a whole session (contract: max 5000). */
export const PAGE_SIZE = 5000

/** Pulls every page of a photos query (client virtualizes up to ~20k items). */
export async function fetchAllPhotos(
  q: Omit<PhotosQuery, 'cursor' | 'limit'>,
): Promise<{ photos: Photo[]; total: number }> {
  const photos: Photo[] = []
  let cursor: string | undefined
  for (;;) {
    const page = await api.photos({ ...q, cursor, limit: PAGE_SIZE })
    photos.push(...page.photos)
    if (!page.next_cursor) return { photos, total: page.total }
    cursor = page.next_cursor
  }
}

// ---- Binary URLs ----

export const thumbUrl = (p: Pick<Photo, 'id' | 'thumb_version'>, s: 256 | 512 = 256) =>
  `${BASE}/thumb/${p.id}?s=${s}&v=${encodeURIComponent(p.thumb_version)}`
export const previewUrl = (id: number, s: 1024 | 2048 | 4096 = 2048) => `${BASE}/preview/${id}?s=${s}`
export const originalUrl = (id: number) => `${BASE}/original/${id}`
