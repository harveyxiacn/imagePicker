import type { QueryClient } from '@tanstack/react-query'
import type { Photo, ServerEvent, Session } from '@/api/types'
import type { PhotosListQuery } from './filter'

export const qk = {
  sessions: ['sessions'] as const,
  session: (id: number) => ['session', id] as const,
  photosAll: ['photos'] as const,
  photos: (sessionId: number, q: PhotosListQuery) => ['photos', sessionId, q] as const,
}

export interface PhotosData {
  photos: Photo[]
  total: number
}

export type PhotoPatch = Partial<Photo>

/** Apply per-photo patches to every cached photo list in place (immutably). */
export function patchPhotosInCache(qc: QueryClient, patches: Map<number, PhotoPatch>): void {
  if (patches.size === 0) return
  qc.setQueriesData<PhotosData>({ queryKey: qk.photosAll }, (old) => {
    if (!old) return old
    let touched = false
    const photos = old.photos.map((p) => {
      const patch = patches.get(p.id)
      if (!patch) return p
      touched = true
      return { ...p, ...patch }
    })
    return touched ? { ...old, photos } : old
  })
}

export type TaskEvent = Extract<ServerEvent, { type: 'task.progress' }>

export interface ApplyResult {
  patchedIds: number
  invalidatedSessions: number[]
  sessions: number
  tasks: number
}

/**
 * Coalesce a batch of server events and apply them to the query cache:
 *  - `photos.updated` + `thumbs.ready` merge per photo id (later wins) into ONE cache pass
 *  - `photos.added` -> one refetch per session
 *  - `session.updated` -> last write per session wins
 *  - `task.progress` -> last event per task id is forwarded
 */
export function applyEvents(
  qc: QueryClient,
  events: ServerEvent[],
  onTask?: (e: TaskEvent) => void,
): ApplyResult {
  const patches = new Map<number, PhotoPatch>()
  const added = new Set<number>()
  const sessions = new Map<number, Session>()
  const tasks = new Map<string, TaskEvent>()

  for (const ev of events) {
    switch (ev.type) {
      case 'photos.updated':
        for (const { id, ...rest } of ev.items) patches.set(id, { ...patches.get(id), ...rest })
        break
      case 'thumbs.ready':
        for (const { id, v } of ev.items)
          patches.set(id, { ...patches.get(id), thumb_ready: true, thumb_version: v })
        break
      case 'photos.added':
        added.add(ev.session_id)
        break
      case 'session.updated':
        sessions.set(ev.session.id, ev.session)
        break
      case 'task.progress':
        tasks.set(ev.task_id, ev)
        break
    }
  }

  patchPhotosInCache(qc, patches)

  for (const s of sessions.values()) {
    qc.setQueryData<{ sessions: Session[] }>(qk.sessions, (old) => {
      if (!old) return old
      const exists = old.sessions.some((x) => x.id === s.id)
      return {
        sessions: exists
          ? old.sessions.map((x) => (x.id === s.id ? s : x))
          : [s, ...old.sessions].sort((a, b) => b.created_at - a.created_at),
      }
    })
    qc.setQueryData(qk.session(s.id), { session: s })
  }

  for (const sid of added) {
    void qc.invalidateQueries({ queryKey: ['photos', sid] })
    void qc.invalidateQueries({ queryKey: qk.session(sid) })
  }

  if (onTask) for (const t of tasks.values()) onTask(t)

  return {
    patchedIds: patches.size,
    invalidatedSessions: [...added],
    sessions: sessions.size,
    tasks: tasks.size,
  }
}

/** Find current values for the given ids from any cached photo list. */
export function findCachedPhotos(qc: QueryClient, ids: Iterable<number>): Photo[] {
  const want = new Set(ids)
  const found = new Map<number, Photo>()
  for (const [, data] of qc.getQueriesData<PhotosData>({ queryKey: qk.photosAll })) {
    if (!data) continue
    for (const p of data.photos) {
      if (want.has(p.id) && !found.has(p.id)) found.set(p.id, p)
    }
    if (found.size === want.size) break
  }
  return [...found.values()]
}
