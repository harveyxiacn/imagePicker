import { delay, http, HttpResponse } from 'msw'
import type { CardImportBody, CollageBody, ExportBody, ExportPreset, ImportBody, PhotoPatchBody, Photo } from '@/api/types'
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
import { aiHandlers, missingModelIds } from './ai'
import { deviceList, filterOptions, filterPhotos } from './query'
import { m6Gate, m6Handlers } from './m6'
import { m8Handlers } from './m8'
import { editHandlers, renderEdited } from './edits'
import { m4Handlers, recordTasteLabels } from './m4'
import { m5Handlers } from './m5'
import { photoSvg } from './svg'
import { endTask, isCancelled, taskHandlers, taskProgress, trackTask } from './tasks'

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
function simulateExport(total: number, params: Record<string, unknown>) {
  const task_id = `export-${++exportSeq}`
  trackTask(task_id, 'export', params, total)
  let done = 0
  const tick = () => {
    if (isCancelled(task_id)) {
      // files written so far stay, the rest is skipped
      endTask(task_id, 'cancelled')
      emit({ type: 'task.progress', task_id, kind: 'export', done, total, state: 'cancelled' })
      return
    }
    done = Math.min(total, done + Math.max(1, Math.ceil(total / 12)))
    taskProgress(task_id, done, total)
    if (done >= total) endTask(task_id, 'done')
    emit({ type: 'task.progress', task_id, kind: 'export', done, total, state: done >= total ? 'done' : 'running' })
    if (done < total) setTimeout(tick, 250)
  }
  setTimeout(tick, 200)
  return task_id
}

let cardSeq = 0
/** Mock of `POST /api/import/card`: scan, copy in steps (cancellable), then import what was copied as a session. */
function simulateCardImport(body: CardImportBody): string {
  const task_id = `card_import-${++cardSeq}`
  const found = 330
  const skipped = 12
  const total = found - skipped
  const params: Record<string, unknown> = { source: body.source, dest: body.dest, pattern: body.pattern ?? '{yyyy}/{yyyy-mm-dd}' }
  trackTask(task_id, 'card_import', params, 0)
  emit({ type: 'task.progress', task_id, kind: 'card_import', done: 0, total: 0, state: 'running' })
  let done = 0
  const finish = (cancelled: boolean) => {
    let session_id: number | null = null
    if (done > 0) {
      session_id = createSessionId()
      const title = body.title || 'EOS_DIGITAL'
      const sess = addSession(session_id, title, `${body.dest}\\2026`, [], 'scanning')
      emit({ type: 'session.updated', session: { ...sess.session } })
      simulateImport(session_id, done, hashString(body.source))
    }
    params.report = {
      found, skipped_existing: skipped, copied: done, reused: 0, bytes: done * 8_200_000, failed_count: 0, failed: [],
      not_attempted: total - done, cancelled, aborted: null, session_id,
    }
    endTask(task_id, cancelled ? 'cancelled' : 'done')
    emit({ type: 'task.progress', task_id, kind: 'card_import', done, total, state: cancelled ? 'cancelled' : 'done' })
  }
  const tick = () => {
    if (isCancelled(task_id)) return finish(true)
    done = Math.min(total, done + 24)
    taskProgress(task_id, done, total)
    if (done >= total) return finish(false)
    emit({ type: 'task.progress', task_id, kind: 'card_import', done, total, state: 'running' })
    setTimeout(tick, 250)
  }
  setTimeout(() => {
    taskProgress(task_id, 0, total)
    emit({ type: 'task.progress', task_id, kind: 'card_import', done: 0, total, state: 'running' })
    setTimeout(tick, 250)
  }, 400)
  return task_id
}

const lat = () => delay(15 + Math.random() * 35)

