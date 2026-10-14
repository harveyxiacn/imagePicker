import type { ModelInfo, Photo, SearchHit } from '@/api/types'

/** Natural-language photo search (PRD F2.9): pure helpers; the request itself lives in api/client.ts. */

/** The SigLIP2 text tower; search needs it besides the image embeddings of a standard analysis. */
export const TEXT_MODEL = 'siglip2-base-text'
/** Wait this long after the last key press before the search runs. */
export const SEARCH_DEBOUNCE_MS = 400
/** Hits asked for. The grid shows these (best first); `total_matched` says how many more there are. */
export const SEARCH_LIMIT = 300
/** The server refuses longer queries. */
export const MAX_QUERY_CHARS = 256

/** Trimmed, whitespace collapsed, at most MAX_QUERY_CHARS characters. '' = no search. */
export function normalizeSearch(text: string): string {
  const t = text.split(/\s+/).filter(Boolean).join(' ')
  const chars = Array.from(t)
  return chars.length > MAX_QUERY_CHARS ? chars.slice(0, MAX_QUERY_CHARS).join('') : t
}

/** `photos` in the order of `hits` (best first); photos that are not hits are dropped. */
export function orderByHits(photos: Photo[], hits: Pick<SearchHit, 'photo_id'>[]): Photo[] {
  const byId = new Map(photos.map((p) => [p.id, p]))
  const out: Photo[] = []
  for (const h of hits) {
    const p = byId.get(h.photo_id)
    if (p) out.push(p)
  }
  return out
}

export type TextModelState = 'ready' | 'missing' | 'unknown'

/** Is the text model installed? 'unknown' while the model list is not known (loading, or a guest). */
export function textModelState(models: Pick<ModelInfo, 'id' | 'installed'>[] | undefined): TextModelState {
  if (!models) return 'unknown'
  const m = models.find((x) => x.id === TEXT_MODEL)
  if (!m) return 'unknown'
  return m.installed ? 'ready' : 'missing'
}

/** Score of a hit as a percentage for tooltips. */
export const scorePercent = (score: number): number => Math.round(score * 100)
