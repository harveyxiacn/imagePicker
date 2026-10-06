import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it, vi } from 'vitest'
import type { AnalysisStatus, HardwareInfo, Photo, ServerEvent, Session } from '@/api/types'
import { ANALYSIS_DIRECT_FETCH_MAX, applyEvents, findCachedPhotos, qk, type PhotosData } from './cache'
import { makePhoto } from './fixtures'
import { buildPhotosQuery, DEFAULT_FILTER } from './filter'

const photo = (id: number, over: Partial<Photo> = {}): Photo => makePhoto(id, { taken_at: id, ...over })

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

describe('applyEvents: analysis (M2)', () => {
  const scored = (id: number, over: Partial<Photo> = {}) =>
    photo(id, { analyzed: true, ai_score: 0.8, ai_rating: 4, issues: ['blurry'], burst_id: 7, rank_in_burst: 1, burst_size: 3, scene_type: 'group', face_count: 2, subject_face_count: 2, ...over })

  it('merges analysis.updated ids per session across events and refetches each photo once', async () => {
    const { qc, all, filtered } = setup()
    const fetchPhoto = vi.fn((id: number) => Promise.resolve(scored(id, { user_rating: 99 })))
    const r = applyEvents(
      qc,
      [
        { type: 'analysis.updated', session_id: 1, ids: [1, 2] },
        { type: 'analysis.updated', session_id: 1, ids: [2, 3] },
        { type: 'analysis.updated', session_id: 5, ids: [42] },
      ],
      undefined,
      { fetchPhoto },
    )
    expect(r.analysisIds).toEqual({ 1: [1, 2, 3], 5: [42] })
    await vi.waitFor(() => expect(get(qc, all).photos[2].analyzed).toBe(true))
    expect(fetchPhoto.mock.calls.map((c) => c[0]).sort((a, b) => a - b)).toEqual([1, 2, 3, 42])
    // AI fields are merged in place into every cached list
    expect(get(qc, all).photos[0]).toMatchObject({ analyzed: true, ai_rating: 4, issues: ['blurry'], burst_id: 7, rank_in_burst: 1, burst_size: 3 })
    expect(get(qc, filtered).photos[0]).toMatchObject({ id: 2, analyzed: true, scene_type: 'group' })
  })

  it('never overwrites user-editable fields (optimistic edits survive the refetch)', async () => {
    const { qc, all } = setup()
    qc.setQueryData<PhotosData>(qk.photos(1, all), { photos: [photo(1, { user_rating: 2, flag: 1 })], total: 1 })
    applyEvents(qc, [{ type: 'analysis.updated', session_id: 1, ids: [1] }], undefined, {
      fetchPhoto: () => Promise.resolve(scored(1, { user_rating: null, flag: 0 })),
    })
    await vi.waitFor(() => expect(get(qc, all).photos[0].analyzed).toBe(true))
    expect(get(qc, all).photos[0]).toMatchObject({ user_rating: 2, flag: 1, ai_score: 0.8 })
  })

  it('keeps identity of photos that were not part of the update (no full-grid re-render)', async () => {
    const { qc, all } = setup()
    const before = get(qc, all)
    applyEvents(qc, [{ type: 'analysis.updated', session_id: 1, ids: [2] }], undefined, { fetchPhoto: (id) => Promise.resolve(scored(id)) })
    await vi.waitFor(() => expect(get(qc, all).photos[1].analyzed).toBe(true))
    expect(get(qc, all).photos[0]).toBe(before.photos[0])
    expect(get(qc, all).photos[2]).toBe(before.photos[2])
  })

  it('large updates do not fetch per photo; they schedule one throttled list refetch', async () => {
    vi.useFakeTimers()
    try {
      const { qc } = setup()
      const fetchPhoto = vi.fn()
      const spy = vi.spyOn(qc, 'invalidateQueries')
      const ids = Array.from({ length: ANALYSIS_DIRECT_FETCH_MAX + 10 }, (_, i) => i + 1)
      applyEvents(qc, [{ type: 'analysis.updated', session_id: 1, ids }], undefined, { fetchPhoto })
      applyEvents(qc, [{ type: 'analysis.updated', session_id: 1, ids }], undefined, { fetchPhoto })
      expect(fetchPhoto).not.toHaveBeenCalled()
      const listCalls = () => spy.mock.calls.filter((c) => JSON.stringify(c[0]?.queryKey) === JSON.stringify(['photos', 1]))
      expect(listCalls()).toHaveLength(0)
      await vi.advanceTimersByTimeAsync(2000)
      expect(listCalls()).toHaveLength(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('analysis.progress updates the status cache (last event per session) and notifies', () => {
    const { qc } = setup()
    qc.setQueryData<AnalysisStatus>(qk.analysisStatus(1), { state: 'idle', profile: 'standard', done: 0, total: 0, stage: null, error: null })
    const onAnalysis = vi.fn()
    applyEvents(
      qc,
      [
        { type: 'analysis.progress', session_id: 1, state: 'running', stage: 'analyzing', done: 10, total: 100 },
        { type: 'analysis.progress', session_id: 1, state: 'running', stage: 'grouping', done: 100, total: 100 },
      ],
      undefined,
      { onAnalysis },
    )
    expect(qc.getQueryData<AnalysisStatus>(qk.analysisStatus(1))).toMatchObject({ state: 'running', stage: 'grouping', done: 100, total: 100, profile: 'standard' })
    expect(onAnalysis).toHaveBeenCalledTimes(1)
  })

  it('groups.updated / people.updated invalidate their queries; worker.status patches the hardware cache', () => {
    const { qc } = setup()
    qc.setQueryData<HardwareInfo>(qk.hardware, {
      worker: { state: 'stopped', tier: null, device: 'cuda:0', providers: [], gpu: { name: 'RTX', vram_mb: 1 }, error: null },
    })
    const spy = vi.spyOn(qc, 'invalidateQueries')
    applyEvents(qc, [
      { type: 'groups.updated', session_id: 1 },
      { type: 'people.updated', session_id: 1 },
      { type: 'worker.status', state: 'ready', tier: 'T3', error: null },
    ])
    const keys = spy.mock.calls.map((c) => JSON.stringify(c[0]?.queryKey))
    expect(keys).toContain(JSON.stringify(qk.groups(1)))
    expect(keys).toContain(JSON.stringify(qk.peopleAll))
    expect(qc.getQueryData<HardwareInfo>(qk.hardware)!.worker).toMatchObject({ state: 'ready', tier: 'T3', device: 'cuda:0' })
  })
})
