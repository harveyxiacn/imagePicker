import { AlertTriangle } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { ModelInfo } from '@/api/types'
import { licenceBadge } from '@/lib/licence'

/** Weights licence plus, unless the model is recorded as commercial-ok, a highlighted usage badge with a short explanation. */
export function LicenseBadge({ model, size = 12, plainClass = 'bg-panel text-muted' }: { model: ModelInfo; size?: number; plainClass?: string }) {
  const { t } = useTranslation()
  const b = licenceBadge(model)
  const info = model.license_info
  const title = [
    b.level === 'warn' ? t(`models.hint_${b.usage}`) : model.license,
    info?.training_data ? `${t('models.trainingData')}: ${info.training_data} (${t(`models.terms_${info.training_data_terms}`)})` : null,
    info?.note,
  ]
    .filter(Boolean)
    .join('\n')
  return (
    <span
      className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-xs ${b.level === 'warn' ? 'bg-warning/20 font-semibold text-warning' : plainClass}`}
      title={title}
      data-nc={b.usage === 'non-commercial'}
      data-usage={b.usage}
      data-testid="license-badge"
    >
      {b.level === 'warn' && <AlertTriangle size={size} />}
      {model.license}
      {b.level === 'warn' && ` · ${t(`models.usage_${b.usage}`)}`}
    </span>
  )
}
