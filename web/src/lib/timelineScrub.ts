/**
 * Timeline scrubber (docs/09 §5.3 P2, PRD F1.9): pure logic of the year -> month -> day zoom and of
 * the bucket <-> capture-date-range mapping. The buckets come from `GET /api/timeline` (local
 * calendar dates); a click on one sets the `taken_from` / `taken_to` filter of the photo grid to
 * the bucket's `from` / `to` dates.
 */
import type { LibraryTimeline, TimelineBucket, TimelineGranularity, TimelineQuery } from '@/api/types'
import { buildPhotosQuery, type FilterState } from './filter'

/** The window the scrubber shows: its level and, below the year level, the dates it is limited to. */
export interface Scope {
  granularity: TimelineGranularity
  from: string | null
  to: string | null
}

export const ROOT_SCOPE: Scope = { granularity: 'year', from: null, to: null }

type Range = Pick<TimelineBucket, 'from' | 'to'>
export type DateRange = Pick<FilterState, 'takenFrom' | 'takenTo'>

const FINER: Record<TimelineGranularity, TimelineGranularity | null> = { year: 'month', month: 'day', day: null }

export const finerGranularity = (g: TimelineGranularity): TimelineGranularity | null => FINER[g]

/** The scope that shows the buckets inside `b` (a year shows its months, a month its days); null for a day. */
export function zoomInScope(b: Range, g: TimelineGranularity): Scope | null {
  const next = FINER[g]
  return next ? { granularity: next, from: b.from, to: b.to } : null
}

/** One level up: days of a month -> months of that year -> all years. */
export function zoomOutScope(s: Scope): Scope {
  if (s.granularity === 'day' && s.from) {
    const y = s.from.slice(0, 4)
    return { granularity: 'month', from: `${y}-01-01`, to: `${y}-12-31` }
  }
  return ROOT_SCOPE
}

/** The grid filter of a bucket. */
export const bucketRange = (b: Range): DateRange => ({ takenFrom: b.from, takenTo: b.to })

/** Is the grid filtered to exactly this bucket? */
export const isBucketActive = (b: Range, f: DateRange): boolean => f.takenFrom === b.from && f.takenTo === b.to

/** Click on a bucket: filter to it, or clear the range when it already is the filter. */
export const toggleBucketRange = (b: Range, f: DateRange): DateRange => (isBucketActive(b, f) ? { takenFrom: null, takenTo: null } : bucketRange(b))

/** Key of the bucket the grid is filtered to, if the range is exactly one of them. */
export const activeBucketKey = (buckets: readonly TimelineBucket[], f: DateRange): string | null => buckets.find((b) => isBucketActive(b, f))?.key ?? null

const utc = (y: number, m: number, d: number) => Date.UTC(y, m - 1, d)

/** `2024` / `2024-05` / `2024-05-03` -> a UTC instant of its first day; null for anything else. */
export function keyDate(key: string): number | null {
  const m = /^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/.exec(key)
  return m ? utc(Number(m[1]), Number(m[2] ?? 1), Number(m[3] ?? 1)) : null
}

/** Full label of a bucket: `2024`, `May 2024`, `May 3, 2024` (locale dependent). */
export function bucketLabel(key: string, g: TimelineGranularity, locale: string): string {
  const t = keyDate(key)
  if (t === null || g === 'year') return key
  const opts: Intl.DateTimeFormatOptions = g === 'month' ? { year: 'numeric', month: 'long', timeZone: 'UTC' } : { dateStyle: 'medium', timeZone: 'UTC' }
  return new Intl.DateTimeFormat(locale, opts).format(t)
}

/** Short label of a tile: the year, the month name, or the day of the month with its weekday. */
export function bucketShortLabel(key: string, g: TimelineGranularity, locale: string): string {
  const t = keyDate(key)
  if (t === null || g === 'year') return key
  const opts: Intl.DateTimeFormatOptions = g === 'month' ? { month: 'short', timeZone: 'UTC' } : { day: 'numeric', weekday: 'short', timeZone: 'UTC' }
  return new Intl.DateTimeFormat(locale, opts).format(t)
}

export interface Crumb {
  /** Scope a click on the crumb goes to; null = the one shown now */
  scope: Scope | null
  granularity: 'all' | TimelineGranularity
  key: string | null
}

/** Breadcrumb of a scope: all years, then the year, then the month it is zoomed to. */
export function breadcrumbs(s: Scope): Crumb[] {
  const out: Crumb[] = [{ scope: s.granularity === 'year' ? null : ROOT_SCOPE, granularity: 'all', key: null }]
  if (s.granularity === 'year' || !s.from) return out
  const year = s.from.slice(0, 4)
  if (s.granularity === 'month') {
    out.push({ scope: null, granularity: 'year', key: year })
    return out
  }
  out.push({ scope: { granularity: 'month', from: `${year}-01-01`, to: `${year}-12-31` }, granularity: 'year', key: year })
  out.push({ scope: null, granularity: 'month', key: s.from.slice(0, 7) })
  return out
}

/** Height of a tile's bar, 0..1: square-root scaled so a few busy days do not flatten the rest. */
export function barRatio(count: number, max: number): number {
  if (max <= 0 || count <= 0) return 0
  return Math.max(0.08, Math.min(1, Math.sqrt(count / max)))
}

/** Index to focus after an arrow key inside the bucket list; null = the key is not a move. */
export function moveFocus(key: string, index: number, length: number): number | null {
  if (length <= 0) return null
  switch (key) {
    case 'ArrowRight':
      return Math.min(index + 1, length - 1)
    case 'ArrowLeft':
      return Math.max(index - 1, 0)
    case 'Home':
      return 0
    case 'End':
      return length - 1
    default:
      return null
  }
}

/** What a key does on a focused tile beyond moving: zoom in / out, or nothing. */
export function zoomKey(key: string): 'in' | 'out' | null {
  if (key === '+' || key === '=' || key === 'ArrowDown') return 'in'
  if (key === '-' || key === 'ArrowUp' || key === 'Backspace') return 'out'
  return null
}

/**
 * Request of the scrubber: the grid's filters (without its capture range, which the scrubber sets,
 * and without the session-only event filter) at the scope's level, for the library or one session.
 */
export function timelineQueryFor(filter: FilterState, scope: Scope, sessionId: number | null): TimelineQuery {
  const q = buildPhotosQuery(0, { ...filter, takenFrom: null, takenTo: null, eventId: null })
  const rest: Record<string, unknown> = { ...q }
  delete rest.session_id
  delete rest.sort
  const out: TimelineQuery = { ...(rest as TimelineQuery), granularity: scope.granularity }
  if (scope.from) out.from = scope.from
  if (scope.to) out.to = scope.to
  if (sessionId !== null) out.session_id = sessionId
  return out
}

/** Photos in the dated buckets of a response. */
export const datedCount = (t: Pick<LibraryTimeline, 'buckets'>): number => t.buckets.reduce((n, b) => n + b.count, 0)
