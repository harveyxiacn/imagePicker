import { delay, http, HttpResponse } from 'msw'
import type { ExportBody, ImportBody, PhotoPatchBody, Photo } from '@/api/types'
import { emit } from './bus'
import {
  addSession,
  allSessions,
  computeCounts,
  createSessionId,
  deleteSession,
  findPhoto,
  generatePhotos,
  getSession,
  hashString,
  newThumbVersion,
} from './db'
import { photoSvg } from './svg'

const err = (status: number, code: string, message: string) =>
  HttpResponse.json({ error: { code, message } }, { status })

// ---------- fake file system ----------
const FS: Record<string, string[]> = {
  'C:\\': ['C:\\Users', 'C:\\Windows', 'C:\\Temp'],
  'C:\\Users': ['C:\\Users\\demo'],
  'C:\\Users\\demo': ['C:\\Users\\demo\\Pictures', 'C:\\Users\\demo\\Desktop', 'C:\\Users\\demo\\Downloads'],
  'C:\\Users\\demo\\Pictures': [
    'C:\\Users\\demo\\Pictures\\Kyoto2026',
    'C:\\Users\\demo\\Pictures\\Birthday',
    'C:\\Users\\demo\\Pictures\\Holiday',
    'C:\\Users\\demo\\Pictures\\Wedding-2025',
    'C:\\Users\\demo\\Pictures\\Misc',
  ],
  'C:\\Users\\demo\\Pictures\\Misc': ['C:\\Users\\demo\\Pictures\\Misc\\Scans', 'C:\\Users\\demo\\Pictures\\Misc\\Phone'],
  'D:\\': ['D:\\Archive', 'D:\\SDCard'],
  'D:\\Archive': ['D:\\Archive\\Stress'],
}

function parentOf(p: string): string | null {
  if (/^[A-Z]:\\?$/.test(p)) return null
  const i = p.lastIndexOf('\\')
  if (i < 0) return null
  const parent = p.slice(0, i)
  return /^[A-Z]:$/.test(parent) ? parent + '\\' : parent
}

// ---------- background simulations ----------
const pendingThumbs = new Set<number>()
let thumbTimer: ReturnType<typeof setTimeout> | null = null

function scheduleThumbs(ids: number[]) {
  for (const id of ids) {
    const p = findPhoto(id)
    if (p && !p.thumb_ready) pendingThumbs.add(id)
  }
  if (thumbTimer || pendingThumbs.size === 0) return
  thumbTimer = setTimeout(() => {
    thumbTimer = null
    const items: { id: number; v: string }[] = []
    for (const id of pendingThumbs) {
      const p = findPhoto(id)
      if (!p) continue
      p.thumb_ready = true
      p.thumb_version = newThumbVersion(id)
      items.push({ id, v: p.thumb_version })
    }
    pendingThumbs.clear()
    if (items.length) emit({ type: 'thumbs.ready', items })
  }, 250)
}

function simulateImport(sessionId: number, total: number, seed: number) {
  const s = getSession(sessionId)
  if (!s) return
  const batch = 250
  let done = 0
  const step = () => {
    const cur = getSession(sessionId)
    if (!cur) return
    const n = Math.min(batch, total - done)
    const fresh = generatePhotos(sessionId, cur.session.root_path, n, seed + done, Date.now() - 30 * 86_400_000 + done * 60_000, 0.2)
    // keep taken_at monotonic across batches
    cur.photos.push(...fresh)
    done += n
    computeCounts(cur)
    emit({ type: 'photos.added', session_id: sessionId, count: n })
    emit({ type: 'session.updated', session: { ...cur.session } })
    if (done < total) setTimeout(step, 450)
    else {
      cur.session.import_state = 'thumbnailing'
      emit({ type: 'session.updated', session: { ...cur.session } })
      finishThumbs(sessionId)
    }
  }
  setTimeout(step, 300)
}

function finishThumbs(sessionId: number) {
  const s = getSession(sessionId)
  if (!s) return
  const pending = s.photos.filter((p) => !p.thumb_ready)
  let i = 0
  const step = () => {
    const chunk = pending.slice(i, i + 150)
    i += chunk.length
    const items = chunk.map((p) => {
      p.thumb_ready = true
      p.thumb_version = newThumbVersion(p.id)
      return { id: p.id, v: p.thumb_version }
    })
    if (items.length) emit({ type: 'thumbs.ready', items })
    if (i < pending.length) setTimeout(step, 200)
    else {
      s.session.import_state = 'ready'
      emit({ type: 'session.updated', session: { ...s.session } })
    }
  }
  setTimeout(step, 400)
}

let exportSeq = 0
function simulateExport(total: number) {
  const task_id = `export-${++exportSeq}`
  let done = 0
  const tick = () => {
    done = Math.min(total, done + Math.max(1, Math.ceil(total / 12)))
    emit({ type: 'task.progress', task_id, kind: 'export', done, total, state: done >= total ? 'done' : 'running' })
    if (done < total) setTimeout(tick, 250)
  }
  setTimeout(tick, 200)
  return task_id
}

// ---------- query ----------
function filterPhotos(photos: Photo[], q: URLSearchParams): Photo[] {
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
    return true
  })
  const sort = q.get('sort') ?? 'taken_at'
  const byId = (a: Photo, b: Photo) => a.id - b.id
  if (sort === 'taken_at')
    out = [...out].sort((a, b) => (a.taken_at ?? Infinity) - (b.taken_at ?? Infinity) || byId(a, b))
  else if (sort === '-taken_at')
    out = [...out].sort((a, b) => (b.taken_at ?? -Infinity) - (a.taken_at ?? -Infinity) || byId(a, b))
  else if (sort === 'name') out = [...out].sort((a, b) => a.file_name.localeCompare(b.file_name) || byId(a, b))
  else if (sort === 'rating')
    out = [...out].sort((a, b) => (b.user_rating ?? -1) - (a.user_rating ?? -1) || byId(a, b))
  return out
}

