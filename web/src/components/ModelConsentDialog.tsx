import { useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, Download, Loader2 } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useModels } from '@/api/queries'
import { startAnalysis } from '@/lib/analysis'
import { effectiveUsage, licenceWarnings } from '@/lib/licence'
import { offerRuntimeInstall } from '@/lib/runtime'
import { formatBytes } from '@/lib/format'
import { useAnalysisUi } from '@/stores/analysis'
import { useToasts } from '@/stores/toasts'
import { LicenseBadge } from './LicenseBadge'
import { Modal } from './Modal'
import { errorText } from '@/lib/errors'

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
  const warnings = licenceWarnings(list)

  const downloading = ensureTaskId !== null && task?.state !== 'failed' && task?.state !== 'done'
  const failed = task?.state === 'failed' ? (task.error ? errorText(task.error) : t('models.failed')) : ensureError

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
      if (offerRuntimeInstall(err)) {
        setConsent(null)
        return
      }
      setEnsure(null, errorText(err))
    } finally {
      setBusy(false)
    }
  }

  // the on-device profile needs no model: the way on when a download is not wanted or fails
  const canLite = consent !== null && !consent.onReady && consent.profile !== 'lite'
  const useLite = () => {
    const c = consent
    if (!c) return
    if (ensureTaskId) useToasts.getState().dismissTask(ensureTaskId)
    setConsent(null)
    void startAnalysis(qc, c.sessionId, 'lite', c.photoIds)
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
          {canLite && (
            <button className={`btn ${failed ? 'btn-ai' : ''}`} onClick={useLite} data-testid="models-lite" title={t('models.liteHint')}>
              {t('analysis.useLite')}
            </button>
          )}
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
                  borderColor: effectiveUsage(m) !== 'commercial-ok' ? 'color-mix(in srgb, var(--warning) 60%, transparent)' : 'var(--line)',
                  background: effectiveUsage(m) !== 'commercial-ok' ? 'color-mix(in srgb, var(--warning) 9%, transparent)' : undefined,
                }}
                data-nc={m.noncommercial}
                data-usage={effectiveUsage(m)}
              >
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium">{m.id}</div>
                  <div className="truncate text-xs text-muted">{m.task.map((k) => t(`models.task_${k}`, { defaultValue: k })).join(' · ')}</div>
                </div>
                <span className="tnum text-xs text-muted">{mb(m.size_mb)}</span>
                <LicenseBadge model={m} />
              </li>
            ))}
            {models.isError && <li className="text-danger">{errorText(models.error)}</li>}
          </ul>
        )}
        {warnings.length > 0 && (
          <div className="mt-3 flex flex-col gap-1.5" data-testid="licence-warnings">
            {warnings.map((u) => (
              <div key={u} className="flex items-start gap-2 rounded-control bg-warning/10 p-2 text-xs text-warning" data-usage={u}>
                <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                {t(`models.warn_${u}`)}
              </div>
            ))}
          </div>
        )}
        {canLite && failed && (
          <div className="mt-3 rounded-control bg-ai/10 p-2 text-xs" data-testid="models-lite-hint">
            {t('models.liteHint')}
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
