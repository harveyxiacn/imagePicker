import type { Device, FilterOptions, Photo } from '@/api/types'
import { splitCsvText } from '@/lib/csvText'
import { MOCK_DEVICES } from './db'
import { filterAi, sortAi } from './ai'

// ---------- query ----------
/** `YYYY-MM-DD` (or ms) -> ms; `end` = last millisecond of that day. */
function dayStart(v: string | null, end: boolean): number | null {
  if (!v) return null
  if (/^-?\d+$/.test(v)) return Number(v)
  const t = Date.parse(`${v}T00:00:00Z`)
  return Number.isNaN(t) ? null : end ? t + 86_400_000 - 1 : t
}

/** Sharpness state as the server derives it: unanalysed = unknown, `blurry` issue = blurry, else sharp. */
export function mockSharp(p: Photo): 'sharp' | 'blurry' | 'unknown' {
  if (!p.analyzed) return 'unknown'
  return p.issues.includes('blurry') ? 'blurry' : 'sharp'
}

export function filterPhotos(photos: Photo[], q: URLSearchParams): Photo[] {
  const rating = q.get('rating_gte')
  const flag = q.get('flag')
  const color = q.get('color_label')
  const devices = q.get('device')?.split(',').filter(Boolean) ?? null
  const lenses = splitCsvText(q.get('lens'))
  const from = dayStart(q.get('taken_from'), false)
  const to = dayStart(q.get('taken_to'), true)
  const sharp = q.get('sharp')
  // the mock has no keywords or face embeddings: a keyword filter matches nothing, face_ids is ignored
  const tags = splitCsvText(q.get('tags'))
  let out = photos.filter((p) => {
    if (rating && Number(rating) > 0 && (p.user_rating ?? -1) < Number(rating)) return false
    if (flag === 'picked' && p.flag !== 1) return false
    if (flag === 'rejected' && p.flag !== -1) return false
    if (flag === 'unflagged' && p.flag !== 0) return false
    if (flag === 'not_rejected' && p.flag === -1) return false
    if (color && p.color_label !== color) return false
    if (q.get('has_edits') === '1' && !p.has_edits) return false
    if (q.get('duplicates') === '1' && p.duplicate_group_id == null) return false
    if (devices && !devices.some((d) => (d === 'none' ? p.device_id === null : p.device_id === Number(d)))) return false
    if (lenses.length && !(p.lens !== null && lenses.includes(p.lens))) return false
    if (from !== null && (p.taken_at === null || p.taken_at < from)) return false
    if (to !== null && (p.taken_at === null || p.taken_at > to)) return false
    if (tags.length) return false
    if (sharp && mockSharp(p) !== sharp) return false
    return true
  })
  out = filterAi(out, q)
  const sort = q.get('sort') ?? 'taken_at'
  const byId = (a: Photo, b: Photo) => a.id - b.id
  if (sort === 'taken_at')
    out = [...out].sort((a, b) => (a.taken_at ?? Infinity) - (b.taken_at ?? Infinity) || byId(a, b))
  else if (sort === '-taken_at')
    out = [...out].sort((a, b) => (b.taken_at ?? -Infinity) - (a.taken_at ?? -Infinity) || byId(a, b))
  else if (sort === 'name') out = [...out].sort((a, b) => a.file_name.localeCompare(b.file_name) || byId(a, b))
  else if (sort === 'ai') out = sortAi(out)
  else if (sort === 'rating')
    out = [...out].sort((a, b) => (b.user_rating ?? -1) - (a.user_rating ?? -1) || byId(a, b))
  return out
}


/** Devices present in a photo list, sorted by photo count desc (mirrors GET /api/sessions/:id/devices). */
export function deviceList(photos: Photo[]): Device[] {
  const counts = new Map<number, number>()
  for (const p of photos) if (p.device_id !== null) counts.set(p.device_id, (counts.get(p.device_id) ?? 0) + 1)
  return MOCK_DEVICES.filter((d) => counts.has(d.id))
    .map((d) => ({ ...d, photo_count: counts.get(d.id) ?? 0 }))
    .sort((a, b) => b.photo_count - a.photo_count || a.id - b.id)
}

/** Lenses and capture span of a photo list (mirrors GET /api/sessions/:id/filter-options; no keywords in the mock). */
export function filterOptions(photos: Photo[]): FilterOptions {
  const lenses = new Map<string, number>()
  let min: number | null = null
  let max: number | null = null
  for (const p of photos) {
    if (p.lens) lenses.set(p.lens, (lenses.get(p.lens) ?? 0) + 1)
    if (p.taken_at !== null) {
      min = min === null ? p.taken_at : Math.min(min, p.taken_at)
      max = max === null ? p.taken_at : Math.max(max, p.taken_at)
    }
  }
  return {
    lenses: [...lenses].map(([lens, photo_count]) => ({ lens, photo_count })).sort((a, b) => b.photo_count - a.photo_count || a.lens.localeCompare(b.lens)),
    tags: [],
    taken_min: min,
    taken_max: max,
  }
}
