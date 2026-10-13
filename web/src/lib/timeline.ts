/**
 * Event layer helpers (docs/09 P1-1): labels for the events of `GET /sessions/{id}/timeline`.
 * The server cuts a session into events at pauses > 2 h or moves > 5 km (`ip-core::analysis::timeline`).
 */
import type { TimelineEvent } from '@/api/types'

const pad = (n: number) => String(n).padStart(2, '0')

/** Wall-clock `Date` (read it with the UTC getters) of an instant at the event's UTC offset. */
const wall = (ms: number, offsetMin: number | null) => new Date(ms + (offsetMin ?? 0) * 60_000)

const hm = (d: Date) => `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`
const ymd = (d: Date) => `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`

/** `35.68°N 139.65°E` */
export function formatCoord(lat: number, lon: number): string {
  return `${Math.abs(lat).toFixed(2)}°${lat < 0 ? 'S' : 'N'} ${Math.abs(lon).toFixed(2)}°${lon < 0 ? 'W' : 'E'}`
}

export interface EventLabel {
  /** The title when set, else the local date. */
  primary: string
  /** Local time span (and the place), `10:12–14:40 · 35.68°N 139.65°E`. */
  secondary: string
}

/**
 * Display text of an event: date first, then the time span, then the place (the mean position of its
 * geotagged photos, or the place name once one is set). Dates use the locale, times are 24 h.
 */
export function eventLabel(ev: TimelineEvent, locale: string): EventLabel {
  const s = wall(ev.start_at, ev.offset_min)
  const e = wall(Math.max(ev.end_at, ev.start_at), ev.offset_min)
  const date = (d: Date) => new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeZone: 'UTC' }).format(d)
  const sameDay = ymd(s) === ymd(e)
  const span = sameDay ? (hm(s) === hm(e) ? hm(s) : `${hm(s)}–${hm(e)}`) : `${date(s)} ${hm(s)} – ${date(e)} ${hm(e)}`
  const place = ev.place?.trim() || (ev.gps_lat !== null && ev.gps_lon !== null ? formatCoord(ev.gps_lat, ev.gps_lon) : '')
  const title = ev.title?.trim()
  return {
    primary: title || date(s),
    secondary: [span, place].filter(Boolean).join(' · '),
  }
}

/** Click on an event of the sidebar: select it, or clear the selection when it is the active one. */
export const toggleEvent = (current: number | null | undefined, id: number): number | null => (current === id ? null : id)

/** The selected event is gone (the grouping changed, or another session is open): drop the filter. */
export const eventStillExists = (events: readonly TimelineEvent[], id: number | null | undefined): boolean =>
  typeof id !== 'number' || events.some((e) => e.id === id)

/** Index of the event that contains the capture time `t` (events are in time order); -1 = none. */
export function eventIndexAt(events: readonly TimelineEvent[], t: number): number {
  let lo = 0
  let hi = events.length - 1
  let found = -1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (events[mid].start_at <= t) {
      found = mid
      lo = mid + 1
    } else hi = mid - 1
  }
  return found >= 0 && t <= events[found].end_at ? found : -1
}
