import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { AnalysisStatus, HardwareInfo, Photo, RuntimeInfo, ServerEvent, Session, Settings, Taste } from '@/api/types'
import type { PhotosListQuery } from './filter'

export const qk = {
  sessions: ['sessions'] as const,
  session: (id: number) => ['session', id] as const,
  photosAll: ['photos'] as const,
  photos: (sessionId: number, q: PhotosListQuery) => ['photos', sessionId, q] as const,
  hardware: ['hardware'] as const,
  runtime: ['runtime'] as const,
  models: ['models'] as const,
  analysisStatus: (sid: number) => ['analysis', 'status', sid] as const,
  analysis: (photoId: number) => ['analysis', 'photo', photoId] as const,
  groups: (sid: number) => ['groups', sid] as const,
  editsAll: ['edits'] as const,
  edits: (photoId: number) => ['edits', photoId] as const,
  presets: ['presets'] as const,
  luts: ['luts'] as const,
  mask: (photoId: number, target: string, personId?: number) => ['mask', photoId, target, personId ?? null] as const,
  burstFaces: (burstId: number) => ['burstFaces', burstId] as const,
  devices: (sid: number) => ['devices', sid] as const,
  peopleAll: ['people'] as const,
  people: (sid: number | undefined) => ['people', sid ?? 'all'] as const,
  photoPeople: (photoId: number) => ['photoPeople', photoId] as const,
  photoPeopleAll: ['photoPeople'] as const,
  collections: ['collections'] as const,
  collectionCount: (sid: number, query: string) => ['collectionCount', sid, query] as const,
  taste: ['taste'] as const,
  bystanders: (photoId: number) => ['bystanders', photoId] as const,
  besttake: (burstId: number) => ['besttake', burstId] as const,
  best: (sid: number, ids: number[], n: number) => ['best', sid, ids, n] as const,
  settings: ['settings'] as const,
  me: ['auth', 'me'] as const,
  assistantStatus: ['assistant', 'status'] as const,
  cacheInfo: ['cache'] as const,
  onboarding: ['onboarding'] as const,
  lan: ['lan'] as const,
  remoteStatus: ['remote', 'status'] as const,
  remoteDevices: ['remote', 'devices'] as const,
}

/** Photo fields owned by the analysis pipeline (never user-editable, safe to overwrite from the server). */
export const AI_FIELDS = [
  'ai_score',
  'ai_rating',
  'issues',
  'burst_id',
  'rank_in_burst',
  'burst_size',
  'scene_type',
  'face_count',
  'subject_face_count',
  'analyzed',
  'thumb_ready',
  'thumb_version',
] as const satisfies readonly (keyof Photo)[]

