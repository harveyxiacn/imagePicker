import type { Photo } from '@/api/types'
import { filterAi, sortAi } from './ai'

// ---------- query ----------
export function filterPhotos(photos: Photo[], q: URLSearchParams): Photo[] {
  const rating = q.get('rating_gte')
  const flag = q.get('flag')
  const color = q.get('color_label')
  let out = photos.filter((p) => {
    if (rating && Number(rating) > 0 && (p.user_rating ?? -1) < Number(rating)) return false
    if (flag === 'picked' && p.flag !== 1) return false
    if (flag === 'rejected' && p.flag !== -1) return false
    if (flag === 'unflagged' && p.flag !== 0) return false
    if (flag === 'not_rejected' && p.flag === -1) return false
    if (color && p.color_label !== color) return false
    if (q.get('has_edits') === '1' && !p.has_edits) return false
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