const lat = () => delay(15 + Math.random() * 35)

export const handlers = [
  http.get('/api/health', () => HttpResponse.json({ ok: true, version: '0.1.0-mock' })),

  http.post('/api/import', async ({ request }) => {
    await lat()
    const body = (await request.json()) as ImportBody
    if (!body.path) return err(400, 'bad_request', 'path required')
    const sid = createSessionId()
    const title = body.title || body.path.split(/[\\/]/).filter(Boolean).pop() || 'Import'
    const total = 600 + (hashString(body.path) % 900)
    const sess = addSession(sid, title, body.path, [], 'scanning')
    emit({ type: 'session.updated', session: { ...sess.session } })
    simulateImport(sid, total, hashString(body.path))
    return HttpResponse.json({ session: { ...sess.session } }, { status: 201 })
  }),

  http.get('/api/sessions', async () => {
    await lat()
    return HttpResponse.json({ sessions: allSessions().map((s) => s.session) })
  }),
  http.get('/api/sessions/:id', async ({ params }) => {
    await lat()
    const s = getSession(Number(params.id))
    return s ? HttpResponse.json({ session: s.session }) : err(404, 'not_found', 'session not found')
  }),
  http.delete('/api/sessions/:id', async ({ params }) => {
    await lat()
    return deleteSession(Number(params.id)) ? new HttpResponse(null, { status: 204 }) : err(404, 'not_found', 'session not found')
  }),

  http.get('/api/photos', async ({ request }) => {
    await lat()
    const q = new URL(request.url).searchParams
    const s = getSession(Number(q.get('session_id')))
    if (!s) return err(404, 'not_found', 'session not found')
    const list = filterPhotos(s.photos, q)
    const offset = q.get('cursor') ? Number(q.get('cursor')) : 0
    const limit = Math.min(5000, Number(q.get('limit') ?? 500))
    const photos = list.slice(offset, offset + limit)
    const next = offset + limit < list.length ? String(offset + limit) : null
    return HttpResponse.json({ photos, total: list.length, next_cursor: next })
  }),
  http.get('/api/photos/:id', ({ params }) => {
    const p = findPhoto(Number(params.id))
    return p ? HttpResponse.json({ photo: p }) : err(404, 'not_found', 'photo not found')
  }),
  http.patch('/api/photos', async ({ request }) => {
    await lat()
    const body = (await request.json()) as PhotoPatchBody
    const items: { id: number; user_rating?: number | null; flag?: Photo['flag']; color_label?: Photo['color_label'] }[] = []
    const touched = new Set<number>()
    for (const id of body.ids) {
      const p = findPhoto(id)
      if (!p) continue
      if ('user_rating' in body) p.user_rating = body.user_rating ?? null
      if ('flag' in body && body.flag !== undefined) p.flag = body.flag
      if ('color_label' in body) p.color_label = body.color_label ?? null
      touched.add(p.session_id)
      items.push({ id, user_rating: p.user_rating, flag: p.flag, color_label: p.color_label })
    }
    for (const sid of touched) {
      const s = getSession(sid)
      if (s) {
        computeCounts(s)
        emit({ type: 'session.updated', session: { ...s.session } })
      }
    }
    emit({ type: 'photos.updated', items })
    return HttpResponse.json({ updated: items.length })
  }),

  http.get('/api/thumb/:id', ({ params, request }) => {
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const s = Number(new URL(request.url).searchParams.get('s') ?? 256)
    return new HttpResponse(photoSvg(p, s, s >= 256 && false), {
      headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'public, max-age=31536000, immutable' },
    })
  }),
  http.get('/api/preview/:id', async ({ params, request }) => {
    await delay(60 + Math.random() * 140)
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const s = Number(new URL(request.url).searchParams.get('s') ?? 2048)
    return new HttpResponse(photoSvg(p, s, true), {
      headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'public, max-age=31536000, immutable' },
    })
  }),
  http.get('/api/original/:id', ({ params }) => {
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    return new HttpResponse(photoSvg(p, 4096, true), { headers: { 'Content-Type': 'image/svg+xml' } })
  }),

  http.post('/api/viewport', async ({ request }) => {
    const { ids } = (await request.json()) as { ids: number[] }
    scheduleThumbs(ids)
    return new HttpResponse(null, { status: 204 })
  }),

  http.post('/api/export', async ({ request }) => {
    await lat()
    const body = (await request.json()) as ExportBody
    if (!body.dest) return err(400, 'bad_request', 'dest required')
    return HttpResponse.json({ task_id: simulateExport(body.ids.length) }, { status: 202 })
  }),

  http.get('/api/fs/roots', () => HttpResponse.json({ roots: ['C:\\', 'D:\\', 'C:\\Users\\demo'] })),
  http.get('/api/fs/list', async ({ request }) => {
    await lat()
    const path = new URL(request.url).searchParams.get('path') || 'C:\\Users\\demo'
    const dirs = FS[path] ?? []
    if (!FS[path] && !/^[A-Z]:/.test(path)) return err(404, 'not_found', 'no such directory')
    return HttpResponse.json({
      path,
      parent: parentOf(path),
      dirs,
      image_count: hashString(path) % 3 === 0 ? 0 : 120 + (hashString(path) % 2400),
    })
  }),
]