export function aiPatchOf(p: Photo): PhotoPatch {
  const out: Record<string, unknown> = {}
  for (const k of AI_FIELDS) out[k] = p[k]
  return out as PhotoPatch
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

export interface ApplyDeps {
  /** Fetch one photo (defaults to the API); injectable for tests. */
  fetchPhoto?: (id: number) => Promise<Photo>
  onAnalysis?: (e: Extract<ServerEvent, { type: 'analysis.progress' }>) => void
  /** M6: assistant.done / xmp.conflict */
  onAssistant?: (e: Extract<ServerEvent, { type: 'assistant.done' }>) => void
  onXmpConflict?: (e: Extract<ServerEvent, { type: 'xmp.conflict' }>) => void
  /** M5: besttake.done / inpaint.done / enhance.done */
  onGen?: (e: Extract<ServerEvent, { type: 'besttake.done' | 'inpaint.done' | 'enhance.done' }>) => void
}

/** Up to this many `analysis.updated` ids are re-fetched individually; more triggers a throttled list refetch. */
export const ANALYSIS_DIRECT_FETCH_MAX = 40
const LIST_REFETCH_MS = 1500
const listTimers = new Map<number, ReturnType<typeof setTimeout>>()

/** At most one background list refetch per session per window (trailing), so streaming results never thrash the grid. */
function scheduleListRefetch(qc: QueryClient, sid: number): void {
  if (listTimers.has(sid)) return
  listTimers.set(
    sid,
    setTimeout(() => {
      listTimers.delete(sid)
      void qc.invalidateQueries({ queryKey: ['photos', sid] })
    }, LIST_REFETCH_MS),
  )
}

async function refetchAnalysed(qc: QueryClient, ids: number[], deps: ApplyDeps): Promise<void> {
  const fetchPhoto = deps.fetchPhoto ?? ((id: number) => api.photo(id).then((r) => r.photo))
  const patches = new Map<number, PhotoPatch>()
  for (let i = 0; i < ids.length; i += 8) {
    const batch = await Promise.all(ids.slice(i, i + 8).map((id) => fetchPhoto(id).catch(() => null)))
    for (const p of batch) if (p) patches.set(p.id, aiPatchOf(p))
  }
  patchPhotosInCache(qc, patches)
}

export interface ApplyResult {
  patchedIds: number
  /** Distinct photo ids named by analysis.updated, per session. */
  analysisIds: Record<number, number[]>
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
  deps: ApplyDeps = {},
): ApplyResult {
  const patches = new Map<number, PhotoPatch>()
  const added = new Set<number>()
  const sessions = new Map<number, Session>()
  const tasks = new Map<string, TaskEvent>()
  const analysisIds = new Map<number, Set<number>>()
  const progress = new Map<number, Extract<ServerEvent, { type: 'analysis.progress' }>>()
  const groupsChanged = new Set<number>()
  const editedIds = new Set<number>()
  let peopleChanged = false
  let collectionsChanged = false
  const beautyReady = new Set<number>()
  const genDone: Extract<ServerEvent, { type: 'besttake.done' | 'inpaint.done' | 'enhance.done' }>[] = []
  const assistantDone: Extract<ServerEvent, { type: 'assistant.done' }>[] = []
  const xmpConflicts: Extract<ServerEvent, { type: 'xmp.conflict' }>[] = []
  let settings: Settings | null = null
  let taste: Extract<ServerEvent, { type: 'taste.updated' }> | null = null
  let worker: Extract<ServerEvent, { type: 'worker.status' }> | null = null

  for (const ev of events) {
    switch (ev.type) {
      case 'photos.updated':
        for (const { id, ...rest } of ev.items) patches.set(id, { ...patches.get(id), ...rest })
        break
      case 'thumbs.ready':
        for (const { id, v } of ev.items)
          patches.set(id, { ...patches.get(id), thumb_ready: true, thumb_version: v })
        break
      case 'edits.updated':
        // Merge by id, latest wins (contract C). Thumbs re-fetch automatically through the new thumb_version.
        for (const { id, has_edits, thumb_version } of ev.items) {
          patches.set(id, { ...patches.get(id), has_edits, thumb_version })
          editedIds.add(id)
        }
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
      case 'analysis.updated': {
        const set = analysisIds.get(ev.session_id) ?? new Set<number>()
        for (const id of ev.ids) set.add(id)
        analysisIds.set(ev.session_id, set)
        break
      }
      case 'analysis.progress':
        progress.set(ev.session_id, ev)
        break
      case 'groups.updated':
        groupsChanged.add(ev.session_id)
        break
      case 'people.updated':
        peopleChanged = true
        break
      case 'worker.status':
        worker = ev
        break
      case 'besttake.done':
      case 'inpaint.done':
      case 'enhance.done':
        genDone.push(ev)
        break
      case 'beauty.ready':
        beautyReady.add(ev.photo_id)
        break
      case 'taste.updated':
        taste = ev
        break
      case 'collections.updated':
        collectionsChanged = true
        break
      case 'assistant.done':
        assistantDone.push(ev)
        break
      case 'settings.updated':
        settings = ev.settings
        break
      case 'xmp.conflict':
        xmpConflicts.push(ev)
        break
      case 'runtime.updated': {
        const prev = qc.getQueryData<RuntimeInfo>(qk.runtime)
        // keep the hardware part of the last full GET; the event carries the install state only
        qc.setQueryData<RuntimeInfo>(qk.runtime, { ...prev, ...ev.runtime })
        if (prev?.state !== ev.runtime.state) {
          // the worker restarts on another interpreter: hardware / models are stale
          void qc.invalidateQueries({ queryKey: qk.hardware })
          void qc.invalidateQueries({ queryKey: qk.models })
        }
        break
      }
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

  for (const [sid, ids] of analysisIds) {
    const list = [...ids]
    if (list.length <= ANALYSIS_DIRECT_FETCH_MAX) void refetchAnalysed(qc, list, deps)
    else scheduleListRefetch(qc, sid)
    const set = ids
    void qc.invalidateQueries({
      predicate: (q) => q.queryKey[0] === 'analysis' && q.queryKey[1] === 'photo' && set.has(q.queryKey[2] as number),
    })
  }

  for (const [sid, ev] of progress) {
    qc.setQueryData<AnalysisStatus>(qk.analysisStatus(sid), (old) => ({
      profile: old?.profile ?? null,
      error: null,
      ...old,
      state: ev.state,
      stage: ev.stage,
      done: ev.done,
      total: ev.total,
    }))
    deps.onAnalysis?.(ev)
  }

  for (const sid of groupsChanged) {
    void qc.invalidateQueries({ queryKey: qk.groups(sid) })
    void qc.invalidateQueries({ queryKey: ['burstFaces'] })
    scheduleListRefetch(qc, sid)
  }
  if (editedIds.size) {
    // Cached stacks of other photos (paste / sync) are stale now; the open photo keeps its live state.
    void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === 'edits' && editedIds.has(q.queryKey[1] as number) })
  }
  if (peopleChanged) void qc.invalidateQueries({ queryKey: qk.peopleAll })
  for (const id of beautyReady) void qc.invalidateQueries({ queryKey: qk.photoPeople(id) })
  if (collectionsChanged) {
    void qc.invalidateQueries({ queryKey: qk.collections })
    void qc.invalidateQueries({ queryKey: ['collectionCount'] })
  }
  if (taste) {
    const t = taste
    // Merge the pushed numbers right away; traits / accuracy come with the refetch.
    qc.setQueryData<Taste>(qk.taste, (old) => (old ? { ...old, labels: t.labels, active: t.active, alpha: t.alpha } : old))
    void qc.invalidateQueries({ queryKey: qk.taste })
  }

  if (worker) {
    const w = worker
    qc.setQueryData<HardwareInfo>(qk.hardware, (old) =>
      old ? { worker: { ...old.worker, state: w.state, tier: w.tier ?? old.worker.tier, error: w.error } } : old,
    )
  }

  if (settings) {
    qc.setQueryData(qk.settings, settings)
    // models.source / lan / cache changes affect other panels
    void qc.invalidateQueries({ queryKey: qk.lan })
  }
  if (deps.onAssistant) for (const a of assistantDone) deps.onAssistant(a)
  if (deps.onXmpConflict) for (const c of xmpConflicts) deps.onXmpConflict(c)
  if (deps.onGen) for (const g of genDone) deps.onGen(g)
  if (onTask) for (const t of tasks.values()) onTask(t)

  return {
    patchedIds: patches.size,
    analysisIds: Object.fromEntries([...analysisIds].map(([sid, ids]) => [sid, [...ids]])),
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
