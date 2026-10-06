import { describe, expect, it } from 'vitest'
import type { AssistantPlan, EditStack, PlanStep } from '@/api/types'
import { emptyStack } from './edit'
import {
  argsToQuery,
  assistantReducer,
  buildContext,
  initialAssistantState,
  isFilterOnly,
  needsConfirm,
  previewOf,
  summarizeResults,
  undoToEntry,
  type AssistantState,
} from './assistant'
import { useHistory } from './history'

const step = (tool: string, args: Record<string, unknown> = {}, extra: Partial<PlanStep> = {}): PlanStep => ({
  tool,
  args,
  summary: tool,
  affects: 3,
  destructive: false,
  ...extra,
})
const plan = (steps: PlanStep[], extra: Partial<AssistantPlan> = {}): AssistantPlan => ({
  plan_id: 'p1',
  reply: 'ok',
  steps,
  needs_confirmation: false,
  engine: 'rules',
  unsupported: null,
  ...extra,
})
const stackWith = (exposure: number): EditStack => ({ version: 1, ops: [{ type: 'global', exposure }] })

describe('plan helpers', () => {
  it('detects filter-only plans and destructive confirmation', () => {
    expect(isFilterOnly(plan([step('filter', { flag: 'picked' })]))).toBe(true)
    expect(isFilterOnly(plan([step('filter'), step('set_flag')]))).toBe(false)
    expect(isFilterOnly(plan([]))).toBe(false)
    expect(isFilterOnly(plan([step('filter')], { unsupported: 'no' }))).toBe(false)
    expect(needsConfirm(plan([step('set_flag', {}, { destructive: true })]))).toBe(true)
    expect(needsConfirm(plan([step('set_flag')], { needs_confirmation: true }))).toBe(true)
    expect(needsConfirm(plan([step('set_flag')]))).toBe(false)
  })

  it('serialises filter args like /api/photos', () => {
    expect(argsToQuery({ rating_gte: 4, flag: 'picked', issues_any: ['blurry', 'noisy'], burst_best_only: true, session_id: 9, x: null })).toBe(
      'rating_gte=4&flag=picked&issues_any=blurry%2Cnoisy&burst_best_only=1',
    )
    expect(argsToQuery({ query: '?flag=picked' })).toBe('flag=picked')
  })

  it('previews through the filter step, else the affected ids', () => {
    expect(previewOf(plan([step('filter', { flag: 'picked' })]))).toEqual({ query: 'flag=picked' })
    expect(previewOf(plan([step('set_flag', { selection: { ids: [1, 2] } })]))).toEqual({ ids: [1, 2] })
    expect(previewOf(plan([step('set_flag', { selection: { query: 'rating_gte=5' } })]))).toEqual({ query: 'rating_gte=5' })
    expect(previewOf(plan([step('set_flag', { selection: 'current_filter' })]))).toBeNull()
  })

  it('builds the request context', () => {
    expect(buildContext({ filter: 'a=1', selection: new Set([3, 4]), currentPhotoId: 7, locale: 'en' })).toEqual({
      filter: 'a=1',
      selection: [3, 4],
      current_photo_id: 7,
      locale: 'en',
    })
    expect(buildContext({ filter: '', selection: [], currentPhotoId: null, locale: 'zh' }).locale).toBe('zh-CN')
  })

  it('summarises results', () => {
    expect(summarizeResults([{ tool: 'a', ok: true, affected: 4 }, { tool: 'b', ok: false, affected: 0, error: 'boom' }])).toEqual({
      ok: 1,
      failed: 1,
      affected: 4,
      errors: ['boom'],
    })
  })
})

