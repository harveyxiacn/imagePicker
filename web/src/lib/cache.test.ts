import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it, vi } from 'vitest'
import type { Photo, ServerEvent, Session } from '@/api/types'
import { applyEvents, findCachedPhotos, qk, type PhotosData } from './cache'
import { buildPhotosQuery, DEFAULT_FILTER } from './filter'

const photo = (id: number, over: Partial<Photo> = {}): Photo => ({
  id,
  session_id: 1,
  path: `/p/${id}.jpg`,
  file_name: `${id}.jpg`,
  format: 'jpeg',
  file_size: 1,
  width: 100,
  height: 100,
  taken_at: id,
  camera: null,
  lens: null,
  focal_mm: null,
  aperture: null,
  shutter_s: null,
  iso: null,
  user_rating: null,
  ai_rating: null,
  flag: 0,
  color_label: null,
  burst_id: null,
  thumb_ready: false,
  thumb_version: 'v0',
  ...over,
})

const session = (over: Partial<Session> = {}): Session => ({
  id: 1,
  title: 's',
  root_path: '/p',
  created_at: 1,
  photo_count: 3,
  picked_count: 0,
  rejected_count: 0,
  rated_count: 0,
  cover_photo_id: 1,
  import_state: 'ready',
  ...over,
})

function setup() {
  const qc = new QueryClient()
  const all = buildPhotosQuery(1, DEFAULT_FILTER)
  const filtered = buildPhotosQuery(1, { ...DEFAULT_FILTER, ratingGte: 3 })
  qc.setQueryData<PhotosData>(qk.photos(1, all), { photos: [photo(1), photo(2), photo(3)], total: 3 })
  qc.setQueryData<PhotosData>(qk.photos(1, filtered), { photos: [photo(2)], total: 1 })
  qc.setQueryData(qk.sessions, { sessions: [session()] })
  return { qc, all, filtered }
}

const get = (qc: QueryClient, q: ReturnType<typeof buildPhotosQuery>) => qc.getQueryData<PhotosData>(qk.photos(1, q))!

describe('applyEvents', () => {
  it('coalesces photos.updated + thumbs.ready per id into every cached list (later wins)', () => {
    const { qc, all, filtered } = setup()
    const events: ServerEvent[] = [
      { type: 'photos.updated', items: [{ id: 2, user_rating: 3 }] },
      { type: 'thumbs.ready', items: [{ id: 2, v: 'abc' }, { id: 3, v: 'def' }] },
      { type: 'photos.updated', items: [{ id: 2, user_rating: 5, flag: 1 }] },
    ]
    const r = applyEvents(qc, events)
    expect(r.patchedIds).toBe(2)
    const list = get(qc, all).photos
    expect(list[1]).toMatchObject({ id: 2, user_rating: 5, flag: 1, thumb_ready: true, thumb_version: 'abc' })
    expect(list[2]).toMatchObject({ id: 3, thumb_ready: true, thumb_version: 'def' })
    expect(list[0].thumb_ready).toBe(false)
    expect(get(qc, filtered).photos[0]).toMatchObject({ user_rating: 5, thumb_version: 'abc' })
  })

  it('keeps object identity for untouched photos and lists', () => {
    const { qc, all } = setup()
    const before = get(qc, all)
    applyEvents(qc, [{ type: 'photos.updated', items: [{ id: 3, flag: -1 }] }])
    const after = get(qc, all)
    expect(after.photos[0]).toBe(before.photos[0])
    expect(after.photos[2]).not.toBe(before.photos[2])
    // no-op patch for an id outside the list leaves the cache entry untouched
    applyEvents(qc, [{ type: 'photos.updated', items: [{ id: 999, flag: 1 }] }])
    expect(get(qc, all)).toBe(after)
  })

  it('photos.added triggers a single invalidation per session', () => {
    const { qc } = setup()
    const spy = vi.spyOn(qc, 'invalidateQueries')
    const r = applyEvents(qc, [
      { type: 'photos.added', session_id: 1, count: 100 },
      { type: 'photos.added', session_id: 1, count: 200 },
    ])
    expect(r.invalidatedSessions).toEqual([1])
    const photoCalls = spy.mock.calls.filter((c) => JSON.stringify(c[0]?.queryKey) === JSON.stringify(['photos', 1]))
    expect(photoCalls).toHaveLength(1)
  })

  it('session.updated: last event wins and is written to list + detail caches', () => {
    const { qc } = setup()
    applyEvents(qc, [
      { type: 'session.updated', session: session({ photo_count: 10, import_state: 'scanning' }) },
      { type: 'session.updated', session: session({ photo_count: 20, import_state: 'thumbnailing' }) },
    ])
    const list = qc.getQueryData<{ sessions: Session[] }>(qk.sessions)!
    expect(list.sessions[0]).toMatchObject({ photo_count: 20, import_state: 'thumbnailing' })
    expect(qc.getQueryData<{ session: Session }>(qk.session(1))!.session.photo_count).toBe(20)
  })

  it('session.updated for an unknown session prepends it newest-first', () => {
    const { qc } = setup()
    applyEvents(qc, [{ type: 'session.updated', session: session({ id: 2, created_at: 99 }) }])
    expect(qc.getQueryData<{ sessions: Session[] }>(qk.sessions)!.sessions.map((s) => s.id)).toEqual([2, 1])
  })

  it('forwards only the latest task.progress per task', () => {
    const { qc } = setup()
    const onTask = vi.fn()
    const base = { type: 'task.progress', task_id: 'export-1', kind: 'export', total: 10 } as const
    applyEvents(
      qc,
      [
        { ...base, done: 1, state: 'running' },
        { ...base, done: 10, state: 'done' },
        { ...base, task_id: 'export-2', done: 2, state: 'running' },
      ],
      onTask,
    )
    expect(onTask).toHaveBeenCalledTimes(2)
    expect(onTask.mock.calls.map((c) => [c[0].task_id, c[0].done])).toEqual([
      ['export-1', 10],
      ['export-2', 2],
    ])
  })
})

describe('findCachedPhotos', () => {
  it('finds photos across cached lists', () => {
    const { qc } = setup()
    expect(findCachedPhotos(qc, [1, 3]).map((p) => p.id).sort()).toEqual([1, 3])
  })
})
