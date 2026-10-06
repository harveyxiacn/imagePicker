import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Bot, Check, CheckCircle2, Eye, Loader2, Play, Send, Sparkles, Trash2, Undo2, X, XCircle } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useAssistantStatus, useMe } from '@/api/queries'
import type { AssistantPlan, PlanStep } from '@/api/types'
import { affectedOf, needsConfirm, selectionOf, summarizeResults, type TranscriptItem } from '@/lib/assistant'
import { applyFilterQuery, executePlan, isTopEntry, preview, sendMessage, undoPlan } from '@/lib/assistantRun'
import { can } from '@/lib/auth'
import { useHistory } from '@/lib/history'
import { useAssistant } from '@/stores/assistant'
import { useUi } from '@/stores/ui'
import { Modal } from '../Modal'

const CHIPS = ['best', 'tone', 'picked', 'bystanders'] as const

/** Ctrl+J toggle, mounted once per page that has a photo context (library, edit). Renders the drawer itself. */
export function AssistantHost({ sessionId, photoId = null }: { sessionId: number; photoId?: number | null }) {
  const me = useMe()
  const allowed = can(me.data?.role ?? 'owner', 'assistant')
  const open = useAssistant((s) => s.open)

  useEffect(() => {
    if (!allowed) return
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'j') {
        e.preventDefault()
        useAssistant.getState().toggle()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [allowed])

  if (!allowed || !open) return null
  return <AssistantDrawer sessionId={sessionId} photoId={photoId} />
}

/** Top-bar toggle button (hidden for guests). */
export function AssistantButton() {
  const { t } = useTranslation()
  const me = useMe()
  const open = useAssistant((s) => s.open)
  const toggle = useAssistant((s) => s.toggle)
  if (!can(me.data?.role ?? 'owner', 'assistant')) return null
  return (
    <button className="btn btn-ai" aria-pressed={open} onClick={toggle} title={`${t('assistant.title')} (Ctrl+J)`} data-testid="assistant-toggle">
      <Sparkles size={14} />
      <span className="hidden lg:inline">{t('assistant.title')}</span>
    </button>
  )
}

function EngineBadge() {
  const { t } = useTranslation()
  const status = useAssistantStatus()
  if (!status.data) return null
  const llm = status.data.engine === 'llm'
  return (
    <span
      className={`rounded px-1.5 py-0.5 text-[11px] font-medium ${llm ? 'bg-ai/20 text-ai' : 'bg-line text-muted'}`}
      title={llm ? t('assistant.engineLlmHint', { model: status.data.llm_model ?? '' }) : t('assistant.engineRulesHint')}
      data-testid="engine-badge"
      data-engine={status.data.engine}
    >
      {llm ? t('assistant.engineLlm') : t('assistant.engineRules')}
    </span>
  )
}

function AssistantDrawer({ sessionId, photoId }: { sessionId: number; photoId: number | null }) {
  const { t } = useTranslation()
  const items = useAssistant((s) => s.items)
  const planning = useAssistant((s) => s.planning)
  const setOpen = useAssistant((s) => s.setOpen)
  const dispatch = useAssistant((s) => s.dispatch)
  const [text, setText] = useState('')
  const bottom = useRef<HTMLDivElement>(null)
  const input = useRef<HTMLInputElement>(null)

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: 'end' })
  }, [items, planning])
  useEffect(() => input.current?.focus(), [])

  const send = (msg: string) => {
    if (!msg.trim() || planning) return
    setText('')
    void sendMessage(sessionId, msg, photoId)
  }

  return (
    <aside
      className="anim-pop fixed top-12 right-0 bottom-0 z-40 flex w-[min(400px,100vw)] flex-col border-l border-line bg-panel shadow-[var(--shadow)] max-sm:top-0"
      aria-label={t('assistant.title')}
      data-testid="assistant-drawer"
      onKeyDown={(e) => {
        if (e.key === 'Escape') setOpen(false)
      }}
    >
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-line px-3">
        <Sparkles size={15} className="text-ai" />
        <span className="font-semibold">{t('assistant.title')}</span>
        <EngineBadge />
        <span className="ml-auto flex items-center gap-1">
          <button className="btn btn-ghost btn-icon" onClick={() => dispatch({ type: 'clear' })} disabled={!items.length} aria-label={t('assistant.clear')} title={t('assistant.clear')}>
            <Trash2 size={14} />
          </button>
          <button className="btn btn-ghost btn-icon" onClick={() => setOpen(false)} aria-label={t('common.close')} data-testid="assistant-close">
            <X size={15} />
          </button>
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-3" data-testid="assistant-transcript">
        <div className="flex flex-col gap-3">
          <Bubble role="bot">
            {t('assistant.intro')}
            <div className="mt-1.5 text-xs text-muted">{t('assistant.introHint')}</div>
          </Bubble>
          {items.map((it) => (
            <Item key={it.id} item={it} />
          ))}
          {planning && (
            <Bubble role="bot">
              <span className="flex items-center gap-2 text-muted" data-testid="assistant-thinking">
                <Loader2 size={13} className="animate-spin" />
                {t('assistant.thinking')}
              </span>
            </Bubble>
          )}
          <div ref={bottom} />
        </div>
      </div>

      <div className="shrink-0 border-t border-line p-3">
        <div className="mb-2 flex flex-wrap gap-1.5" data-testid="assistant-chips">
          {CHIPS.map((c) => (
            <button key={c} className="chip chip-ai" disabled={planning} onClick={() => send(t(`assistant.chip_${c}_prompt`))} data-testid={`chip-${c}`}>
              {t(`assistant.chip_${c}`)}
            </button>
          ))}
        </div>
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            send(text)
          }}
        >
          <input
            ref={input}
            className="field min-w-0 flex-1"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={t('assistant.placeholder')}
            aria-label={t('assistant.placeholder')}
            data-testid="assistant-input"
          />
          <button className="btn btn-primary btn-icon" type="submit" disabled={planning || !text.trim()} aria-label={t('assistant.send')} data-testid="assistant-send">
            <Send size={14} />
          </button>
        </form>
      </div>
    </aside>
  )
}

