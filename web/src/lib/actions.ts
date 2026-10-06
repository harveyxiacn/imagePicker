import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { useToasts } from '@/stores/toasts'
import { planAcceptAi } from './ai'
import { applyEditChanges } from './editActions'
import { findCachedPhotos, patchPhotosInCache, qk } from './cache'
import {
  computeChanges,
  groupPatches,
  useHistory,
  type Change,
  type EditablePatch,
  type HistoryEntry,
} from './history'

/** Optimistically write `side` values of the changes to the cache, then PATCH the server. */
async function commit(qc: QueryClient, changes: Change[], side: 'before' | 'after'): Promise<void> {
  const other = side === 'after' ? 'before' : 'after'
  patchPhotosInCache(qc, new Map(changes.map((c) => [c.id, c[side]])))
  try {
    await Promise.all(groupPatches(changes, side).map((body) => api.patchPhotos(body)))
  } catch (err) {
    // Roll back the optimistic write and resync.
    patchPhotosInCache(qc, new Map(changes.map((c) => [c.id, c[other]])))
    void qc.invalidateQueries({ queryKey: qk.photosAll })
    useToasts.getState().push('error', err instanceof Error ? err.message : String(err))
  }
}

/** Edit rating / flag / label of photos: optimistic, undoable. */
export async function editPhotos(
  qc: QueryClient,
  ids: number[],
  patch: EditablePatch,
  label: string,
): Promise<void> {
  const changes = computeChanges(findCachedPhotos(qc, ids), patch)
  if (changes.length === 0) return
  useHistory.getState().push({ label, changes })
  await commit(qc, changes, 'after')
}

export async function undo(qc: QueryClient): Promise<HistoryEntry | undefined> {
  const e = useHistory.getState().undo()
  if (e) {
    if (e.changes.length) await commit(qc, e.changes, 'before')
    if (e.edits?.length) await applyEditChanges(qc, e.edits, 'before')
  }
  return e
}

export async function redo(qc: QueryClient): Promise<HistoryEntry | undefined> {
  const e = useHistory.getState().redo()
  if (e) {
    if (e.changes.length) await commit(qc, e.changes, 'after')
    if (e.edits?.length) await applyEditChanges(qc, e.edits, 'after')
  }
  return e
}

/** Apply several different patches as ONE undo step (e.g. keep best + reject the rest). */
export async function editPhotoGroups(
  qc: QueryClient,
  groups: { ids: number[]; patch: EditablePatch }[],
  label: string,
): Promise<void> {
  const changes: Change[] = []
  for (const g of groups) changes.push(...computeChanges(findCachedPhotos(qc, g.ids), g.patch))
  if (changes.length === 0) return
  useHistory.getState().push({ label, changes })
  await commit(qc, changes, 'after')
}

/**
 * Accept AI ratings as user ratings (undoable via the shared history). The server write goes through
 * POST /api/photos/accept-ai; undo/redo use the regular PATCH path.
 */
export async function acceptAiRatings(qc: QueryClient, ids: number[], label: string): Promise<number> {
  const changes = planAcceptAi(findCachedPhotos(qc, ids))
  if (changes.length === 0) return 0
  useHistory.getState().push({ label, changes })
  patchPhotosInCache(qc, new Map(changes.map((c) => [c.id, c.after])))
  try {
    await api.acceptAi(changes.map((c) => c.id))
  } catch (err) {
    patchPhotosInCache(qc, new Map(changes.map((c) => [c.id, c.before])))
    void qc.invalidateQueries({ queryKey: qk.photosAll })
    useHistory.getState().drop(changes)
    useToasts.getState().push('error', err instanceof Error ? err.message : String(err))
    return 0
  }
  return changes.length
}
