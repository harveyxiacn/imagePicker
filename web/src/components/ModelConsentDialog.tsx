import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Download, Loader2 } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useModels } from '@/api/queries'
import { startAnalysis } from '@/lib/analysis'
import { formatBytes } from '@/lib/format'
import { useAnalysisUi } from '@/stores/analysis'
import { useToasts } from '@/stores/toasts'
import { Modal } from './Modal'

const mb = (n: number) => (n >= 1024 ? `${(n / 1024).toFixed(1)} GB` : n >= 10 ? `${Math.round(n)} MB` : `${n} MB`)

/** Shown on 409 models_missing: lists the models (size, licence; non-commercial highlighted) and downloads them on consent. */
export function ModelConsentDialog() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const consent = useAnalysisUi((s) => s.consent)
  const setConsent = useAnalysisUi((s) => s.setConsent)
  const ensureTaskId = useAnalysisUi((s) => s.ensureTaskId)
  const ensureError = useAnalysisUi((s) => s.ensureError)
  const setEnsure = useAnalysisUi((s) => s.setEnsure)
  const task = useToasts((s) => (ensureTaskId ? s.tasks[ensureTaskId] : undefined))
  const models = useModels(consent !== null)
  const [busy, setBusy] = useState(false)
  const retried = useRef<string | null>(null)

  const list = (models.data ?? []).filter((m) => consent?.models.includes(m.id))
  const total = list.reduce((n, m) => n + m.size_mb, 0)
  const hasNc = list.some((m) => m.noncommercial)

  const downloading = ensureTaskId !== null && task?.state !== 'failed' && task?.state !== 'done'
  const failed = task?.state === 'failed' ? (task.error ?? t('models.failed')) : ensureError

  // Download finished: retry the analysis the user asked for.
  useEffect(() => {
    if (!consent || !ensureTaskId || task?.state !== 'done' || retried.current === ensureTaskId) return
    retried.current = ensureTaskId
    const c = consent
    useToasts.getState().dismissTask(ensureTaskId) // analysis resumes right away: no "ready" toast
    void qc.invalidateQueries({ queryKey: ['models'] })
    setConsent(null)
    if (c.onReady) c.onReady()
    else void startAnalysis(qc, c.sessionId, c.profile, c.photoIds)
  }, [consent, ensureTaskId, task?.state, qc, setConsent])

  const confirm = async () => {
    if (!consent) return
    setBusy(true)
    try {
      const { task_id } = await api.ensureModels(consent.models)
      setEnsure(task_id)
    } catch (err) {
      setEnsure(null, err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const pct = task && task.total > 0 ? Math.round((task.done / task.total) * 100) : 0

  return (
    <Modal
      open={consent !== null}
      onOpenChange={(o) => {
        if (!o) setConsent(null)
      }}
      title={t('models.title')}
      description={t('models.desc')}
      width="max-w-xl"
      footer={
        <>
          <button className="btn" onClick={() => setConsent(null)}>
            {downloading ? t('models.background') : t('common.cancel')}
          </button>
          <button className="btn btn-primary" disabled={busy || downloading || models.isPending} onClick={() => void confirm()} data-testid="models-confirm">
            {busy || downloading ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
            {failed ? t('common.retry') : t('models.confirm', { size: mb(total) })}
          </button>
        </>
      }
    >
      <div data-testid="models-dialog">
        {models.isPending ? (
          <div className="flex items-center gap-2 text-muted">
            <Loader2 size={14} className="animate-spin" />
            {t('library.loading')}
          </div>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {list.map((m) => (
              <li
                key={m.id}
                className="flex items-center gap-3 rounded-control border px-3 py-2"
                style={{
                  borderColor: m.noncommercial ? 'color-mix(in srgb, var(--warning) 60%, transparent)' : 'var(--line)',
                  background: m.noncommercial ? 'color-mix(in srgb, var(--warning) 9%, transparent)' : undefined,
                }}
                data-nc={m.noncommercial}
              >
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium">{m.id}</div>
                  <div className="truncate text-xs text-muted">{m.task.map((k) => t(`models.task_${k}`, { defaultValue: k })).join(' · ')}</div>
                </div>
                <span className="tnum text-xs text-muted">{mb(m.size_mb)}</span>
                <span
                  className={`flex items-center gap-1 rounded px-1.5 py-0.5 text-xs ${m.noncommercial ? 'bg-warning/20 font-semibold text-warning' : 'bg-panel text-muted'}`}
                  title={m.noncommercial ? t('models.ncHint') : m.license}
                >
                  {m.noncommercial && <AlertTriangle size={12} />}
                  {m.license}
                  {m.noncommercial && ` · ${t('models.nc')}`}
                </span>
              </li>
            ))}
            {models.isError && <li className="text-danger">{(models.error as Error).message}</li>}
          </ul>
        )}
        {hasNc && (
          <div className="mt-3 flex items-start gap-2 rounded-control bg-warning/10 p-2 text-xs text-warning">
            <AlertTriangle size={14} className="mt-0.5 shrink-0" />
            {t('models.ncWarning')}
          </div>
        )}
        {(ensureTaskId || failed) && (
          <div className="mt-4" data-testid="models-progress">
            <div className="mb-1 flex justify-between text-xs text-muted">
              <span>{failed ? <span className="text-danger">{failed}</span> : t('models.downloading')}</span>
              {task && (
                <span className="tnum">
                  {formatBytes(task.done)} / {formatBytes(task.total)} · {pct}%
                </span>
              )}
            </div>
            <div className="h-2 overflow-hidden rounded bg-line">
              <div className={`h-full transition-[width] duration-200 ${failed ? 'bg-danger' : 'bg-ai'}`} style={{ width: `${pct}%` }} />
            </div>
          </div>
        )}
      </div>
    </Modal>
  )
}
