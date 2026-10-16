/**
 * Mock backend for M4 (docs/api-contract-m4.md): portrait geometry readiness, beauty profiles, best-N per person,
 * face search with a fake upload, smart collections, taste, plus the `beauty.ready` / `taste.updated` /
 * `collections.updated` events.
 */
import { delay, http, HttpResponse } from 'msw'
import type { BeautyProfile, Collection, Face, FaceSearchResponse, FaceSearchSimilarFace, Taste, TasteTrait } from '@/api/types'
import { applyProfile } from '@/lib/beauty'
import { missingModelIds, mockPeopleOfPhoto, peopleListOf, sessionFaces } from './ai'
import { isBeautyReady, markBeautyReady } from './beautyState'
import { emit } from './bus'
import { findPhoto, getSession, hashString } from './db'
import { savedStack, store } from './edits'

const err = (status: number, code: string, message: string) => HttpResponse.json({ error: { code, message } }, { status })
const lat = () => delay(15 + Math.random() * 30)

// ---------------------------------------------------------------- beauty geometry + profiles

const profiles = new Map<number, BeautyProfile>()
const preparing = new Set<number>()

/** Models the portrait geometry needs (the pose + skin/person segmentation are fetched on first use). */
const BEAUTY_MODELS = ['mediapipe-face-landmarker', 'mediapipe-pose-landmarker', 'selfie-multiclass']

export function profileOf(personId: number): BeautyProfile | undefined {
  return profiles.get(personId)
}

// ---------------------------------------------------------------- collections

const BUILTIN_COLLECTIONS: Collection[] = [
  { id: 'best_per_group', name: '每组最佳', query: 'burst_best_only=1', builtin: true },
  { id: 'has_closed_eyes', name: '有人闭眼', query: 'issues_any=closed_eyes', builtin: true },
  { id: 'undecided', name: '待定', query: 'flag=unflagged', builtin: true },
  { id: 'edited', name: '已编辑', query: 'has_edits=1', builtin: true },
  { id: 'duplicates', name: '重复', query: 'duplicates=1', builtin: true },
]
const userCollections: Collection[] = []
let collectionSeq = 0

// ---------------------------------------------------------------- taste

let tasteLabels = 84
let tasteEnabled = true
let tasteUpdated: number | null = null
const TASTE_MIN = 30

function taste(): Taste {
  const L = tasteEnabled ? tasteLabels : 0
  // roughly 45% kept, 40% rejected, the rest neutral
  const positives = Math.round(tasteLabels * 0.45)
  const negatives = Math.round(tasteLabels * 0.4)
  const enough = positives >= TASTE_MIN && negatives >= TASTE_MIN
  const trained = enough && tasteUpdated !== null
  const auc = trained ? Math.round((0.62 + Math.min(0.2, tasteLabels / 1500)) * 1000) / 1000 : 0
  const active = tasteEnabled && trained && auc >= 0.66
  const traits: TasteTrait[] = []
  if (active) traits.push({ key: 'likes', params: { feature: 'eyes_open', strength: 0.8 } }, { key: 'dislikes', params: { feature: 'blurry', strength: 0.6 } })
  if (active && L >= 150) traits.push({ key: 'likes', params: { feature: 'scene_food', strength: 0.3 } })
  return {
    labels: tasteLabels,
    positives,
    negatives,
    min_per_class: TASTE_MIN,
    pending: tasteLabels % 50,
    retrain_every: 50,
    enabled: tasteEnabled,
    state: !tasteEnabled ? 'disabled' : active ? 'active' : !enough ? 'collecting' : trained ? 'not_better' : 'ready',
    active,
    alpha: active ? Math.round(Math.min(0.6, (0.6 * tasteLabels) / 200) * 100) / 100 : 0,
    holdout_accuracy: trained ? Math.round((auc - 0.05) * 100) / 100 : null,
    base_accuracy: trained ? 0.58 : null,
    metrics: trained
      ? { auc, base_auc: 0.61, accuracy: Math.round((auc - 0.05) * 100) / 100, base_accuracy: 0.58, margin: 0.05, eval_positives: positives, eval_negatives: negatives, folds: 5, lambda: 0.03, dim: 768, fit_photos: positives + negatives, train_ms: 420 }
      : null,
    traits,
    scored: active ? 1200 : 0,
    feature_version: 2,
    updated_at: tasteUpdated,
  }
}

