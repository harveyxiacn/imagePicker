import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { AutoMode, EditSection, EditStack } from '@/api/types'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { patchPhotosInCache, qk } from './cache'
import { applySections, diffAdjust, emptyStack, getAdjust, isEmptyStack, mergeAuto, normalizeStack, stacksEqual } from './edit'
import { useHistory, type EditChange } from './history'

// ---------------------------------------------------------------- persistence (debounced PUT)

export const SAVE_DEBOUNCE_MS = 300

const timers = new Map<number, ReturnType<typeof setTimeout>>()
const pending = new Map<number, EditStack>()
const chains = new Map<number, Promise<void>>()

const setSaveState = (s: 'idle' | 'saving' | 'error') => useEdit.getState().setSaveState(s)

/** PUT one stack; writes to the same photo are serialised so an older request never lands last. */
function put(qc: QueryClient, photoId: number, stack: EditStack): Promise<void> {
  const prev = chains.get(photoId) ?? Promise.resolve()
  const next = prev
    .catch(() => undefined)
    .then(async () => {
      setSaveState('saving')
      try {
        const res = await api.putEdits(photoId, stack)
        qc.setQueryData(qk.edits(photoId), { photo_id: photoId, stack: res.stack, updated_at: res.updated_at })
        patchPhotosInCache(qc, new Map([[photoId, { has_edits: !isEmptyStack(res.stack), thumb_version: res.thumb_version }]]))
        if (chains.get(photoId) === next) setSaveState('idle')
      } catch (err) {
        setSaveState('error')
        useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
      }
    })
  chains.set(photoId, next)
  return next
}

/** Debounced save (slider commits, wheel ticks). */
export function scheduleSave(qc: QueryClient, photoId: number, stack: EditStack, ms = SAVE_DEBOUNCE_MS): void {
  pending.set(photoId, stack)
  const t = timers.get(photoId)
  if (t) clearTimeout(t)
  timers.set(
    photoId,
    setTimeout(() => {
      timers.delete(photoId)
      const s = pending.get(photoId)
      pending.delete(photoId)
      if (s) void put(qc, photoId, s)
    }, ms),
  )
  setSaveState('saving')
}

/** Cancel any debounced save and PUT immediately (undo/redo, paste). */
export function saveNow(qc: QueryClient, photoId: number, stack: EditStack): Promise<void> {
  const t = timers.get(photoId)
  if (t) clearTimeout(t)
  timers.delete(photoId)
  pending.delete(photoId)
  return put(qc, photoId, stack)
}

/** Run every debounced save now and wait for in-flight writes (leaving the page, sync). */
export async function flushSaves(qc: QueryClient): Promise<void> {
  for (const [id, t] of [...timers]) {
    clearTimeout(t)
    timers.delete(id)
    const s = pending.get(id)
    pending.delete(id)
    if (s) void put(qc, id, s)
  }
  await Promise.all([...chains.values()])
}

// ---------------------------------------------------------------- commits (history)

/** Live (uncommitted) update while a control is dragged: preview only, no history, no save. */
export function setLive(stack: EditStack): void {
  useEdit.getState().live(stack)
}

/**
 * Commit the live stack: push one undo step (`before` = stack at the last commit) and schedule the PUT.
 * Commits sharing `group` within ~1 s (arrow keys / wheel ticks on one slider) merge into one step.
 */
export function commitLive(qc: QueryClient, label: string, group?: string): boolean {
  const st = useEdit.getState()
  if (st.photoId === null) return false
  const photoId = st.photoId
  const before = st.committed
  const after = normalizeStack(st.stack)
  if (stacksEqual(before, after)) {
    useEdit.setState({ stack: before })
    return false
  }
  useHistory.getState().push({ label, changes: [], edits: [{ photoId, before, after }], group, at: Date.now() })
  useEdit.setState({ stack: after, committed: after })
  scheduleSave(qc, photoId, after)
  return true
}

