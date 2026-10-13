import type { ModelInfo, ModelUsage } from '@/api/types'

/** How a model's licence is presented. `ok` shows a plain label, `warn` a highlighted badge. */
export interface LicenceBadge {
  usage: ModelUsage
  level: 'ok' | 'warn'
  /** The user has to confirm before downloading (non-commercial / research-only). */
  needsAck: boolean
}

const USAGES: ModelUsage[] = ['commercial-ok', 'research-only', 'non-commercial', 'unknown']

/**
 * Effective usage flag of a model. The legacy `noncommercial` boolean always wins over a looser
 * `usage`; a missing or unrecognised flag is `unknown` (never silently "ok").
 */
export function effectiveUsage(m: Pick<ModelInfo, 'usage' | 'noncommercial'>): ModelUsage {
  if (m.noncommercial) return 'non-commercial'
  return USAGES.includes(m.usage) ? m.usage : 'unknown'
}

export function licenceBadge(m: Pick<ModelInfo, 'usage' | 'noncommercial'>): LicenceBadge {
  const usage = effectiveUsage(m)
  return { usage, level: usage === 'commercial-ok' ? 'ok' : 'warn', needsAck: usage === 'non-commercial' || usage === 'research-only' }
}

/** The distinct non-ok usages in `models`, worst first; drives which warning text a list shows. */
export function licenceWarnings(models: Pick<ModelInfo, 'usage' | 'noncommercial'>[]): ModelUsage[] {
  const present = new Set(models.map((m) => effectiveUsage(m)))
  return (['non-commercial', 'research-only', 'unknown'] as const).filter((u) => present.has(u))
}