function emitTaste(): void {
  const t = taste()
  emit({ type: 'taste.updated', labels: t.labels, active: t.active, alpha: t.alpha })
}

/** Mock taste score of a photo while the taste model is on (deterministic per id). */
export function mockTasteScore(id: number): number | undefined {
  if (!taste().active) return undefined
  return Math.round((((id * 2654435761) >>> 0) % 1000) / 10) / 100
}

/** Called by the PATCH /api/photos mock: every rating / flag is a training label. */
export function recordTasteLabels(n: number): void {
  if (n <= 0 || !tasteEnabled) return
  tasteLabels += n
  tasteUpdated = Date.now()
  emitTaste()
}

// ---------------------------------------------------------------- face search

/** `queryFace`: searching with a library face, whose person comes first and which is not among the similar faces. */
function searchResult(seed: number, faceIndex: number, nFaces: number, sessionId: number | null, queryFace?: Face): FaceSearchResponse {
  const faces: FaceSearchResponse['faces_detected'] = Array.from({ length: nFaces }, (_, i) => [
    nFaces === 1 ? 0.34 : 0.1 + (0.8 / nFaces) * i + 0.02,
    0.2 + (i % 2) * 0.08,
    nFaces === 1 ? 0.3 : 0.7 / nFaces - 0.04,
    0.34,
  ])
  const people = peopleListOf(sessionId).filter((p) => !p.hidden)
  const own = queryFace?.person_id
  if (own != null) {
    // the query face's own person ranks first
    const i = people.findIndex((p) => p.id === own)
    if (i > 0) people.unshift(...people.splice(i, 1))
  }
  const k = people.length
  const start = k && own == null ? (seed + faceIndex * 3) % k : 0
  const picked = Array.from({ length: Math.min(4, k) }, (_, i) => people[(start + i) % k])
  const candidates = picked.map((p, i) => ({
    person_id: p.id,
    person_name: p.name,
    similarity: Math.round((0.93 - i * 0.14 - ((seed >> i) % 5) * 0.01) * 100) / 100,
  }))
  const top = candidates[0]
  // the top person's faces, then a couple of look-alikes below the preselection threshold
  const all = sessionFaces(sessionId ?? 1).filter((f) => f.id !== queryFace?.id)
  const near = (personId: number | undefined, n: number, from: number, step: number): FaceSearchSimilarFace[] =>
    all
      .filter((f) => personId !== undefined && f.person_id === personId)
      .slice(0, n)
      .map((f, i) => ({ face_id: f.id, photo_id: f.photo_id, person_id: f.person_id, person_name: f.person_name, similarity: Math.round((from - i * step) * 100) / 100 }))
  const similar = top ? [...near(top.person_id, 7, top.similarity, 0.02), ...near(candidates[1]?.person_id, 2, 0.42, 0.04)] : []
  return { faces_detected: faces, query_face: faceIndex, candidates, similar_faces: similar }
}

// ---------------------------------------------------------------- handlers