/** Discrete change (preset, button, toggle): set live + commit in one go. */
export function changeStack(qc: QueryClient, next: EditStack, label: string, group?: string): boolean {
  useEdit.getState().live(next)
  return commitLive(qc, label, group)
}

/** Undo/redo/paste: write the `side` stacks of every change (PUT now) and refresh the open photo. */
export async function applyEditChanges(qc: QueryClient, edits: EditChange[], side: 'before' | 'after'): Promise<void> {
  const st = useEdit.getState()
  await Promise.all(
    edits.map((e) => {
      const stack = e[side]
      if (st.photoId === e.photoId) st.reset(e.photoId, stack)
      qc.setQueryData(qk.edits(e.photoId), { photo_id: e.photoId, stack, updated_at: Date.now() })
      patchPhotosInCache(qc, new Map([[e.photoId, { has_edits: !isEmptyStack(stack) }]]))
      return saveNow(qc, e.photoId, stack)
    }),
  )
}

export function resetAll(qc: QueryClient, label: string): boolean {
  return changeStack(qc, emptyStack(), label)
}

// ---------------------------------------------------------------- AI one-click

export async function autoApply(qc: QueryClient, mode: AutoMode, label: string): Promise<boolean> {
  const st = useEdit.getState()
  const photoId = st.photoId
  if (photoId === null) return false
  try {
    const { adjust } = await api.autoEdit(photoId, mode)
    if (useEdit.getState().photoId !== photoId) return false
    const cur = useEdit.getState().stack
    const next = mergeAuto(cur, adjust)
    const diff = diffAdjust(getAdjust(cur), getAdjust(next))
    const changed = changeStack(qc, next, label)
    const nonce = Date.now()
    useEdit.getState().setAutoApplied({ mode, diff, nonce })
    useEdit.getState().setFlash(diff.map((d) => d.key))
    setTimeout(() => {
      if (useEdit.getState().autoApplied?.nonce === nonce) useEdit.getState().setFlash([])
    }, 1800)
    return changed
  } catch (err) {
    useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
    return false
  }
}

// ---------------------------------------------------------------- copy / paste / sync

async function fetchStack(qc: QueryClient, id: number): Promise<EditStack> {
  const res = await api.edits(id)
  qc.setQueryData(qk.edits(id), res)
  return res.stack
}

/** Paste the copied sections onto `ids` as ONE undo step. Returns the number of changed photos. */
export async function pasteToPhotos(
  qc: QueryClient,
  ids: number[],
  clip: { stack: EditStack; sections: EditSection[] },
  label: string,
): Promise<number> {
  await flushSaves(qc)
  const edits: EditChange[] = []
  for (const id of ids) {
    const before = await fetchStack(qc, id)
    const after = applySections(before, clip.stack, clip.sections)
    if (!stacksEqual(before, after)) edits.push({ photoId: id, before, after })
  }
  if (!edits.length) return 0
  useHistory.getState().push({ label, changes: [], edits })
  await applyEditChanges(qc, edits, 'after')
  return edits.length
}

/** "Sync to group": server-side adaptive sync from `fromId`; before/after are read back for undo. */
export async function syncToPhotos(
  qc: QueryClient,
  fromId: number,
  toIds: number[],
  include: EditSection[],
  label: string,
): Promise<number> {
  if (!toIds.length) return 0
  await flushSaves(qc)
  const befores = await Promise.all(toIds.map((id) => fetchStack(qc, id)))
  const { updated } = await api.syncEdits({ from_id: fromId, to_ids: toIds, include, adaptive: true })
  const afters = await Promise.all(toIds.map((id) => fetchStack(qc, id)))
  const edits: EditChange[] = []
  toIds.forEach((id, i) => {
    if (!stacksEqual(befores[i], afters[i])) edits.push({ photoId: id, before: befores[i], after: afters[i] })
  })
  if (edits.length) {
    useHistory.getState().push({ label, changes: [], edits })
    const st = useEdit.getState()
    for (const e of edits) if (st.photoId === e.photoId) st.reset(e.photoId, e.after)
  }
  return updated
}