function Bubble({ role, children }: { role: 'user' | 'bot' | 'error'; children: React.ReactNode }) {
  if (role === 'user')
    return (
      <div className="ml-8 self-end rounded-card bg-accent/90 px-3 py-1.5 text-accent-fg" data-testid="msg-user">
        {children}
      </div>
    )
  return (
    <div className={`flex gap-2 ${role === 'error' ? 'text-danger' : ''}`} data-testid={role === 'error' ? 'msg-error' : 'msg-bot'}>
      <span className={`mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full ${role === 'error' ? 'bg-danger/20' : 'bg-ai/20 text-ai'}`}>
        {role === 'error' ? <AlertTriangle size={13} /> : <Bot size={14} />}
      </span>
      <div className="min-w-0 flex-1 rounded-card bg-elevated px-3 py-1.5">{children}</div>
    </div>
  )
}

function Item({ item }: { item: TranscriptItem }) {
  if (item.kind === 'user') return <Bubble role="user">{item.text}</Bubble>
  if (item.kind === 'text') return <Bubble role="bot">{item.text}</Bubble>
  if (item.kind === 'error') return <Bubble role="error">{item.text}</Bubble>
  return <PlanCard item={item} />
}

const STEP_TOOL_I18N = (tool: string) => `assistant.tool_${tool}`

