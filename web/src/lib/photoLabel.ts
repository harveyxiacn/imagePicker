import type { Photo } from '@/api/types'
import { issueBadges } from './ai'

type T = (key: string, opts?: Record<string, unknown>) => string

export type LabelPhoto = Pick<
  Photo,
  'file_name' | 'user_rating' | 'ai_rating' | 'flag' | 'color_label' | 'has_edits' | 'issues' | 'missing' | 'duplicate_group_id' | 'duplicate_kind'
>

/**
 * Accessible name of one thumbnail (grid cell, filmstrip item): file name first, then every status the
 * visual badges carry, so nothing is conveyed by colour or a glyph alone. `stack` is the burst the cell
 * belongs to (null / undefined when it is not stacked).
 */
export function photoLabel(p: LabelPhoto, t: T, stack?: { count: number } | null): string {
  const parts: string[] = [p.file_name]
  if (p.user_rating) parts.push(p.user_rating === 1 ? t('a11y.starOne') : t('inspector.starN', { n: p.user_rating }))
  else if (p.ai_rating != null) parts.push(t('a11y.aiRating', { n: p.ai_rating.toFixed(1) }))
  if (p.flag === 1) parts.push(t('flag.picked'))
  else if (p.flag === -1) parts.push(t('flag.rejected'))
  if (p.color_label) parts.push(t('a11y.colorLabel', { c: t(`color.${p.color_label}`) }))
  if (p.has_edits) parts.push(t('edit.edited'))
  if (p.duplicate_group_id != null) parts.push(t(p.duplicate_kind === 'exact' ? 'duplicates.exact' : 'duplicates.near'))
  if (p.missing) parts.push(t('files.missing'))
  if (stack && stack.count > 1) parts.push(t('a11y.stack', { n: stack.count }))
  for (const b of issueBadges(p.issues)) parts.push(t(`issue.${b.label}`))
  return parts.join(t('a11y.sep'))
}

/** Name of a colour label for tooltips / dots (the dot itself carries no text). */
export const colorLabelName = (c: NonNullable<Photo['color_label']>, t: T): string => t(`color.${c}`)
