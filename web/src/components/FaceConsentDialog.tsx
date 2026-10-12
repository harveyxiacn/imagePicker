import { useQueryClient } from '@tanstack/react-query'
import { Loader2, ScanFace } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import { startAnalysis } from '@/lib/analysis'
import { qk } from '@/lib/cache'
import { errorText } from '@/lib/errors'
import { faceConsentPatch } from '@/lib/faceConsent'
import { useFaceConsent } from '@/stores/faceConsent'
import { useToasts } from '@/stores/toasts'
import { Modal } from './Modal'

const POINTS = ['purpose', 'local', 'off', 'wipe'] as const

/**
 * Purpose explanation + consent for face recognition (docs/02 §8, P0-4). Shown before the first
 * analysis (and when face recognition is switched on in the settings); until the user agrees the
 * server detects no faces and computes no face features. Declining switches face recognition off.
 * Closing the dialog answers nothing and starts nothing.
 */
export function FaceConsentDialog() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const open = useFaceConsent((s) => s.open)
  const analysis = useFaceConsent((s) => s.analysis)
  const close = useFaceConsent((s) => s.close)
  const [busy, setBusy] = useState<'agree' | 'decline' | null>(null)

  const answer = async (agree: boolean) => {
    setBusy(agree ? 'agree' : 'decline')
    try {
      const s = await api.patchSettings(faceConsentPatch(agree))
      qc.setQueryData(qk.settings, s)
      const pending = analysis
      close()
      if (pending) void startAnalysis(qc, pending.sessionId, pending.profile, pending.photoIds)
      else useToasts.getState().push('success', t(agree ? 'faceConsent.agreed' : 'faceConsent.declined'), 3500)
    } catch (e) {
      useToasts.getState().push('error', errorText(e), 6000)
    } finally {
      setBusy(null)
    }
  }

  return (
    <Modal
      open={open}
      onOpenChange={(o) => {
        if (!o && busy === null) close()
      }}
      title={t('faceConsent.title')}
      description={t('faceConsent.lead')}
      width="max-w-lg"
      footer={
        <>
          <button className="btn" disabled={busy !== null} onClick={() => void answer(false)} data-testid="face-consent-decline">
            {busy === 'decline' && <Loader2 size={14} className="animate-spin" />}
            {t('faceConsent.decline')}
          </button>
          <button className="btn btn-primary" disabled={busy !== null} onClick={() => void answer(true)} data-testid="face-consent-agree">
            {busy === 'agree' ? <Loader2 size={14} className="animate-spin" /> : <ScanFace size={14} />}
            {t('faceConsent.agree')}
          </button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm leading-relaxed" data-testid="face-consent-dialog">
        <ul className="flex list-disc flex-col gap-1.5 pl-5">
          {POINTS.map((p) => (
            <li key={p}>{t(`faceConsent.point_${p}`)}</li>
          ))}
        </ul>
        <p className="text-xs text-muted">{t('faceConsent.note')}</p>
      </div>
    </Modal>
  )
}