export const handlers = [
  m6Gate,
  ...aiHandlers,
  ...editHandlers,
  ...m4Handlers,
  ...m6Handlers,
  ...m8Handlers,
  ...m5Handlers,
  ...taskHandlers,

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

  http.get('/api/import/sources', async () => {
    await lat()
    return HttpResponse.json({
      sources: [
        { label: 'EOS_DIGITAL', path: 'G:\\', kind: 'removable', total_bytes: 64_000_000_000, free_bytes: 41_000_000_000, photo_estimate: 318, estimate_truncated: false },
      ],
      default_pattern: '{yyyy}/{yyyy-mm-dd}',
    })
  }),
  http.post('/api/import/card', async ({ request }) => {
    await lat()
    const body = (await request.json()) as CardImportBody
    if (!body.source) return err(400, 'bad_request', 'source must not be empty')
    if (!body.dest) return err(400, 'bad_request', 'dest must not be empty')
    return HttpResponse.json({ task_id: simulateCardImport(body) }, { status: 202 })
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

  // ---- files: rescan, relocate, safe reject ----
  http.post('/api/sessions/:id/rescan', async ({ params }) => {
    await lat()
    return getSession(Number(params.id)) ? HttpResponse.json({ started: true }, { status: 202 }) : err(404, 'not_found', 'session not found')
  }),
  http.post('/api/sessions/:id/relocate', async ({ params, request }) => {
    await lat()
    const s = getSession(Number(params.id))
    if (!s) return err(404, 'not_found', 'session not found')
    const body = (await request.json()) as { path: string; dry_run?: boolean }
    const total = s.photos.length
    const report = { total, matched: total, changed: 0, absent: 0, applied: !body.dry_run }
    if (!body.dry_run) {
      s.session.root_path = body.path
      for (const p of s.photos) {
        p.missing = false
        for (const c of p.companions ?? []) c.missing = false
      }
      emit({ type: 'photos.added', session_id: s.session.id, count: total })
      emit({ type: 'session.updated', session: { ...s.session } })
    }
    return HttpResponse.json({ relocate: report })
  }),
  http.post('/api/photos/reject-files', async ({ request }) => {
    await lat()
    const body = (await request.json()) as { session_id: number; mode: 'trash' | 'folder'; confirm?: boolean }
    const s = getSession(body.session_id)
    if (!s) return err(404, 'not_found', 'session not found')
    if (!body.confirm) return err(400, 'bad_request', 'confirmation required')
    const rejected = s.photos.filter((p) => p.flag === -1)
    if (body.mode === 'trash') for (const p of rejected) p.missing = true
    emit({ type: 'photos.added', session_id: s.session.id, count: rejected.length })
    return HttpResponse.json({ result: { mode: body.mode, photos: rejected.length, files: rejected.length, skipped: 0, errors: [] } })
  }),

  http.get('/api/sessions/:id/devices', async ({ params }) => {
    await lat()
    const s = getSession(Number(params.id))
    if (!s) return err(404, 'not_found', 'session not found')
    return HttpResponse.json({ devices: deviceList(s.photos) })
  }),
  http.get('/api/sessions/:id/filter-options', async ({ params }) => {
    await lat()
    const s = getSession(Number(params.id))
    if (!s) return err(404, 'not_found', 'session not found')
    return HttpResponse.json(filterOptions(s.photos))
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
    // ratings / flags are taste training labels (M4)
    if ('user_rating' in body || 'flag' in body) recordTasteLabels(items.length)
    return HttpResponse.json({ updated: items.length })
  }),

  http.get('/api/thumb/:id', ({ params, request }) => {
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const s = Number(new URL(request.url).searchParams.get('s') ?? 256)
    if (p.has_edits) return renderEdited(p, s)
    return new HttpResponse(photoSvg(p, s, s >= 256 && false), {
      headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'public, max-age=31536000, immutable' },
    })
  }),
  http.get('/api/preview/:id', async ({ params, request }) => {
    await delay(60 + Math.random() * 140)
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const s = Number(new URL(request.url).searchParams.get('s') ?? 2048)
    if (p.has_edits) return renderEdited(p, s)
    return new HttpResponse(photoSvg(p, s, true), {
      headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'public, max-age=31536000, immutable' },
    })
  }),
  http.get('/api/full/:id', async ({ params }) => {
    await delay(150 + Math.random() * 250)
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const s = Math.max(p.width ?? 4096, p.height ?? 4096)
    if (p.has_edits) return renderEdited(p, s)
    return new HttpResponse(photoSvg(p, s, true), {
      headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'public, max-age=3600' },
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

  // mirrors EXPORT_PRESETS in crates/ip-core/src/export.rs (the server owns the table)
  http.get('/api/export/presets', async () => {
    await lat()
    const presets: ExportPreset[] = [
      { id: 'original', long_edge: null, aspect: null, quality: 95, format: 'original', strip_gps: false },
      { id: 'wechat', long_edge: 2048, aspect: null, quality: 85, format: 'jpeg', strip_gps: true },
      { id: 'xiaohongshu', long_edge: 1920, aspect: [3, 4], quality: 90, format: 'jpeg', strip_gps: true },
      { id: 'instagram', long_edge: 1350, aspect: [4, 5], quality: 90, format: 'jpeg', strip_gps: true },
    ]
    return HttpResponse.json({ presets })
  }),

  http.post('/api/export', async ({ request }) => {
    await lat()
    const body = (await request.json()) as ExportBody
    if (!body.dest) return err(400, 'bad_request', 'dest required')
    if (body.upscale) {
      // M5: super-resolution needs the Real-ESRGAN model (409 models_missing -> consent dialog)
      const miss = missingModelIds(['realesrgan-x4'])
      if (miss.length) return HttpResponse.json({ error: { code: 'models_missing', message: 'required models are not installed' }, models: miss }, { status: 409 })
    }
    ;(globalThis as { __lastExport?: unknown }).__lastExport = body
    const total = body.folders ? Object.values(body.folders).reduce((n, ids) => n + ids.length, 0) : (body.ids?.length ?? 0)
    return HttpResponse.json({ task_id: simulateExport(total, { dest: body.dest, long_edge: body.long_edge ?? null, upscale: body.upscale ?? null, count: total }) }, { status: 202 })
  }),

  http.post('/api/export/collage', async ({ request }) => {
    await lat()
    const body = (await request.json()) as CollageBody
    if (!body.dest) return err(400, 'bad_request', 'dest required')
    if (body.ids.length < 2 || body.ids.length > 30) return err(400, 'bad_request', 'a collage takes 2..30 photos')
    ;(globalThis as { __lastCollage?: unknown }).__lastCollage = body
    return HttpResponse.json({ task_id: simulateExport(body.ids.length, { dest: body.dest, count: body.ids.length, collage: body.layout }) }, { status: 202 })
  }),

  http.get('/api/fs/roots', () => HttpResponse.json({ roots: ['C:\\', 'D:\\', 'C:\\Users\\demo'] })),
  http.get('/api/fs/list', async ({ request }) => {
    await lat()
    const path = new URL(request.url).searchParams.get('path') || 'C:\\Users\\demo'
    const dirs = FS[path] ?? []
    // `?ext=png,ttf`: a couple of sample files (watermark logo / font picker)
    const ext = (new URL(request.url).searchParams.get('ext') ?? '').split(',').filter(Boolean)
    const sep = path.includes('/') ? '/' : '\\'
    const files = ext.map((e) => `${path.replace(/[\\/]$/, '')}${sep}sample.${e}`)
    if (!FS[path] && !/^[A-Z]:/.test(path)) return err(404, 'not_found', 'no such directory')
    return HttpResponse.json({
      path,
      parent: parentOf(path),
      dirs,
      image_count: hashString(path) % 3 === 0 ? 0 : 120 + (hashString(path) % 2400),
      ...(files.length ? { files } : {}),
    })
  }),
]
