/**
 * Generative task orchestration (docs/api-contract-m5.md C/E): best take, inpaint and enhance all follow the same shape.
 *   POST -> 202 {task_id}  ->  `task.progress` events  ->  `*.done` + `edits.updated`
 * The server writes the patch layers into the photo's edit stack; the client reads the new stack back and records
 * ONE undo step (before = stack at request time, after = stack read back), so undo/redo go through the regular
 * PUT /api/edits path.
 */
import type { QueryClient } from '@tanstack/react-query'
import { ApiError } from '@/api/client'
import type { EditStack, ServerEvent } from '@/api/types'
import i18n from '@/i18n'
import { useAnalysisUi } from '@/stores/analysis'
import { useEdit } from '@/stores/edit'
import { isGenKind, useGen, type GenKind, type GenTask } from '@/stores/gen'
import { useToasts, type Toast } from '@/stores/toasts'
import { qk, type TaskEvent } from './cache'
import { emptyStack, stacksEqual } from './edit'
import { fetchStack, flushSaves, saveNow } from './editActions'
import { useHistory } from './history'
import { missingModelsOf } from './analysis'
import { mergePatches } from './patches'
import { CANCELLED, cancelTask } from './tasks'

export const GEN_TIMEOUT_MS = 180_000

const tr = (key: string, opts?: Record<string, unknown>) => i18n.t(key, opts) as string
const toast = (kind: Toast['kind'], text: string, ttl = 5000) => useToasts.getState().push(kind, text, ttl)

// ---------------------------------------------------------------- pure helpers (unit tested)

/**
 * Stack to keep after a task finished. When the user did not touch the open photo meanwhile the server's stack wins;
 * otherwise the client's newer sliders are kept and only the patch layers come from the server.
 */
export function resolveAfter(before: EditStack, server: EditStack, live: EditStack | null): EditStack {
  if (!live || stacksEqual(live, before)) return server
  return mergePatches(live, server)
}

/** `before` of the history entry for a finished task on `photoId`. */
export function beforeFor(task: Pick<GenTask, 'before' | 'snapshots' | 'photoId'>, photoId: number): EditStack {
  if (photoId === task.photoId) return task.before
  return task.snapshots?.[photoId] ?? emptyStack()
}

// ---------------------------------------------------------------- error UX (doc 04 section 8)

export interface ErrorCtx {
  sessionId: number
  /** called after the models were downloaded (consent dialog) */
  retry: () => void
}

/** 409 models_missing -> consent dialog then retry; 503 / timeouts / everything else -> toast. */
export function reportGenError(err: unknown, ctx: ErrorCtx): void {
  const missing = missingModelsOf(err)
  if (missing) {
    useAnalysisUi.getState().setConsent({ sessionId: ctx.sessionId, profile: 'fast', models: missing, onReady: ctx.retry })
    return
  }
  if (err instanceof ApiError && err.status === 503) {
    toast('error', tr('gen.workerUnavailable'))
    return
  }
  if (err instanceof ApiError && (err.status === 408 || err.status === 504)) {
    toast('error', tr('gen.timeout'))
    return
  }
  if (err instanceof TypeError) {
    toast('error', tr('gen.timeout'))
    return
  }
  toast('error', err instanceof Error ? err.message : String(err))
}

// ---------------------------------------------------------------- run

export interface GenSpec {
  kind: GenKind
  sessionId: number
  photoId: number
  label: string
  /** auto best take: snapshot the stacks of these photos (the real base is only known when the task finishes) */
  snapshotIds?: number[]
  baseFaceIds?: number[]
  start: () => Promise<{ task_id: string }>
  /** called once the server accepted the task (also when it was accepted after a consent-dialog retry) */
  onAccepted?: () => void
}

const watchdogs = new Map<string, ReturnType<typeof setTimeout>>()
/** Terminal events that arrived before `begin` registered the task (response/event race). */
const earlyTerminal = new Map<string, { ok: boolean; reason: string | null; at: number }>()
const earlyDone = new Map<string, { photoId: number; ok: boolean; reason: string | null; at: number }>()

/** Start a task. Returns true when the server accepted it. */
export async function runGen(qc: QueryClient, spec: GenSpec): Promise<boolean> {
  try {
    await flushSaves(qc)
    const st = useEdit.getState()
    const before = st.photoId === spec.photoId ? st.committed : await fetchStack(qc, spec.photoId)
    let snapshots: Record<number, EditStack> | undefined
    if (spec.snapshotIds?.length) {
      const list = await Promise.all(spec.snapshotIds.map((id) => (id === spec.photoId ? before : fetchStack(qc, id))))
      snapshots = Object.fromEntries(spec.snapshotIds.map((id, i) => [id, list[i]]))
    }
    const startedAt = Date.now()
    const { task_id } = await spec.start()
    const task: GenTask = {
      taskId: task_id,
      kind: spec.kind,
      photoId: spec.photoId,
      label: spec.label,
      before,
      snapshots,
      baseFaceIds: spec.baseFaceIds ?? [],
      done: 0,
      total: 0,
      state: 'running',
      startedAt,
    }
    useGen.getState().begin(task)
    spec.onAccepted?.()
    watchdogs.set(
      task_id,
      setTimeout(() => {
        watchdogs.delete(task_id)
        if (useGen.getState().remove(task_id)) toast('error', tr('gen.timeout'))
      }, GEN_TIMEOUT_MS),
    )
    // events that beat the HTTP response
    const term = earlyTerminal.get(task_id)
    const dn = [...earlyDone.entries()].find(([k, d]) => k.startsWith(`${spec.kind}:`) && d.at >= startedAt && d.photoId === spec.photoId)
    if (dn) earlyDone.delete(dn[0])
    if (term) void finalizeGen(qc, task_id, term.ok, term.reason)
    else if (dn) void finalizeGen(qc, task_id, dn[1].ok, dn[1].reason, dn[1].photoId)
    return true
  } catch (err) {
    reportGenError(err, { sessionId: spec.sessionId, retry: () => void runGen(qc, spec) })
    return false
  }
}

