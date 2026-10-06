/**
 * AI assistant (docs/api-contract-m6.md A): transcript reducer, plan helpers and the translation of the
 * `assistant.done.undo` payload into ONE history entry.
 */
import type { AssistantContext, AssistantPlan, AssistantResult, AssistantUndo, EditStack, PlanStep } from '@/api/types'
import { stacksEqual } from './edit'
import type { Change, EditablePatch, EditChange, HistoryEntry } from './history'

// ---------------------------------------------------------------- plan helpers

/** A plan made only of `filter` steps is applied immediately (no execute round trip). */
export function isFilterOnly(plan: AssistantPlan): boolean {
  return !plan.unsupported && plan.steps.length > 0 && plan.steps.every((s) => s.tool === 'filter')
}

export const hasDestructive = (plan: AssistantPlan): boolean => plan.steps.some((s) => s.destructive)

/** Execution needs an explicit confirmation when the server says so or any step is destructive. */
export const needsConfirm = (plan: AssistantPlan): boolean => plan.needs_confirmation || hasDestructive(plan)

/** Total number of photos touched by the plan (largest step: steps usually act on the same selection). */
export const affectedOf = (plan: AssistantPlan): number => plan.steps.reduce((n, s) => Math.max(n, s.affects), 0)

/** Serialises filter-tool args (the /api/photos query object) to a query string. */
export function argsToQuery(args: Record<string, unknown>): string {
  if (typeof args.query === 'string') return args.query.replace(/^\?/, '')
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(args)) {
    if (k === 'session_id' || k === 'collection' || v === null || v === undefined || v === false || v === '') continue
    if (v === true) p.set(k, '1')
    else if (Array.isArray(v)) {
      if (v.length) p.set(k, v.join(','))
    } else if (typeof v === 'string' || typeof v === 'number') p.set(k, String(v))
  }
  return p.toString()
}

export const filterQueryOf = (step: PlanStep): string => argsToQuery(step.args)

/** The `selection` arg of a step, normalised. */
export type StepSelection = { kind: 'ids'; ids: number[] } | { kind: 'query'; query: string } | { kind: 'current_filter' } | null
export function selectionOf(step: PlanStep): StepSelection {
  const sel = step.args.selection
  if (sel === 'current_filter') return { kind: 'current_filter' }
  if (sel && typeof sel === 'object') {
    const o = sel as { ids?: unknown; query?: unknown }
    if (Array.isArray(o.ids)) return { kind: 'ids', ids: o.ids.filter((x): x is number => typeof x === 'number') }
    if (typeof o.query === 'string') return { kind: 'query', query: o.query }
  }
  return null
}

export interface PreviewAction {
  /** apply as the library filter */
  query?: string
  /** highlight (select) these photos */
  ids?: number[]
}

/** "预览影响": show what the plan would touch — apply its filter step, else highlight the affected photos. */
export function previewOf(plan: AssistantPlan): PreviewAction | null {
  const f = plan.steps.find((s) => s.tool === 'filter')
  if (f) return { query: filterQueryOf(f) }
  for (const s of plan.steps) {
    const sel = selectionOf(s)
    if (sel?.kind === 'ids' && sel.ids.length) return { ids: sel.ids }
    if (sel?.kind === 'query') return { query: sel.query }
  }
  return null
}

export function buildContext(input: {
  filter: string
  selection: Iterable<number>
  currentPhotoId: number | null
  locale: string
}): AssistantContext {
  return {
    filter: input.filter,
    selection: [...input.selection],
    current_photo_id: input.currentPhotoId,
    locale: input.locale === 'en' ? 'en' : 'zh-CN',
  }
}

export function summarizeResults(results: AssistantResult[]): { ok: number; failed: number; affected: number; errors: string[] } {
  let ok = 0
  let failed = 0
  let affected = 0
  const errors: string[] = []
  for (const r of results) {
    if (r.ok) {
      ok++
      affected += r.affected
    } else {
      failed++
      if (r.error) errors.push(r.error)
    }
  }
  return { ok, failed, affected, errors }
}

// ---------------------------------------------------------------- undo payload -> history entry

/**
 * One undo step for a whole execution. `undo` holds the values BEFORE; the AFTER values are read back by the
 * caller (photos / edit stacks) so redo works. Photos / stacks that did not change are skipped. Returns null
 * when nothing changed.
 */
export function undoToEntry(
  label: string,
  undo: AssistantUndo | undefined,
  after: { photos: Map<number, EditablePatch>; stacks: Map<number, EditStack> },
): HistoryEntry | null {
  const changes: Change[] = []
  for (const b of undo?.photos ?? []) {
    const a = after.photos.get(b.id)
    if (!a) continue
    const before: Record<string, unknown> = {}
    const next: Record<string, unknown> = {}
    for (const k of ['user_rating', 'flag', 'color_label'] as const) {
      if (k in a && a[k] !== b[k]) {
        before[k] = b[k]
        next[k] = a[k]
      }
    }
    if (Object.keys(next).length) changes.push({ id: b.id, before: before as EditablePatch, after: next as EditablePatch })
  }
  const edits: EditChange[] = []
  for (const b of undo?.edits ?? []) {
    const a = after.stacks.get(b.photo_id)
    if (a && !stacksEqual(b.before, a)) edits.push({ photoId: b.photo_id, before: b.before, after: a })
  }
  if (!changes.length && !edits.length) return null
  return { label, changes, ...(edits.length ? { edits } : {}) }
}

