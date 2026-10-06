/** Side-effectful glue of the AI assistant: plan / execute requests, assistant.done handling, undo, edit suggestions. */
import type { QueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import type { AssistantPlan, EditSuggestion, ServerEvent } from '@/api/types'
import i18n from '@/i18n'
import { useAssistant } from '@/stores/assistant'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { undo } from './actions'
import {
  buildContext,
  filterQueryOf,
  isFilterOnly,
  previewOf,
  summarizeResults,
  undoToEntry,
  type PreviewAction,
} from './assistant'
import { patchPhotosInCache, qk } from './cache'
import { filterToQuery, queryToFilter } from './collections'
import { changeStack, fetchStack } from './editActions'
import { mergeAuto } from './edit'
import { useHistory, type EditablePatch, type HistoryEntry } from './history'

type DoneEvent = Extract<ServerEvent, { type: 'assistant.done' }>

const t = (k: string, o?: Record<string, unknown>) => i18n.t(k, o) as string
const errText = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** history entry created per executed plan (for the "undo" button: only valid while it is the newest step) */
const entries = new Map<string, HistoryEntry>()
/** one-shot listeners for plans run outside the transcript (edit suggestions) */
const waiters = new Map<string, (e: DoneEvent) => void>()

export const entryOf = (planId: string): HistoryEntry | undefined => entries.get(planId)
export const isTopEntry = (planId: string): boolean => {
  const e = entries.get(planId)
  const stack = useHistory.getState().undoStack
  return !!e && stack[stack.length - 1] === e
}

const D = () => useAssistant.getState().dispatch

/** Apply a query string as the library filter (the assistant's `filter` tool). */
export function applyFilterQuery(query: string): void {
  useUi.getState().setFilter(queryToFilter(query))
}

export function applyPreview(p: PreviewAction | null): void {
  if (!p) return
  if (p.query !== undefined) applyFilterQuery(p.query)
  if (p.ids?.length) useUi.getState().setSelection({ ids: new Set(p.ids), anchorId: p.ids[0] })
}

function contextNow(photoId: number | null) {
  const ui = useUi.getState()
  return buildContext({
    filter: filterToQuery(ui.filter),
    selection: ui.selection.ids,
    currentPhotoId: photoId ?? ui.activeId,
    locale: i18n.language,
  })
}

/** Chat message -> plan. Filter-only plans are applied at once. */
export async function sendMessage(sessionId: number, text: string, photoId: number | null = null): Promise<AssistantPlan | null> {
  const message = text.trim()
  if (!message) return null
  D()({ type: 'send', text: message })
  try {
    const plan = await api.assistantPlan({ session_id: sessionId, message, context: contextNow(photoId) })
    D()({ type: 'plan', plan })
    if (isFilterOnly(plan)) applyFilterQuery(filterQueryOf(plan.steps.find((s) => s.tool === 'filter')!))
    return plan
  } catch (e) {
    D()({ type: 'planFailed', message: errText(e) })
    return null
  }
}

/** Execute a stored plan: the `filter` steps apply client-side, the rest run on the server (progress + assistant.done). */
export async function executePlan(plan: AssistantPlan): Promise<boolean> {
  const filterStep = plan.steps.find((s) => s.tool === 'filter')
  if (filterStep) applyFilterQuery(filterQueryOf(filterStep))
  if (plan.steps.every((s) => s.tool === 'filter')) {
    D()({ type: 'applied', planId: plan.plan_id })
    return true
  }
  try {
    const { task_id } = await api.assistantExecute(plan.plan_id)
    D()({ type: 'executeStarted', planId: plan.plan_id, taskId: task_id })
    return true
  } catch (e) {
    D()({ type: 'executeFailed', planId: plan.plan_id, message: errText(e) })
    return false
  }
}

export const preview = (plan: AssistantPlan): void => applyPreview(previewOf(plan))

const chunk = async <T, R>(items: T[], size: number, f: (x: T) => Promise<R>): Promise<R[]> => {
  const out: R[] = []
  for (let i = 0; i < items.length; i += size) out.push(...(await Promise.all(items.slice(i, i + size).map(f))))
  return out
}

/** `assistant.done`: read the AFTER values back, then record the whole execution as ONE undo step. */
export async function onAssistantDone(qc: QueryClient, ev: DoneEvent): Promise<void> {
  const waiter = waiters.get(ev.plan_id)
  if (waiter) {
    waiters.delete(ev.plan_id)
    waiter(ev)
    return
  }
  const item = useAssistant.getState().items.find((i) => i.kind === 'plan' && i.plan.plan_id === ev.plan_id)
  const label = t('assistant.historyLabel', { text: item && item.kind === 'plan' ? item.plan.reply || item.plan.steps[0]?.summary || '' : '' })

  const photos = new Map<number, EditablePatch>()
  const stacks = new Map<number, Awaited<ReturnType<typeof fetchStack>>>()
  try {
    const got = await chunk(ev.undo?.photos ?? [], 8, (p) =>
      api
        .photo(p.id)
        .then((r) => r.photo)
        .catch(() => null),
    )
    for (const p of got) if (p) photos.set(p.id, { user_rating: p.user_rating, flag: p.flag, color_label: p.color_label })
    patchPhotosInCache(qc, new Map([...photos].map(([id, v]) => [id, v])))
    const ids = (ev.undo?.edits ?? []).map((e) => e.photo_id)
    await chunk(ids, 8, async (id) => stacks.set(id, await fetchStack(qc, id)).get(id))
  } catch (e) {
    useToasts.getState().push('error', errText(e), 5000)
  }
  const entry = undoToEntry(label, ev.undo, { photos, stacks })
  if (entry) {
    useHistory.getState().push(entry)
    entries.set(ev.plan_id, entry)
    const st = useEdit.getState()
    for (const c of entry.edits ?? []) if (st.photoId === c.photoId) st.reset(c.photoId, c.after)
  }
  void qc.invalidateQueries({ queryKey: qk.photosAll })
  void qc.invalidateQueries({ queryKey: ['groups'] })
  D()({ type: 'done', planId: ev.plan_id, ok: ev.ok, results: ev.results, undoable: !!entry })
  const sum = summarizeResults(ev.results)
  useToasts.getState().push(ev.ok ? 'success' : 'error', ev.ok ? t('assistant.doneToast', { n: sum.affected }) : (sum.errors[0] ?? t('assistant.failed')), 3500)
}

/** Undo the whole execution (one history step). Only while it is still the newest step. */
export async function undoPlan(qc: QueryClient, planId: string): Promise<boolean> {
  if (!isTopEntry(planId)) {
    useToasts.getState().push('info', t('assistant.undoBlocked'), 3500)
    return false
  }
  await undo(qc)
  entries.delete(planId)
  D()({ type: 'undone', planId })
  return true
}

// ---------------------------------------------------------------- edit suggestions (suggest_edits)

const SUGGEST_TIMEOUT_MS = 20_000

/** Ask the VLM for edit suggestions of one photo through the same plan/execute path (nothing is saved). */
export async function requestSuggestion(sessionId: number, photoId: number): Promise<EditSuggestion> {
  const plan = await api.assistantPlan({
    session_id: sessionId,
    message: i18n.language === 'en' ? 'suggest edits for this photo' : '给这张照片一些修图建议',
    context: contextNow(photoId),
  })
  const step = plan.steps.find((s) => s.tool === 'suggest_edits')
  if (!step) throw new Error(plan.unsupported ?? t('assistant.suggestUnavailable'))
  const done = new Promise<DoneEvent>((resolve, reject) => {
    const timer = setTimeout(() => {
      waiters.delete(plan.plan_id)
      reject(new Error(t('assistant.suggestTimeout')))
    }, SUGGEST_TIMEOUT_MS)
    waiters.set(plan.plan_id, (e) => {
      clearTimeout(timer)
      resolve(e)
    })
  })
  try {
    await api.assistantExecute(plan.plan_id)
  } catch (e) {
    waiters.delete(plan.plan_id)
    throw e
  }
  const ev = await done
  const r = ev.results.find((x) => x.tool === 'suggest_edits')
  if (!r?.ok) throw new Error(r?.error ?? t('assistant.suggestUnavailable'))
  const data = r.data as EditSuggestion | undefined
  if (!data?.adjust) throw new Error(t('assistant.suggestUnavailable'))
  return data
}

/** One-click apply of a suggestion to the open photo as an undoable change. */
export function applySuggestion(qc: QueryClient, s: EditSuggestion): boolean {
  const st = useEdit.getState()
  if (st.photoId === null) return false
  return changeStack(qc, mergeAuto(st.stack, { ...s.adjust, source: 'ai_suggest@1' }), t('assistant.suggestHistory'))
}
