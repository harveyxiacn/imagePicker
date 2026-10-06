import { useQueryClient } from '@tanstack/react-query'
import { Check, Loader2, Sparkles, X } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'
import { useAssistantStatus } from '@/api/queries'
import type { EditSuggestion } from '@/api/types'
import { applySuggestion, requestSuggestion } from '@/lib/assistantRun'
import { diffAdjust, getAdjust, mergeAuto } from '@/lib/edit'
import { useEdit } from '@/stores/edit'
import { useToasts } from '@/stores/toasts'

/** "✨ 修图建议" (edit page, AI panel): VLM suggestions for the open photo; one click applies them as ONE undoable change. */
export function EditSuggest() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const { sessionId } = useParams()
  const status = useAssistantStatus()
  const photoId = useEdit((s) => s.photoId)
  const stack = useEdit((s) => s.stack)
  const [busy, setBusy] = useState(false)
  const [sug, setSug] = useState<{ photoId: number; data: EditSuggestion } | null>(null)
  const [applied, setApplied] = useState(false)

  const current = sug && sug.photoId === photoId ? sug.data : null
  const diff = useMemo(() => (current ? diffAdjust(getAdjust(stack), getAdjust(mergeAuto(stack, current.adjust))) : []), [current, stack])

  if (!status.data?.vlm_available) return null

  const run = async () => {
    if (photoId === null) return
    setBusy(true)
    setApplied(false)
    try {
      setSug({ photoId, data: await requestSuggestion(Number(sessionId), photoId) })
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 5000)
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <button className="btn btn-ai" disabled={busy || photoId === null} onClick={() => void run()} data-testid="suggest-edits" title={t('assistant.suggestHint')}>
        {busy ? <Loader2 size={13} className="animate-spin" /> : <Sparkles size={13} />}
        {t('assistant.suggest')}
      </button>
      {current && (
        <div className="anim-pop mt-2 w-full basis-full rounded-card border border-ai/40 bg-ai/10 p-2" data-testid="suggestions">
          <div className="mb-1.5 flex items-center gap-1 text-xs text-ai">
            <Sparkles size={12} />
            <span className="flex-1 font-medium">{t('assistant.suggestTitle')}</span>
            <button className="btn btn-ghost btn-icon !h-5 !w-5" aria-label={t('common.close')} onClick={() => setSug(null)}>
              <X size={11} />
            </button>
          </div>
          {current.problems.length > 0 && (
            <div className="mb-1.5 flex flex-wrap gap-1" data-testid="suggest-problems">
              {current.problems.map((p) => (
                <span key={p} className="rounded bg-warning/15 px-1.5 py-0.5 text-[11px] text-warning">
                  {problemLabel(p, t)}
                </span>
              ))}
            </div>
          )}
          <div className="mb-1.5 text-xs text-muted">{current.reason}</div>
          <div className="mb-2 flex flex-wrap gap-1">
            {diff.map((d) => (
              <span key={d.key} className="tnum rounded bg-bg/60 px-1.5 py-0.5 text-[11px]">
                <span className="text-muted">{t(`adjust.${d.key}`)} </span>
                <span className="text-faint">{d.from.toFixed(d.key === 'exposure' ? 2 : 0)} → </span>
                <span className="text-ai">{(d.to > 0 ? '+' : '') + d.to.toFixed(d.key === 'exposure' ? 2 : 0)}</span>
              </span>
            ))}
          </div>
          <button
            className="btn btn-primary"
            disabled={applied || diff.length === 0}
            onClick={() => setApplied(applySuggestion(qc, current))}
            data-testid="suggest-apply"
          >
            {applied ? <Check size={13} /> : <Sparkles size={13} />}
            {applied ? t('assistant.suggestApplied') : t('assistant.suggestApply')}
          </button>
        </div>
      )}
    </>
  )
}

function problemLabel(p: string, t: (k: string, o?: Record<string, unknown>) => string): string {
  if (p.startsWith('issue:')) return t(`issue.${p.slice(6)}`, { defaultValue: p.slice(6) })
  return t(`assistant.problem_${p}`, { defaultValue: p })
}
