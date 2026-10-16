/**
 * The AI score is stored and sent as 0-1 (higher is better). The UI always shows it as a
 * whole number 0-100 so "which end is best" is obvious; convert only at the display edge.
 */
export const AI_SCORE_MAX = 100

/** 0-1 score -> integer 0-100 (clamped); null/undefined/NaN -> null. */
export function aiScore100(score: number | null | undefined): number | null {
  if (score === null || score === undefined || !Number.isFinite(score)) return null
  return Math.min(AI_SCORE_MAX, Math.max(0, Math.round(score * AI_SCORE_MAX)))
}

/** "82" for 0.82; "–" when not analysed. */
export function formatAiScore(score: number | null | undefined): string {
  const n = aiScore100(score)
  return n === null ? '–' : String(n)
}

/** "82/100"; "–" when not analysed. */
export function formatAiScoreOf100(score: number | null | undefined): string {
  const n = aiScore100(score)
  return n === null ? '–' : `${n}/${AI_SCORE_MAX}`
}

/** " · AI score 82/100 (higher is better)" suffix for tooltips; empty when there is no score. */
export function aiScoreTip(t: (key: string, opts?: Record<string, unknown>) => string, score: number | null | undefined): string {
  return aiScore100(score) === null ? '' : ` · ${t('score.tooltip', { score: formatAiScore(score) })}`
}