export const m4Handlers = [
  http.get('/api/photos/:id/people', async ({ params }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const people = mockPeopleOfPhoto(p).map((x) => ({
      face_id: x.face_id,
      person_id: x.person_id,
      person_name: x.person_name,
      face_box: x.face_box,
      is_subject: x.is_subject,
      has_pose: x.has_pose,
      has_profile: x.person_id !== null && profiles.has(x.person_id),
    }))
    return HttpResponse.json({ people, ready: isBeautyReady(p.id) })
  }),

  http.post('/api/photos/:id/beauty/prepare', async ({ params }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const missing = missingModelIds(BEAUTY_MODELS)
    if (missing.length)
      return HttpResponse.json({ error: { code: 'models_missing', message: 'required models are not installed' }, models: missing }, { status: 409 })
    if (!isBeautyReady(p.id) && !preparing.has(p.id)) {
      preparing.add(p.id)
      setTimeout(() => {
        preparing.delete(p.id)
        markBeautyReady(p.id)
        emit({ type: 'beauty.ready', photo_id: p.id })
      }, 1300)
    }
    return HttpResponse.json({ task_id: `beauty-${p.id}` }, { status: 202 })
  }),

  http.get('/api/people/best', async ({ request }) => {
    await lat()
    const q = new URL(request.url).searchParams
    const sid = Number(q.get('session_id'))
    const ids = (q.get('ids') ?? '').split(',').filter(Boolean).map(Number)
    const n = Math.max(1, Math.min(50, Number(q.get('n') ?? 3)))
    if (!getSession(sid)) return err(404, 'not_found', 'session not found')
    const faces = sessionFaces(sid)
    const out = ids.map((pid) => {
      // Score = this person's expression score x the photo's composite score; one photo per burst.
      const best = new Map<string, { photo_id: number; score: number }>()
      for (const f of faces) {
        if (f.person_id !== pid) continue
        const ph = findPhoto(f.photo_id)
        if (!ph) continue
        const score = Math.round((f.expression_score ?? 0.5) * (ph.ai_score ?? 0.5) * 1000) / 1000
        const key = ph.burst_id !== null ? `b${ph.burst_id}` : `p${ph.id}`
        const cur = best.get(key)
        if (!cur || score > cur.score) best.set(key, { photo_id: ph.id, score })
      }
      const photos = [...best.values()].sort((a, b) => b.score - a.score || a.photo_id - b.photo_id).slice(0, n)
      return { person_id: pid, photos }
    })
    return HttpResponse.json({ people: out })
  }),

  http.post('/api/faces/search', async ({ request }) => {
    await delay(350 + Math.random() * 200)
    const type = request.headers.get('content-type') ?? ''
    if (type.includes('multipart/form-data')) {
      const fd = await request.formData()
      const image = fd.get('image')
      if (!(image instanceof File)) return err(400, 'bad_request', 'image required')
      const sid = fd.get('session_id') ? Number(fd.get('session_id')) : null
      const idxRaw = fd.get('face_index')
      const seed = hashString(`${image.name}:${image.size}`)
      // 0 faces for tiny / non-image-ish uploads, else 1..3 deterministic from the file
      if (image.size < 64) return HttpResponse.json({ faces_detected: [], query_face: 0, candidates: [], similar_faces: [] })
      const nFaces = 1 + (seed % 3)
      const idx = idxRaw !== null ? Math.min(nFaces - 1, Math.max(0, Number(idxRaw))) : 0
      return HttpResponse.json(searchResult(seed, idx, nFaces, sid))
    }
    const body = (await request.json()) as { face_id?: number; session_id?: number }
    if (body.face_id === undefined) return err(400, 'bad_request', 'image or face_id required')
    const face = sessionFaces(body.session_id ?? 1).find((f) => f.id === body.face_id)
    if (!face) return err(404, 'not_found', 'face not found')
    return HttpResponse.json(searchResult(body.face_id, 0, 1, body.session_id ?? null, face))
  }),

  http.get('/api/people/:id/beauty-profile', async ({ params }) => {
    await lat()
    return HttpResponse.json({ profile: profiles.get(Number(params.id)) ?? null })
  }),
  http.put('/api/people/:id/beauty-profile', async ({ params, request }) => {
    await lat()
    const id = Number(params.id)
    const { profile } = (await request.json()) as { profile: BeautyProfile | null }
    if (profile) profiles.set(id, profile)
    else profiles.delete(id)
    return HttpResponse.json({ profile: profile ?? null })
  }),
  http.post('/api/edits/apply-profiles', async ({ request }) => {
    await lat()
    const { photo_ids } = (await request.json()) as { photo_ids: number[] }
    const items: { id: number; has_edits: boolean; thumb_version: string }[] = []
    for (const id of photo_ids ?? []) {
      const p = findPhoto(id)
      if (!p) continue
      let stack = savedStack(id)
      let touched = false
      for (const person of mockPeopleOfPhoto(p)) {
        const prof = person.person_id !== null ? profiles.get(person.person_id) : undefined
        if (!prof || person.person_id === null) continue
        stack = applyProfile(stack, prof, person.person_id)
        touched = true
      }
      if (!touched) continue
      store(p, stack)
      items.push({ id, has_edits: p.has_edits, thumb_version: p.thumb_version })
    }
    if (items.length) emit({ type: 'edits.updated', items })
    return HttpResponse.json({ updated: items.length })
  }),

  http.get('/api/collections', async () => {
    await lat()
    return HttpResponse.json({ collections: [...BUILTIN_COLLECTIONS, ...userCollections] })
  }),
  http.post('/api/collections', async ({ request }) => {
    await lat()
    const { name, query } = (await request.json()) as { name: string; query: string }
    if (!name?.trim()) return err(400, 'bad_request', 'name required')
    const c: Collection = { id: `c${++collectionSeq}`, name: name.trim(), query: query ?? '', builtin: false }
    userCollections.push(c)
    emit({ type: 'collections.updated' })
    return HttpResponse.json({ collection: c }, { status: 201 })
  }),
  http.patch('/api/collections/:id', async ({ params, request }) => {
    await lat()
    const id = String(params.id)
    if (BUILTIN_COLLECTIONS.some((c) => c.id === id)) return err(400, 'bad_request', 'built-in collections cannot be changed')
    const c = userCollections.find((x) => x.id === id)
    if (!c) return err(404, 'not_found', 'collection not found')
    const body = (await request.json()) as { name?: string; query?: string }
    if (body.name !== undefined && body.name.trim()) c.name = body.name.trim()
    if (body.query !== undefined) c.query = body.query
    emit({ type: 'collections.updated' })
    return HttpResponse.json({ collection: c })
  }),
  http.delete('/api/collections/:id', async ({ params }) => {
    await lat()
    const id = String(params.id)
    if (BUILTIN_COLLECTIONS.some((c) => c.id === id)) return err(400, 'bad_request', 'built-in collections cannot be deleted')
    const i = userCollections.findIndex((x) => x.id === id)
    if (i < 0) return err(404, 'not_found', 'collection not found')
    userCollections.splice(i, 1)
    emit({ type: 'collections.updated' })
    return new HttpResponse(null, { status: 204 })
  }),

  http.get('/api/taste', async () => {
    await lat()
    return HttpResponse.json(taste())
  }),
  http.patch('/api/taste', async ({ request }) => {
    await lat()
    const b = (await request.json()) as { enabled?: unknown }
    if (typeof b.enabled !== 'boolean') return err(400, 'bad_request', 'enabled must be a boolean')
    tasteEnabled = b.enabled
    emitTaste()
    return HttpResponse.json(taste())
  }),
  http.post('/api/taste/train', async () => {
    await lat()
    if (!tasteEnabled) return err(400, 'bad_request', 'taste learning is switched off')
    tasteUpdated = Date.now()
    emitTaste()
    return HttpResponse.json(taste())
  }),
  http.get('/api/taste/explain/:id', async ({ params }) => {
    await lat()
    const id = Number(params.id)
    const score = mockTasteScore(id) ?? null
    return HttpResponse.json({
      photo_id: id,
      taste_score: score,
      base_score: 0.6,
      contributions:
        score === null
          ? []
          : [
              { feature: 'eyes_open', delta: 0.42, direction: 'high' },
              { feature: 'look', delta: 0.21, direction: 'high' },
              { feature: 'sharpness', delta: -0.18, direction: 'low' },
            ],
    })
  }),
  http.post('/api/taste/reset', async ({ request }) => {
    await lat()
    const text = await request.text()
    const b = text ? (JSON.parse(text) as { disable?: boolean }) : {}
    tasteLabels = 0
    tasteUpdated = null
    if (b.disable) tasteEnabled = false
    emitTaste()
    return new HttpResponse(null, { status: 204 })
  }),
]