/** Read the new stack back, push the single undo step and refresh the open editor. */
export async function finalizeGen(qc: QueryClient, taskId: string, ok: boolean, reason: string | null, resultPhotoId?: number): Promise<void> {
  const task = useGen.getState().remove(taskId)
  if (!task) return
  const wd = watchdogs.get(taskId)
  if (wd) clearTimeout(wd)
  watchdogs.delete(taskId)
  earlyTerminal.delete(taskId)
  if (!ok && reason === CANCELLED) toast('info', tr('gen.cancelledToast', { label: task.label }), 3000)
  else if (!ok) toast('error', `${task.label}: ${reason ?? tr('gen.failed')}`)
  const photoId = resultPhotoId ?? task.photoId
  try {
    const server = await fetchStack(qc, photoId)
    const before = beforeFor(task, photoId)
    const st = useEdit.getState()
    const open = st.photoId === photoId
    const after = resolveAfter(before, server, open ? st.stack : null)
    if (!stacksEqual(before, after)) {
      useHistory.getState().push({ label: task.label, changes: [], edits: [{ photoId, before, after }] })
    }
    if (open) st.reset(photoId, after)
    if (!stacksEqual(after, server)) await saveNow(qc, photoId, after)
    void qc.invalidateQueries({ queryKey: ['bystanders', photoId] })
    if (task.kind === 'besttake' && ok && resultPhotoId !== undefined && resultPhotoId !== task.photoId) useGen.getState().setAutoBase(resultPhotoId)
  } catch (err) {
    toast('error', err instanceof Error ? err.message : String(err))
  }
}

// ---------------------------------------------------------------- events

/** `task.progress` of a generative task: progress bar, terminal states. */
export function genOnTask(qc: QueryClient, e: TaskEvent): void {
  if (!isGenKind(e.kind)) return
  const st = useGen.getState()
  const reason = e.state === 'cancelled' ? CANCELLED : (e.error ?? null)
  if (st.tasks[e.task_id]) {
    st.progress(e.task_id, e.done, e.total)
    if (e.state !== 'running') void finalizeGen(qc, e.task_id, e.state === 'done', reason)
  } else if (e.state !== 'running') {
    earlyTerminal.set(e.task_id, { ok: e.state === 'done', reason, at: Date.now() })
    if (earlyTerminal.size > 50) earlyTerminal.delete(earlyTerminal.keys().next().value as string)
  }
}

type DoneEvent = Extract<ServerEvent, { type: 'besttake.done' | 'inpaint.done' | 'enhance.done' }>

/** `besttake.done` / `inpaint.done` / `enhance.done`: per-person results, failure reasons, finalise the matching task. */
export function genOnDone(qc: QueryClient, ev: DoneEvent): void {
  const st = useGen.getState()
  let ok: boolean
  let reason: string | null
  let kind: GenKind
  let cancelled = false
  if (ev.type === 'besttake.done') {
    kind = 'besttake'
    // a cancelled task reports `cancelled` for every choice: no per-person outcome to show
    cancelled = ev.results.length > 0 && ev.results.every((r) => r.reason === CANCELLED)
    const failed = ev.results.filter((r) => !r.ok)
    ok = failed.length === 0
    reason = cancelled ? CANCELLED : (failed[0]?.reason ?? null)
    if (!cancelled) {
      st.setResults(ev.photo_id, ev.results)
      if (!ok) toast('error', tr('besttake.failedFor', { n: failed.length, reason: reason ? tr(`besttake.reason_${reason}`, { defaultValue: reason }) : tr('gen.failed') }))
      const warned = ev.results.filter((r) => r.ok && r.warnings.length > 0).length
      if (warned > 0) toast('info', tr('besttake.warnedToast', { n: warned }), 4500)
    }
  } else {
    kind = ev.type === 'inpaint.done' ? 'inpaint' : 'enhance'
    ok = ev.ok
    reason = ev.reason
  }
  const task = Object.values(st.tasks).find(
    (t) => t.kind === kind && (t.photoId === ev.photo_id || (t.kind === 'besttake' && t.snapshots !== undefined && ev.photo_id in t.snapshots)),
  )
  if (task) void finalizeGen(qc, task.taskId, kind === 'besttake' ? !cancelled : ok, kind === 'besttake' && !cancelled ? null : reason, ev.photo_id)
  else earlyDone.set(`${kind}:${ev.photo_id}`, { photoId: ev.photo_id, ok, reason, at: Date.now() })
  void qc.invalidateQueries({ queryKey: qk.editsAll })
}

// ---------------------------------------------------------------- cancel

/**
 * Stop a running generative task (`POST /api/tasks/{id}/cancel`). The task stays listed as "cancelling" until the
 * server reports `cancelled` (nothing is written into the stack then); when it already finished, its normal result wins.
 */
export async function cancelGen(qc: QueryClient, taskId: string): Promise<void> {
  const st = useGen.getState()
  if (!st.tasks[taskId] || st.tasks[taskId].cancelling) return
  st.setCancelling(taskId, true)
  // refused (already over, or the request failed): let the user try again while it is still listed
  if (!(await cancelTask(qc, taskId))) useGen.getState().setCancelling(taskId, false)
}
