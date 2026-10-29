import type { Photo } from '@/api/types'

/** Why `B` (group view) cannot open for the cursor photo. */
export type GroupBlock = 'ok' | 'not-analysed' | 'no-similar'

/**
 * Only suggest running analysis when the photo really has none; an analysed photo that sits
 * alone (burst of 1, or no burst) simply has no similar shots.
 */
export function groupViewState(p: Pick<Photo, 'analyzed' | 'burst_id' | 'burst_size'>): GroupBlock {
  if (p.burst_id !== null && (p.burst_size ?? 0) >= 2) return 'ok'
  return p.analyzed ? 'no-similar' : 'not-analysed'
}

/** Photos folded away behind a stack cover: the filtered total minus what the list shows. */
export const foldedAway = (total: number, shown: number): number => Math.max(0, total - shown)
