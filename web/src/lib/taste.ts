import type { Taste, TasteContribution, TasteMetrics } from '@/api/types'

/** Taste v2 (docs/03 §3.3): decisions that trigger an immediate retraining. */
export const TASTE_RETRAIN_EVERY = 50

export interface TasteNeeds {
  /** more kept photos (pick or 4–5 stars) needed before it can learn */
  keep: number
  /** more rejected photos (reject or 1–2 stars) needed */
  reject: number
  /** 0..1 cold-start progress (both classes count half) */
  fraction: number
}

/** Cold-start progress: both kept and rejected photos must reach `min_per_class`. */
export function tasteNeeds(t: Pick<Taste, 'positives' | 'negatives' | 'min_per_class'>): TasteNeeds {
  const min = Math.max(1, t.min_per_class)
  const pos = Math.max(0, Math.floor(t.positives))
  const neg = Math.max(0, Math.floor(t.negatives))
  return {
    keep: Math.max(0, min - pos),
    reject: Math.max(0, min - neg),
    fraction: (Math.min(pos, min) + Math.min(neg, min)) / (2 * min),
  }
}

/** AUC lead of the taste model over the generic score, in percentage points (rounded). */
export function tasteLead(m: TasteMetrics): number {
  return Math.round((m.auc - m.base_auc) * 100)
}

/** i18n key + params of a taste feature's name (`scene_<x>` reuses the scene names). */
export function tasteFeatureLabel(feature: string): { key: string; params?: Record<string, string> } {
  if (feature.startsWith('scene_')) return { key: 'taste.feat.scene', params: { scene: feature.slice(6) } }
  return { key: `taste.feat.${feature}` }
}

/** Splits an explanation into what speaks for and against the photo (largest first). */
export function splitContributions(cs: TasteContribution[]): { pro: TasteContribution[]; con: TasteContribution[] } {
  const pro = cs.filter((c) => c.delta > 0).sort((a, b) => b.delta - a.delta)
  const con = cs.filter((c) => c.delta < 0).sort((a, b) => a.delta - b.delta)
  return { pro, con }
}

/** Percentage shown for a 0..1 taste score. */
export function tastePercent(score: number): number {
  return Math.round(Math.min(1, Math.max(0, score)) * 100)
}

/** i18n keys (and params) of the status line for a taste state. */
export function tasteStateText(d: Pick<Taste, 'state' | 'alpha' | 'positives' | 'negatives' | 'min_per_class'>): {
  title: string
  hint: string
  params?: Record<string, number>
} {
  switch (d.state) {
    case 'disabled':
    case 'ready':
    case 'not_better':
      return { title: `taste.state_${d.state}`, hint: `taste.state_${d.state}_hint` }
    case 'active':
      return { title: 'taste.active', hint: 'taste.activeHint', params: { alpha: Math.round(d.alpha * 100) } }
    default: {
      const n = tasteNeeds(d)
      return { title: 'taste.inactive', hint: 'taste.needs', params: { keep: n.keep, reject: n.reject } }
    }
  }
}
