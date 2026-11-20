import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import { useToasts } from '@/stores/toasts'
import i18n from '@/i18n'
import { planAcceptAi } from './ai'
import { canNow } from './auth'
import { applyEditChanges } from './editActions'
import { findCachedPhotos, patchPhotosInCache, qk } from './cache'
import {
  computeChanges,
  groupPatches,
  useHistory,
  type Change,
  type EditablePatch,
  type EditableKey,
  type HistoryEntry,
} from './history'
import { errorText } from '@/lib/errors'

/** Read-only guests (LAN mode) cannot change ratings / flags: tell them instead of failing with a 403. */
export function guestBlocked(): boolean {
  if (canNow('rate')) return false
  useToasts.getState().push('info', i18n.t('auth.readOnly') as string, 2500)
  return true
}

/** Per-photo tail of the PATCH chain: writes touching the same photo are sent one after the other, in call order. */
const chains = new Map<number, Promise<unknown>>()
/** Writes still on the wire (or queued behind another one). */
let pending = 0
/** A failed write, or a list refetch we cancelled, left the cache unsynced: refetch once everything has settled. */
let needsResync = false

/** Run `run` after every earlier write of any of `ids` has settled (whether it succeeded or not). */
function enqueue(ids: number[], run: () => Promise<unknown>): Promise<unknown> {
  const prev = Promise.all(ids.map((id) => chains.get(id)))
  const next = prev.then(run)
  const tail = next.then(
    () => undefined,
    () => undefined,
  )
  for (const id of ids) chains.set(id, tail)
  void tail.then(() => {
    for (const id of ids) if (chains.get(id) === tail) chains.delete(id)
  })
  return next
}

/**
 * Stop list refetches that are already in flight: their response was computed before this edit and would put the old
 * value back over the optimistic one. Only awaited when one is running, so the common case stays synchronous.
 */
async function cancelListRefetch(qc: QueryClient): Promise<void> {
  if (qc.isFetching({ queryKey: qk.photosAll }) === 0) return
  needsResync = true // the cancelled refetch carried data we no longer have; ask again after the write
  await qc.cancelQueries({ queryKey: qk.photosAll })
}

function settleWrite(qc: QueryClient): void {
  if (--pending === 0 && needsResync) {
    needsResync = false
    void qc.invalidateQueries({ queryKey: qk.photosAll })
  }
}

/** Roll back only photos whose cached value is still the one this write set; a newer edit keeps its value. */
function rollBack(qc: QueryClient, changes: Change[], side: 'before' | 'after'): void {
  const other = side === 'after' ? 'before' : 'after'
  const cached = new Map(findCachedPhotos(qc, changes.map((c) => c.id)).map((p) => [p.id, p]))
  const back = new Map<number, EditablePatch>()
  for (const c of changes) {
    const p = cached.get(c.id)
    if (!p) continue
    const mine = Object.entries(c[side]) as [EditableKey, unknown][]
    if (mine.every(([k, v]) => p[k] === v)) back.set(c.id, c[other])
  }
  patchPhotosInCache(qc, back)
}

/** Optimistically write `side` values of the changes to the cache, then PATCH the server (ordered per photo). */
async function commit(qc: QueryClient, changes: Change[], side: 'before' | 'after', auto = false): Promise<void> {
  pending++
  try {
    await cancelListRefetch(qc)
    patchPhotosInCache(qc, new Map(changes.map((c) => [c.id, c[side]])))
    const source = auto ? (side === 'before' ? 'undo' : 'auto') : undefined
    const byId = new Map(changes.map((c) => [c.id, c]))
    const results = await Promise.allSettled(
      groupPatches(changes, side).map((body) =>
        enqueue(body.ids, () => api.patchPhotos(source ? { ...body, source } : body)).catch((err) => {
          rollBack(qc, body.ids.map((id) => byId.get(id)!), side)
          throw err
        }),
      ),
    )
    const failed = results.find((r): r is PromiseRejectedResult => r.status === 'rejected')
    if (failed) {
      needsResync = true
      useToasts.getState().push('error', errorText(failed.reason))
    }
  } finally {
    settleWrite(qc)
  }
}

/** Edit rating / flag / label of photos: optimistic, undoable. */
export async function editPhotos(
  qc: QueryClient,
  ids: number[],
  patch: EditablePatch,
  label: string,
): Promise<void> {
  if (guestBlocked()) return
  const changes = computeChanges(findCachedPhotos(qc, ids), patch)
  if (changes.length === 0) return
  useHistory.getState().push({ label, changes })
  await commit(qc, changes, 'after')
}

/** Group / person entries call their own inverse requests; a failure is reported instead of thrown. */
async function runCustom(e: HistoryEntry, dir: 'undo' | 'redo'): Promise<void> {
  if (!e.custom) return
  try {
    await e.custom[dir]()
  } catch (err) {
    useToasts.getState().push('error', i18n.t('history.undoFailed', { reason: errorText(err) }) as string)
  }
}

export async function undo(qc: QueryClient): Promise<HistoryEntry | undefined> {
  const e = useHistory.getState().undo()
  if (e) {
    if (e.changes.length) await commit(qc, e.changes, 'before', e.auto)
    if (e.edits?.length) await applyEditChanges(qc, e.edits, 'before')
    await runCustom(e, 'undo')
  }
  return e
}

export async function redo(qc: QueryClient): Promise<HistoryEntry | undefined> {
  const e = useHistory.getState().redo()
  if (e) {
    if (e.changes.length) await commit(qc, e.changes, 'after', e.auto)
    if (e.edits?.length) await applyEditChanges(qc, e.edits, 'after')
    await runCustom(e, 'redo')
  }
  return e
}

/** Apply several different patches as ONE undo step (e.g. keep best + reject the rest). */
export async function editPhotoGroups(
  qc: QueryClient,
  groups: { ids: number[]; patch: EditablePatch }[],
  label: string,
): Promise<void> {
  if (guestBlocked()) return
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
  if (guestBlocked()) return 0
  const changes = planAcceptAi(findCachedPhotos(qc, ids))
  if (changes.length === 0) return 0
  useHistory.getState().push({ label, changes, auto: true })
  pending++
  try {
    await cancelListRefetch(qc)
    patchPhotosInCache(qc, new Map(changes.map((c) => [c.id, c.after])))
    await enqueue(
      changes.map((c) => c.id),
      () => api.acceptAi(changes.map((c) => c.id)),
    )
  } catch (err) {
    rollBack(qc, changes, 'after')
    needsResync = true
    useHistory.getState().drop(changes)
    useToasts.getState().push('error', errorText(err))
    return 0
  } finally {
    settleWrite(qc)
  }
  return changes.length
}
