import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { useToasts } from '@/stores/toasts'
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
  if (e) await commit(qc, e.changes, 'before')
  return e
}

export async function redo(qc: QueryClient): Promise<HistoryEntry | undefined> {
  const e = useHistory.getState().redo()
  if (e) await commit(qc, e.changes, 'after')
  return e
}