describe('assistant reducer', () => {
  const run = (...actions: Parameters<typeof assistantReducer>[1][]): AssistantState => actions.reduce(assistantReducer, initialAssistantState)
  const planItem = (s: AssistantState) => s.items.find((i) => i.kind === 'plan')

  it('send -> plan -> execute -> progress -> done', () => {
    const p = plan([step('set_flag', {}, { destructive: true })])
    let s = run({ type: 'send', text: '淘汰闭眼' })
    expect(s.planning).toBe(true)
    s = run({ type: 'send', text: '淘汰闭眼' }, { type: 'plan', plan: p })
    expect(s.planning).toBe(false)
    expect(planItem(s)).toMatchObject({ phase: 'planned' })
    s = assistantReducer(s, { type: 'executeStarted', planId: 'p1', taskId: 't1' })
    expect(planItem(s)).toMatchObject({ phase: 'executing', taskId: 't1' })
    s = assistantReducer(s, { type: 'progress', taskId: 't1', done: 2, total: 5 })
    expect(planItem(s)).toMatchObject({ progress: { done: 2, total: 5 } })
    s = assistantReducer(s, { type: 'progress', taskId: 'other', done: 9, total: 9 })
    expect(planItem(s)).toMatchObject({ progress: { done: 2, total: 5 } })
    s = assistantReducer(s, { type: 'done', planId: 'p1', ok: true, results: [{ tool: 'set_flag', ok: true, affected: 5 }], undoable: true })
    expect(planItem(s)).toMatchObject({ phase: 'done', ok: true, undoable: true })
    s = assistantReducer(s, { type: 'undone', planId: 'p1' })
    expect(planItem(s)).toMatchObject({ undone: true, undoable: false })
  })

  it('filter-only plans are applied immediately', () => {
    const s = run({ type: 'send', text: 'x' }, { type: 'plan', plan: plan([step('filter')]) })
    expect(planItem(s)).toMatchObject({ phase: 'applied' })
  })

  it('unsupported plans show no executable card', () => {
    const s = run({ type: 'send', text: 'x' }, { type: 'plan', plan: plan([], { unsupported: '看不懂' }) })
    expect(planItem(s)).toMatchObject({ phase: 'dismissed' })
  })

  it('failed execution and failed planning keep the transcript', () => {
    let s = run({ type: 'send', text: 'x' }, { type: 'plan', plan: plan([step('set_flag')]) })
    s = assistantReducer(s, { type: 'executeFailed', planId: 'p1', message: 'nope' })
    expect(planItem(s)).toMatchObject({ phase: 'failed', error: 'nope' })
    s = assistantReducer(s, { type: 'planFailed', message: 'down' })
    expect(s.items.at(-1)).toMatchObject({ kind: 'error', text: 'down' })
  })

  it('ids are unique', () => {
    const s = run({ type: 'send', text: 'a' }, { type: 'plan', plan: plan([step('set_flag')]) }, { type: 'send', text: 'b' })
    const ids = s.items.map((i) => i.id)
    expect(new Set(ids).size).toBe(ids.length)
  })
})

describe('undo payload -> ONE history entry', () => {
  it('merges photo and edit changes and skips unchanged ones', () => {
    const entry = undoToEntry(
      'AI',
      {
        photos: [
          { id: 1, user_rating: 2, flag: 0, color_label: null },
          { id: 2, user_rating: 3, flag: 0, color_label: null },
        ],
        edits: [
          { photo_id: 5, before: emptyStack() },
          { photo_id: 6, before: stackWith(1) },
        ],
      },
      {
        photos: new Map([
          [1, { user_rating: 2, flag: -1 as const, color_label: null }],
          [2, { user_rating: 3, flag: 0 as const, color_label: null }],
        ]),
        stacks: new Map([
          [5, stackWith(0.4)],
          [6, stackWith(1)],
        ]),
      },
    )
    expect(entry).not.toBeNull()
    expect(entry!.label).toBe('AI')
    expect(entry!.changes).toEqual([{ id: 1, before: { flag: 0 }, after: { flag: -1 } }])
    expect(entry!.edits).toHaveLength(1)
    expect(entry!.edits![0]).toMatchObject({ photoId: 5, before: emptyStack(), after: stackWith(0.4) })
  })

  it('returns null when nothing changed', () => {
    expect(undoToEntry('x', { photos: [], edits: [] }, { photos: new Map(), stacks: new Map() })).toBeNull()
    expect(undoToEntry('x', undefined, { photos: new Map(), stacks: new Map() })).toBeNull()
  })

  it('pushing the entry gives exactly one undo step that undo() hands back whole', () => {
    useHistory.getState().clear()
    const entry = undoToEntry(
      'AI',
      { photos: [{ id: 1, user_rating: null, flag: 0, color_label: null }], edits: [{ photo_id: 2, before: emptyStack() }] },
      { photos: new Map([[1, { user_rating: 5 }]]), stacks: new Map([[2, stackWith(1)]]) },
    )!
    useHistory.getState().push(entry)
    expect(useHistory.getState().undoStack).toHaveLength(1)
    const popped = useHistory.getState().undo()
    expect(popped?.changes[0]).toMatchObject({ id: 1, before: { user_rating: null }, after: { user_rating: 5 } })
    expect(popped?.edits?.[0].photoId).toBe(2)
    expect(useHistory.getState().undoStack).toHaveLength(0)
  })
})
