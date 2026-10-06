import { useQueryClient } from '@tanstack/react-query'
import { Download, Loader2, Trash2, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { useRuntime } from '@/api/queries'
import { qk } from '@/lib/cache'
import { formatBytes } from '@/lib/format'
import { runtimeView } from '@/lib/runtime'
import { useToasts } from '@/stores/toasts'
import { Card, ConfirmDialog, Row } from './controls'

const TONE = { muted: 'bg-faint', ai: 'bg-ai', success: 'bg-success', danger: 'bg-danger' } as const

/** M7: install state of the private AI runtime, with install / cancel / remove. */
export function RuntimeCard() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const rt = useRuntime()
  const [askRemove, setAskRemove] = useState(false)
  const [busy, setBusy] = useState(false)
  const v = runtimeView(rt.data)
  const info = rt.data

  const run = async (fn: () => Promise<unknown>, done?: string) => {
    setBusy(true)
    try {
      await fn()
      if (done) useToasts.getState().push('success', done, 2500)
    } catch (e) {
      useToasts.getState().push('error', e instanceof Error ? e.message : String(e), 6000)
    } finally {
      setBusy(false)
      void qc.invalidateQueries({ queryKey: qk.runtime })
    }
  }

  if (rt.isError) return null // an older server without /api/runtime (or a guest): nothing to show

  return (
    <Card title={t('runtime.title')} hint={t('runtime.hint')} testId="settings-runtime">
      <Row label={t('settings.hardware.worker')} hint={info?.state === 'failed' ? info.error : v.detail || undefined} testId="runtime-state-row">
        {info ? (
          <span className="flex items-center gap-2" data-testid="runtime-state" data-state={info.state}>
            <span className={`h-2 w-2 rounded-full ${TONE[v.tone]}`} />
            {t(`runtime.${v.labelKey}`)}
          </span>
        ) : (
          <Loader2 size={14} className="animate-spin text-muted" />
        )}
      </Row>
      {v.percent !== null && (
        <div className="flex flex-col gap-1 px-4 py-3" data-testid="runtime-progress">
          <div className="flex justify-between text-xs text-muted">
            <span>{v.stepKey ? t(`runtime.${v.stepKey}`) : t('runtime.installingHint')}</span>
            <span className="tnum">{v.percent}%</span>
          </div>
          <span className="h-1.5 overflow-hidden rounded bg-line">
            <span className="block h-full bg-ai transition-[width] duration-200" style={{ width: `${v.percent}%` }} />
          </span>
        </div>
      )}
      {info?.state === 'ready' && (
        <Row label={t('runtime.extras')} hint={info.python ?? undefined}>
          <span className="text-right text-xs text-muted" data-testid="runtime-extras">
            {info.extras.join(' · ')}
            {info.venv_bytes ? ` · ${formatBytes(info.venv_bytes)}` : ''}
          </span>
        </Row>
      )}
      {info && !v.canInstall && v.needsInstall && info.reason && <div className="px-4 py-2 text-xs text-warning">{info.reason}</div>}
      <div className="flex flex-wrap justify-end gap-2 px-4 py-3">
        {v.canCancel && (
          <button className="btn" disabled={busy} onClick={() => void run(api.cancelRuntime)} data-testid="runtime-cancel">
            <X size={13} />
            {t('runtime.cancel')}
          </button>
        )}
        {v.canRemove && (
          <button className="btn btn-ghost" disabled={busy} onClick={() => setAskRemove(true)} data-testid="runtime-remove">
            <Trash2 size={13} />
            {t('runtime.remove')}
          </button>
        )}
        {(v.needsInstall || info?.state === 'ready') && (
          <button
            className={`btn ${v.needsInstall ? 'btn-primary' : ''}`}
            disabled={busy || !v.canInstall}
            onClick={() => void run(() => api.installRuntime())}
            data-testid="runtime-install"
          >
            {busy ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
            {v.needsInstall ? t('runtime.install') : t('runtime.reinstall')}
          </button>
        )}
      </div>
      <ConfirmDialog
        open={askRemove}
        onOpenChange={setAskRemove}
        title={t('runtime.removeTitle')}
        description={t('runtime.removeDesc')}
        confirmLabel={t('runtime.remove')}
        danger
        testId="runtime-remove-confirm"
        onConfirm={() => {
          setAskRemove(false)
          void run(api.removeRuntime, t('runtime.removed'))
        }}
      />
    </Card>
  )
}
