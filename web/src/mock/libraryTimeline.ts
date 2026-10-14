import { delay, http, HttpResponse } from 'msw'
import type { Photo, TimelineBucket } from '@/api/types'
import { allSessions, getSession } from './db'
import { filterPhotos } from './query'

const DAY = 86_400_000
const REPS = 4
const ymd = (day: number) => new Date(day * DAY).toISOString().slice(0, 10)

/** Mirrors `ip-core::library_timeline`: local calendar dates, best photos first, undated apart. */
function bucketOf(day: number, g: string): { key: string; from: string; to: string } {
  const d = new Date(day * DAY)
  const y = d.getUTCFullYear()
  const m = d.getUTCMonth() + 1
  const pad = (n: number) => String(n).padStart(2, '0')
  if (g === 'year') return { key: `${y}`, from: `${y}-01-01`, to: `${y}-12-31` }
  if (g === 'month') return { key: `${y}-${pad(m)}`, from: `${y}-${pad(m)}-01`, to: ymd(Date.UTC(y, m, 0) / DAY) }
  return { key: ymd(day), from: ymd(day), to: ymd(day) }
}

const rank = (p: Photo): [number, number, number] => [p.flag === -1 ? 0 : 1, p.user_rating ?? 0, p.ai_rating ?? 0]
const better = (a: Photo, b: Photo) => {
  const [ra, rb] = [rank(a), rank(b)]
  for (let i = 0; i < 3; i++) if (ra[i] !== rb[i]) return rb[i] - ra[i]
  return a.id - b.id
}

export const libraryTimelineHandlers = [
  http.get('/api/timeline', async ({ request }) => {
    await delay(20)
    const q = new URL(request.url).searchParams
    const g = q.get('granularity') ?? 'month'
    if (!['year', 'month', 'day'].includes(g)) return HttpResponse.json({ error: { code: 'bad_request', message: 'granularity must be year|month|day' } }, { status: 400 })
    const sid = q.get('session_id')
    let sessions = allSessions()
    if (sid !== null) {
      const s = getSession(Number(sid))
      if (!s) return HttpResponse.json({ error: { code: 'not_found', message: 'session not found' } }, { status: 404 })
      sessions = [s]
    }
    const seen = new Set<number>()
    const photos = sessions.flatMap((s) => filterPhotos(s.photos, q)).filter((p) => !seen.has(p.id) && !!seen.add(p.id))
    const from = q.get('from')
    const to = q.get('to')
    const groups = new Map<string, { b: TimelineBucket; ps: Photo[] }>()
    let undated = 0
    for (const p of photos) {
      if (p.taken_at === null) {
        if (!from && !to) undated++
        continue
      }
      const day = Math.floor((p.taken_at + (p.taken_at_offset_min ?? 0) * 60_000) / DAY)
      const date = ymd(day)
      if ((from && date < from) || (to && date > to)) continue
      const { key, from: f, to: t } = bucketOf(day, g)
      const e = groups.get(key) ?? { b: { key, from: f, to: t, count: 0, photo_ids: [] }, ps: [] }
      e.b.count++
      e.ps.push(p)
      groups.set(key, e)
    }
    const buckets = [...groups.values()]
      .sort((a, b) => (a.b.key < b.b.key ? -1 : 1))
      .map(({ b, ps }) => ({ ...b, photo_ids: ps.sort(better).slice(0, REPS).map((p) => p.id) }))
    const dated = buckets.reduce((n, b) => n + b.count, 0)
    return HttpResponse.json({ granularity: g, from, to, total: dated + undated, buckets, undated: { count: undated, photo_ids: [] }, events: [] })
  }),
]