function StepRow({ step, index, result }: { step: PlanStep; index: number; result?: { ok: boolean; affected: number; error?: string | null } }) {
  const { t } = useTranslation()
  return (
    <li className="flex items-start gap-2" data-testid="plan-step" data-tool={step.tool} data-destructive={step.destructive}>
      <span className="tnum mt-0.5 w-4 shrink-0 text-right text-faint">{index + 1}.</span>
      <span className="min-w-0 flex-1">
        <span>{step.summary}</span>
        <span className="mt-0.5 flex flex-wrap items-center gap-1.5 text-[11px]">
          <span className="rounded bg-bg/60 px-1.5 py-0.5 text-muted">{t(STEP_TOOL_I18N(step.tool), { defaultValue: step.tool })}</span>
          {step.tool !== 'filter' && <span className="tnum rounded bg-bg/60 px-1.5 py-0.5 text-muted">{t('assistant.affects', { n: step.affects })}</span>}
          {step.destructive && (
            <span className="flex items-center gap-1 rounded bg-warning/20 px-1.5 py-0.5 font-medium text-warning" data-testid="destructive-badge">
              <AlertTriangle size={10} />
              {t('assistant.destructive')}
            </span>
          )}
          {result && (
            <span className={`flex items-center gap-1 ${result.ok ? 'text-success' : 'text-danger'}`}>
              {result.ok ? <CheckCircle2 size={11} /> : <XCircle size={11} />}
              {result.ok ? t('assistant.stepDone', { n: result.affected }) : (result.error ?? t('assistant.failed'))}
            </span>
          )}
        </span>
      </span>
    </li>
  )
}

function PlanCard({ item }: { item: Extract<TranscriptItem, { kind: 'plan' }> }) {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const [confirm, setConfirm] = useState(false)
  const [busy, setBusy] = useState(false)
  const stackTop = useHistory((s) => s.undoStack[s.undoStack.length - 1])
  const { plan, phase } = item
  void stackTop // re-render when the history changes (undo is only offered while this step is the newest)

  if (plan.unsupported) {
    return (
      <div className="ml-8 rounded-card border border-line bg-bg/40 p-2.5 text-xs text-muted" data-testid="plan-unsupported">
        {plan.unsupported}
      </div>
    )
  }

  const run = async () => {
    setConfirm(false)
    setBusy(true)
    try {
      if (isExportOnly(plan)) openExport(plan)
      else await executePlan(plan)
    } finally {
      setBusy(false)
    }
  }
  const results = item.results ?? []
  const sum = summarizeResults(results)

  return (
    <div className="ml-8 rounded-card border border-ai/40 bg-ai/5 p-2.5" data-testid="plan-card" data-phase={phase}>
      <div className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold text-ai">
        <Sparkles size={12} />
        {t('assistant.plan')}
        <span className="tnum font-normal text-muted">· {t('assistant.affects', { n: affectedOf(plan) })}</span>
      </div>
      <ol className="flex flex-col gap-1.5">
        {plan.steps.map((s, i) => (
          <StepRow key={i} step={s} index={i} result={results[i]} />
        ))}
      </ol>

      {phase === 'planned' && (
        <div className="mt-2.5 flex flex-wrap gap-1.5">
          <button className="btn" onClick={() => preview(plan)} data-testid="plan-preview">
            <Eye size={13} />
            {t('assistant.preview')}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => (needsConfirm(plan) ? setConfirm(true) : void run())} data-testid="plan-execute">
            {busy ? <Loader2 size={13} className="animate-spin" /> : <Play size={13} />}
            {t('assistant.execute')}
          </button>
          <button className="btn btn-ghost" onClick={() => useAssistant.getState().dispatch({ type: 'dismiss', planId: plan.plan_id })} data-testid="plan-cancel">
            {t('common.cancel')}
          </button>
        </div>
      )}
      {phase === 'applied' && (
        <div className="mt-2 flex items-center gap-1.5 text-xs text-success" data-testid="plan-applied">
          <Check size={13} />
          {t('assistant.applied')}
        </div>
      )}
      {phase === 'executing' && (
        <div className="mt-2.5" data-testid="plan-progress">
          <div className="mb-1 flex items-center justify-between text-xs text-muted">
            <span className="flex items-center gap-1.5">
              <Loader2 size={12} className="animate-spin text-ai" />
              {t('assistant.executing')}
            </span>
            <span className="tnum">
              {item.progress?.done ?? 0}/{item.progress?.total || plan.steps.length}
            </span>
          </div>
          <div className="h-1.5 overflow-hidden rounded bg-line">
            <div
              className="h-full bg-ai transition-[width] duration-300"
              style={{ width: `${Math.round(((item.progress?.done ?? 0) / (item.progress?.total || plan.steps.length || 1)) * 100)}%` }}
            />
          </div>
        </div>
      )}
      {phase === 'done' && (
        <div className="mt-2.5 flex flex-col gap-1.5" data-testid="plan-result">
          <div className="flex items-center gap-1.5 text-xs font-medium text-success">
            <CheckCircle2 size={13} />
            {item.undone ? t('assistant.undone') : t('assistant.resultOk', { n: sum.affected })}
          </div>
          <DataResults results={results} />
          {item.undoable && !item.undone && (
            <div>
              <button className="btn" disabled={!isTopEntry(plan.plan_id)} onClick={() => void undoPlan(qc, plan.plan_id)} title={isTopEntry(plan.plan_id) ? undefined : t('assistant.undoBlocked')} data-testid="plan-undo">
                <Undo2 size={13} />
                {t('assistant.undo')}
              </button>
            </div>
          )}
        </div>
      )}
      {phase === 'failed' && (
        <div className="mt-2.5 flex flex-col gap-1.5" data-testid="plan-failed">
          <div className="flex items-start gap-1.5 text-xs text-danger">
            <XCircle size={13} className="mt-0.5 shrink-0" />
            {item.error ?? t('assistant.failed')}
          </div>
          {results.length === 0 && (
            <div>
              <button className="btn" onClick={() => void run()}>
                {t('common.retry')}
              </button>
            </div>
          )}
        </div>
      )}
      {phase === 'dismissed' && <div className="mt-2 text-xs text-faint">{t('assistant.dismissed')}</div>}

      <Modal
        open={confirm}
        onOpenChange={setConfirm}
        title={t('assistant.confirmTitle')}
        description={t('assistant.confirmDesc', { n: affectedOf(plan) })}
        width="max-w-md"
        footer={
          <>
            <button className="btn" onClick={() => setConfirm(false)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" onClick={() => void run()} data-testid="plan-confirm">
              {t('assistant.confirmRun')}
            </button>
          </>
        }
      >
        <ul className="flex flex-col gap-1.5">
          {plan.steps
            .filter((s) => s.destructive)
            .map((s, i) => (
              <li key={i} className="flex items-start gap-2">
                <AlertTriangle size={14} className="mt-0.5 shrink-0 text-warning" />
                {s.summary}
              </li>
            ))}
        </ul>
        <div className="mt-3 text-xs text-muted">{t('assistant.confirmHint')}</div>
      </Modal>
    </div>
  )
}

