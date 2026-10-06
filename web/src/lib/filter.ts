import type { ColorName, FlagFilter, PhotosQuery, SortKey } from '@/api/types'

export interface FilterState {
  /** Minimum user rating (0 = no rating filter). */
  ratingGte: number
  flag: 'all' | FlagFilter
  color: ColorName | null
  sort: SortKey
}

export const DEFAULT_FILTER: FilterState = {
  ratingGte: 0,
  flag: 'all',
  color: null,
  sort: 'taken_at',
}

export type PhotosListQuery = Omit<PhotosQuery, 'cursor' | 'limit'>

/** Translate UI filter state to /api/photos query params. Defaults are omitted. */
export function buildPhotosQuery(sessionId: number, f: FilterState): PhotosListQuery {
  const q: PhotosListQuery = { session_id: sessionId }
  if (f.ratingGte > 0) q.rating_gte = f.ratingGte
  if (f.flag !== 'all') q.flag = f.flag
  if (f.color) q.color_label = f.color
  if (f.sort !== DEFAULT_FILTER.sort) q.sort = f.sort
  return q
}

export function isFilterActive(f: FilterState): boolean {
  return f.ratingGte > 0 || f.flag !== 'all' || f.color !== null
}