// ---------------------------------------------------------------- transcript reducer

export type PlanPhase = 'planned' | 'applied' | 'executing' | 'done' | 'failed' | 'dismissed'

export type TranscriptItem =
  | { id: number; kind: 'user'; text: string }
  | { id: number; kind: 'text'; text: string }
  | { id: number; kind: 'error'; text: string }
  | {
      id: number
      kind: 'plan'
      plan: AssistantPlan
      phase: PlanPhase
      taskId?: string
      progress?: { done: number; total: number }
      results?: AssistantResult[]
      ok?: boolean
      /** history entry pushed for this execution (undo button only while it is the newest step) */
      undoable?: boolean
      undone?: boolean
      error?: string
    }

export interface AssistantState {
  items: TranscriptItem[]
  planning: boolean
  seq: number
}

export const initialAssistantState: AssistantState = { items: [], planning: false, seq: 0 }

export type AssistantAction =
  | { type: 'send'; text: string }
  | { type: 'plan'; plan: AssistantPlan }
  | { type: 'planFailed'; message: string }
  | { type: 'executeStarted'; planId: string; taskId: string }
  | { type: 'executeFailed'; planId: string; message: string }
  | { type: 'progress'; taskId: string; done: number; total: number }
  | { type: 'done'; planId: string; ok: boolean; results: AssistantResult[]; undoable: boolean }
  | { type: 'undone'; planId: string }
  | { type: 'dismiss'; planId: string }
  | { type: 'applied'; planId: string }
  | { type: 'clear' }

const mapPlan = (items: TranscriptItem[], planId: string, f: (it: Extract<TranscriptItem, { kind: 'plan' }>) => TranscriptItem): TranscriptItem[] =>
  items.map((it) => (it.kind === 'plan' && it.plan.plan_id === planId ? f(it) : it))

export function assistantReducer(s: AssistantState, a: AssistantAction): AssistantState {
  const id = s.seq + 1
  switch (a.type) {
    case 'send':
      return { ...s, seq: id, planning: true, items: [...s.items, { id, kind: 'user', text: a.text }] }
    case 'plan': {
      const items: TranscriptItem[] = [...s.items]
      if (a.plan.reply) items.push({ id, kind: 'text', text: a.plan.reply })
      let seq = id
      if (!a.plan.unsupported && a.plan.steps.length) {
        seq++
        items.push({ id: seq, kind: 'plan', plan: a.plan, phase: isFilterOnly(a.plan) ? 'applied' : 'planned' })
      } else if (a.plan.unsupported) {
        seq++
        items.push({ id: seq, kind: 'plan', plan: a.plan, phase: 'dismissed' })
      }
      return { ...s, seq, planning: false, items }
    }
    case 'planFailed':
      return { ...s, seq: id, planning: false, items: [...s.items, { id, kind: 'error', text: a.message }] }
    case 'executeStarted':
      return { ...s, items: mapPlan(s.items, a.planId, (it) => (it.phase === 'done' ? it : { ...it, phase: 'executing', taskId: a.taskId, progress: { done: 0, total: 0 }, error: undefined })) }
    case 'executeFailed':
      return { ...s, items: mapPlan(s.items, a.planId, (it) => ({ ...it, phase: 'failed', error: a.message })) }
    case 'progress':
      return {
        ...s,
        items: s.items.map((it) =>
          it.kind === 'plan' && it.taskId === a.taskId && it.phase === 'executing' ? { ...it, progress: { done: a.done, total: a.total } } : it,
        ),
      }
    case 'done':
      return {
        ...s,
        items: mapPlan(s.items, a.planId, (it) => ({
          ...it,
          phase: a.ok ? 'done' : 'failed',
          ok: a.ok,
          results: a.results,
          undoable: a.undoable,
          error: a.ok ? undefined : (a.results.find((r) => !r.ok)?.error ?? undefined),
        })),
      }
    case 'undone':
      return { ...s, items: mapPlan(s.items, a.planId, (it) => ({ ...it, undone: true, undoable: false })) }
    case 'dismiss':
      return { ...s, items: mapPlan(s.items, a.planId, (it) => ({ ...it, phase: 'dismissed' })) }
    case 'applied':
      return { ...s, items: mapPlan(s.items, a.planId, (it) => ({ ...it, phase: 'applied' })) }
    case 'clear':
      return { ...initialAssistantState, seq: s.seq }
  }
}

/** Latest plan card still waiting for the user (used to disable the composer's execute shortcuts). */
export const activePlan = (s: AssistantState): Extract<TranscriptItem, { kind: 'plan' }> | undefined =>
  [...s.items].reverse().find((it): it is Extract<TranscriptItem, { kind: 'plan' }> => it.kind === 'plan' && (it.phase === 'planned' || it.phase === 'executing'))
