import { Check, LifeBuoy, Loader2, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { exportDiagnostics } from '@/api/client'
import { DIAGNOSTICS_EXCLUDED, DIAGNOSTICS_INCLUDED, saveBlob } from '@/lib/diagnostics'
import { errorText } from '@/lib/errors'
import { useToasts } from '@/stores/toasts'
import { Card, Row } from './controls'

/** 诊断: one button that downloads a zip for bug reports (versions, system, redacted settings, logs, crash records, catalog statistics; never photos). */
export function DiagnosticsSection() {
  const { t } = useTranslation()
  const [busy, setBusy] = useState(false)
  const run = async () => {
    setBusy(true)
    try {
      const { blob, filename } = await exportDiagnostics()
      saveBlob(blob, filename)
      useToasts.getState().push('success', t('settings.diagnostics.saved', { name: filename }), 5000)
    } catch (e) {
      useToasts.getState().push('error', errorText(e), 8000)
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex flex-col gap-4" data-testid="settings-diagnostics">
      <Card title={t('settings.diagnostics.title')} hint={t('settings.diagnostics.hint')}>
        <Row label={t('settings.diagnostics.export')} hint={t('settings.diagnostics.exportHint')}>
          <button className="btn" disabled={busy} onClick={() => void run()} data-testid="diagnostics-export">
            {busy ? <Loader2 size={13} className="animate-spin" /> : <LifeBuoy size={13} />}
            {busy ? t('settings.diagnostics.exporting') : t('settings.diagnostics.exportButton')}
          </button>
        </Row>
      </Card>
      <div className="grid gap-4 md:grid-cols-2">
        <Card title={t('settings.diagnostics.includedTitle')}>
          <ul className="flex flex-col gap-1.5 px-4 py-3 text-xs" data-testid="diagnostics-included">
            {DIAGNOSTICS_INCLUDED.map((k) => (
              <li key={k} className="flex items-start gap-2">
                <Check size={13} className="mt-0.5 shrink-0 text-success" />
                <span>{t(`settings.diagnostics.include.${k}`)}</span>
              </li>
            ))}
          </ul>
        </Card>
        <Card title={t('settings.diagnostics.excludedTitle')}>
          <ul className="flex flex-col gap-1.5 px-4 py-3 text-xs" data-testid="diagnostics-excluded">
            {DIAGNOSTICS_EXCLUDED.map((k) => (
              <li key={k} className="flex items-start gap-2">
                <X size={13} className="mt-0.5 shrink-0 text-danger" />
                <span>{t(`settings.diagnostics.exclude.${k}`)}</span>
              </li>
            ))}
          </ul>
        </Card>
      </div>
    </div>
  )
}
