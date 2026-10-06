/** Personalised scoring (docs/api-contract-m4.md C.2): ranking head is retrained every 50 labels. */
export const TASTE_RETRAIN_EVERY = 50
/** Minimum labels before traits / accuracy are meaningful. */
export const TASTE_MIN_LABELS = 50

export interface TasteProgress {
  /** labels still missing until the next automatic retraining */
  toNext: number
  /** 0..1 progress inside the current 50-label window */
  fraction: number
}

export function tasteProgress(labels: number): TasteProgress {
  const l = Math.max(0, Math.floor(labels))
  const within = l % TASTE_RETRAIN_EVERY
  return { toNext: TASTE_RETRAIN_EVERY - within, fraction: within / TASTE_RETRAIN_EVERY }
}
