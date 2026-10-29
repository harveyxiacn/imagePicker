/**
 * Mock backend of `GET /api/search` (docs/api-contract-m2.md "语义搜索"). There are no embeddings in the mock: a photo
 * matches when a word of the query appears in its file name or scene type, or (for any other word) with a stable
 * pseudo-random chance, so the grid, pill and empty state can be tried out. Query `zzz` finds nothing; a query without a
 * literal match is flagged `low_confidence` (as the server does when even the best photo is a weak match).
 */
import { delay, http, HttpResponse } from 'msw'
import { getSession, hashString } from './db'
import { filterPhotos } from './query'

const err = (status: number, code: string, message: string) => HttpResponse.json({ error: { code, message } }, { status })

export const searchHandlers = [
  http.get('/api/search', async ({ request }) => {
    await delay(60 + Math.random() * 80)
    const q = new URL(request.url).searchParams
    const text = (q.get('q') ?? '').split(/\s+/).filter(Boolean).join(' ')
    if (!text) return err(400, 'bad_request', 'q is required')
    const sid = q.get('session_id')
    const s = sid ? getSession(Number(sid)) : undefined
    if (sid && !s) return err(404, 'not_found', 'session not found')
    const pool = s ? filterPhotos(s.photos, q) : []
    const words = text.toLowerCase().split(' ')
    const limit = Math.min(5000, Number(q.get('limit') ?? 200))
    const hits = pool
      .map((p) => {
        const hay = `${p.file_name} ${p.scene_type ?? ''}`.toLowerCase()
        const literal = words.some((w) => hay.includes(w))
        const h = (hashString(`${text}|${p.id}`) % 1000) / 1000
        const score = text === 'zzz' ? 0 : literal ? 0.9 + h * 0.09 : h < 0.12 ? 0.3 + h : 0
        return { photo_id: p.id, score: Math.round(score * 10000) / 10000, similarity: Math.round((0.1 + score * 0.2) * 10000) / 10000 }
      })
    const minScore = q.get('min_score')
    const cut = minScore === null ? 0.003 : Number(minScore)
    const ranked = hits.filter((h) => h.score >= cut).sort((a, b) => b.score - a.score || a.photo_id - b.photo_id)
    return HttpResponse.json({
      query: text,
      model: 'siglip2-base-text',
      threshold: minScore === null ? 'adaptive' : 'min_score',
      min_score: cut,
      min_similarity: minScore === null ? 0.095 : null,
      low_confidence: pool.length > 0 && (ranked[0]?.score ?? 0) < 0.9,
      hits: ranked.slice(0, limit),
      total_matched: ranked.length,
      searched: pool.length,
      unindexed: 0,
    })
  }),
]