/** `describe` captions etc. returned by the server in `results[].data`. */
function DataResults({ results }: { results: { tool: string; data?: unknown }[] }) {
  const d = results.find((r) => r.tool === 'describe')?.data as { caption?: string; keywords?: string[] } | undefined
  if (!d?.caption) return null
  return (
    <div className="rounded bg-bg/60 p-2 text-xs">
      <div>{d.caption}</div>
      {d.keywords?.length ? <div className="mt-1 text-muted">{d.keywords.join(' · ')}</div> : null}
    </div>
  )
}

/** An `export` plan without a destination is completed in the export dialog (the server needs a `dest`). */
function isExportOnly(plan: AssistantPlan): boolean {
  return plan.steps.every((s) => s.tool === 'export' && !s.args.dest)
}

function openExport(plan: AssistantPlan): void {
  const step = plan.steps[0]
  const sel = selectionOf(step)
  const ui = useUi.getState()
  if (sel?.kind === 'ids') ui.setSelection({ ids: new Set(sel.ids), anchorId: sel.ids[0] ?? null })
  if (sel?.kind === 'query') applyFilterQuery(sel.query)
  ui.setExportPreset(typeof step.args.preset === 'string' ? step.args.preset : null)
  ui.setExportOpen(true)
  useAssistant.getState().dispatch({ type: 'applied', planId: plan.plan_id })
}
