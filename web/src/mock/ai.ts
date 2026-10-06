import { delay, http, HttpResponse } from 'msw'
import type {
  AnalysisProfile,
  AnalysisStage,
  AnalysisStatus,
  Burst,
  BurstFaces,
  Face,
  GroupsResponse,
  HardwareInfo,
  Issue,
  ModelInfo,
  Person,
  Photo,
  PhotoAnalysis,
  Scene,
  SceneType,
  WorkerState,
} from '@/api/types'
import { avatarSvg, AVATAR_STYLES } from './avatar'
import { emit } from './bus'
import { burstKeyOf, findPhoto, getSession, mulberry32, newThumbVersion } from './db'

const err = (status: number, code: string, message: string) =>
  HttpResponse.json({ error: { code, message } }, { status })
const lat = () => delay(15 + Math.random() * 30)
const clamp01 = (x: number) => Math.min(1, Math.max(0, x))
const r2 = (x: number) => Math.round(x * 100) / 100

// ============================================================================
// Models, worker
// ============================================================================

const MODELS: ModelInfo[] = [
  { id: 'yunet', task: ['face_detect'], size_mb: 0.3, license: 'MIT', noncommercial: false, installed: true, required_for: ['fast', 'standard'] },
  { id: 'mediapipe-face-landmarker', task: ['face_landmarks', 'blendshapes'], size_mb: 4, license: 'Apache-2.0', noncommercial: false, installed: false, required_for: ['fast', 'standard'] },
  { id: 'auraface-v1', task: ['face_identity'], size_mb: 250, license: 'Apache-2.0', noncommercial: false, installed: false, required_for: ['standard'] },
  { id: 'siglip2-base', task: ['embed_image', 'zero_shot'], size_mb: 375, license: 'Apache-2.0', noncommercial: false, installed: false, required_for: ['standard'] },
  { id: 'topiq-nr', task: ['iqa'], size_mb: 120, license: 'Research / NC', noncommercial: true, installed: false, required_for: ['standard'] },
  { id: 'aesthetic-head', task: ['aesthetic'], size_mb: 4, license: 'MIT', noncommercial: false, installed: false, required_for: ['standard'] },
  { id: 'scrfd-10g', task: ['face_detect'], size_mb: 17, license: 'insightface NC', noncommercial: true, installed: false, required_for: [] },
  // M3 AI masks (not part of any analysis profile; fetched on first use of a mask)
  { id: 'sky-seg', task: ['mask_sky'], size_mb: 28, license: 'Apache-2.0', noncommercial: false, installed: false, required_for: [] },
  // M4 portrait geometry (fetched on first use of the beauty panel)
  { id: 'mediapipe-pose-landmarker', task: ['pose'], size_mb: 9, license: 'Apache-2.0', noncommercial: false, installed: false, required_for: [] },
  { id: 'selfie-multiclass', task: ['mask_skin', 'mask_person'], size_mb: 16, license: 'Apache-2.0', noncommercial: false, installed: false, required_for: [] },
  { id: 'sam2-tiny', task: ['mask_subject', 'mask_person'], size_mb: 156, license: 'Apache-2.0', noncommercial: false, installed: false, required_for: [] },
]

/** Ids from `ids` that are not installed yet (used by the M3 mask endpoint to answer 409 models_missing). */
export function missingModelIds(ids: string[]): string[] {
  return MODELS.filter((m) => ids.includes(m.id) && !m.installed).map((m) => m.id)
}

const worker: HardwareInfo['worker'] = {
  state: 'stopped',
  tier: 'T3',
  device: 'cuda:0',
  providers: ['CUDAExecutionProvider', 'CPUExecutionProvider'],
  gpu: { name: 'NVIDIA GeForce RTX 4090', vram_mb: 24564 },
  error: null,
}

function setWorker(state: WorkerState) {
  worker.state = state
  emit({ type: 'worker.status', state, tier: worker.tier, error: worker.error })
}

let modelSeq = 0
function simulateEnsure(ids: string[]): string {
  const task_id = `model-${++modelSeq}`
  const targets = MODELS.filter((m) => ids.includes(m.id) && !m.installed)
  const total = Math.max(1, Math.round(targets.reduce((n, m) => n + m.size_mb, 0) * 1e6))
  let done = 0
  const tick = () => {
    done = Math.min(total, done + Math.max(1_500_000, Math.ceil(total / 14)))
    const finished = done >= total
    emit({ type: 'task.progress', task_id, kind: 'model_download', done, total, state: finished ? 'done' : 'running' })
    if (finished) for (const m of targets) m.installed = true
    else setTimeout(tick, 220)
  }
  setTimeout(tick, 150)
  return task_id
}

// ============================================================================
// Ground truth + per-photo analysis
// ============================================================================

const PEOPLE_NAMES: (string | null)[] = ['小明', '小红', '阿强', '妈妈', '爸爸', '小美', '老王', null, null]

interface PersonRec {
  id: number
  name: string | null
  hidden: boolean
  /** avatar style index */
  style: number
}
const people = new Map<number, PersonRec>()
function resetPeople() {
  people.clear()
  PEOPLE_NAMES.forEach((name, i) => people.set(i + 1, { id: i + 1, name, hidden: false, style: i }))
}
resetPeople()
let nextPersonId = 100

interface FaceRec extends Face {
  /** ground-truth identity (index into PEOPLE_NAMES) */
  pi: number
  /** person id assigned by clustering (null before the clustering stage) */
  assigned: boolean
}
const faceById = new Map<number, FaceRec>()
let nextFaceId = 1

interface PhotoAi {
  profile: AnalysisProfile
  scores: PhotoAnalysis['scores']
  contributions: PhotoAnalysis['contributions']
  reasons: PhotoAnalysis['reasons']
  faces: FaceRec[]
  sceneType: SceneType
  aiScore: number
  issues: Issue[]
  /** face positions never change; used when rendering the fake photo */
  over: boolean
  under: boolean
}
const photoAi = new Map<number, PhotoAi>()
export const aiOf = (id: number) => photoAi.get(id)

interface TrackTruth {
  pi: number
  x: number
  y: number
  w: number
  bg: boolean
  smileBase: number
  blink: number
  absent: number
}
const truths = new Map<number, { scene: SceneType; tracks: TrackTruth[] }>()
function truthFor(key: number) {
  let t = truths.get(key)
  if (t) return t
  const r = mulberry32(key * 2654435761 + 17)
  const pick = r()
  const scene: SceneType =
    pick < 0.24 ? 'portrait' : pick < 0.52 ? 'group' : pick < 0.72 ? 'landscape' : pick < 0.78 ? 'food' : pick < 0.86 ? 'architecture' : pick < 0.93 ? 'night' : pick < 0.96 ? 'pet' : 'other'
  const tracks: TrackTruth[] = []
  const used = new Set<number>()
  const pickPerson = () => {
    for (;;) {
      const pi = Math.floor(Math.pow(r(), 1.4) * PEOPLE_NAMES.length)
      if (!used.has(pi)) {
        used.add(pi)
        return pi
      }
    }
  }
  const nSubjects = scene === 'portrait' ? 1 : scene === 'group' ? 3 + Math.floor(r() * 3) : scene === 'landscape' ? (r() < 0.15 ? 1 : 0) : 0
  const w = scene === 'portrait' ? 0.2 : scene === 'group' ? 0.13 : 0.1
  for (let i = 0; i < nSubjects; i++) {
    tracks.push({
      pi: pickPerson(),
      x: nSubjects === 1 ? 0.3 + r() * 0.3 : 0.08 + ((i + 0.5) / nSubjects) * 0.84 - w / 2,
      y: 0.2 + r() * 0.2 + (i % 2) * 0.06,
      w,
      bg: false,
      smileBase: 0.2 + r() * 0.7,
      blink: 0.14 + r() * 0.14,
      absent: 0.05,
    })
  }
  if (scene === 'group' && r() < 0.4) {
    tracks.push({ pi: pickPerson(), x: 0.02 + r() * 0.9, y: 0.55 + r() * 0.2, w: 0.045, bg: true, smileBase: 0.3, blink: 0.1, absent: 0.1 })
  }
  t = { scene, tracks }
  truths.set(key, t)
  return t
}

const SCORE_LABELS: Record<string, string> = {
  sharpness: 'score.sharpness',
  exposure: 'score.exposure',
  noise: 'score.noise',
  iqa: 'score.iqa',
  aesthetic: 'score.aesthetic',
  face: 'score.face',
}

function genPhotoAi(p: Photo, profile: AnalysisProfile): PhotoAi {
  const key = burstKeyOf(p.id)
  const truth = truthFor(key)
  const r = mulberry32(p.id * 7919 + 101)
  const kr = mulberry32(key * 31 + 5)
  const blurry = p.id % 11 === 0
  const sharp = blurry ? 0.1 + r() * 0.2 : 0.55 + r() * 0.4
  const over = !blurry && r() < 0.05
  const under = !blurry && !over && r() < 0.05
  const exposure = over || under ? 0.12 + r() * 0.15 : 0.5 + r() * 0.45
  const noise = clamp01(((p.iso ?? 400) / 12800) * 1.4 + r() * 0.12)
  const iqa = clamp01(0.5 * sharp + 0.3 * exposure + 0.2 * (1 - noise))
  const aesthetic = profile === 'standard' ? clamp01(0.3 + kr() * 0.45 + r() * 0.2) : null

  const faces: FaceRec[] = []
  const aspect = (p.width ?? 3) / (p.height ?? 2)
  truth.tracks.forEach((tr) => {
    if (r() < tr.absent) return
    const eyes = r() < tr.blink ? 0.05 + r() * 0.3 : 0.65 + r() * 0.35
    const smile = clamp01(tr.smileBase + (r() - 0.5) * 0.7)
    const gaze = clamp01(0.45 + r() * 0.55)
    const w = tr.w * (0.97 + r() * 0.06)
    const h = Math.min(0.9, w * aspect)
    const fSharp = clamp01(sharp * (0.85 + r() * 0.15))
    const f: FaceRec = {
      id: nextFaceId++,
      photo_id: p.id,
      person_id: null,
      person_name: null,
      bbox: [r2(Math.min(0.98 - w, Math.max(0.01, tr.x + (r() - 0.5) * 0.02))), r2(Math.min(0.98 - h, tr.y + (r() - 0.5) * 0.02)), r2(w), r2(h)],
      eyes_open: r2(eyes),
      smile: r2(smile),
      gaze: r2(gaze),
      yaw: Math.round((r() - 0.5) * 40),
      pitch: Math.round((r() - 0.5) * 20),
      roll: Math.round((r() - 0.5) * 12),
      sharpness: r2(fSharp),
      expression_score: r2(clamp01(0.5 * eyes + 0.35 * smile + 0.15 * gaze)),
      is_subject: !tr.bg,
      pi: tr.pi,
      assigned: false,
    }
    faces.push(f)
  })
  const subjects = faces.filter((f) => f.is_subject)
  const closed = subjects.filter((f) => (f.eyes_open ?? 1) < 0.45)
  const faceScore = subjects.length ? subjects.reduce((n, f) => n + (f.expression_score ?? 0), 0) / subjects.length : null

  // Weighted composite (renormalised over available parts) minus a closed-eyes penalty.
  const parts: [string, number | null, number][] = [
    ['sharpness', sharp, 0.25],
    ['exposure', exposure, 0.15],
    ['iqa', iqa, 0.2],
    ['aesthetic', aesthetic, 0.25],
    ['face', faceScore, 0.15],
  ]
  const avail = parts.filter((x) => x[1] !== null)
  const wsum = avail.reduce((n, x) => n + x[2], 0)
  const contributions = avail.map(([k, v, wt]) => ({ key: k, label_key: SCORE_LABELS[k], delta: r2(((v as number) - 0.5) * (wt / wsum)) }))
  let score = avail.reduce((n, [, v, wt]) => n + (v as number) * (wt / wsum), 0)
  if (closed.length) {
    const pen = Math.min(0.2, 0.1 * closed.length)
    score -= pen
    const fc = contributions.find((c) => c.key === 'face')
    if (fc) fc.delta = r2(fc.delta - pen)
    else contributions.push({ key: 'face', label_key: 'score.face', delta: r2(-pen) })
  }
  score = clamp01(score)
  contributions.sort((a, b) => Math.abs(b.delta) - Math.abs(a.delta))

  const issues: Issue[] = []
  if (closed.length) issues.push('closed_eyes')
  if (sharp < 0.35) issues.push('blurry')
  if (over) issues.push('overexposed')
  if (under) issues.push('underexposed')
  if (noise > 0.55) issues.push('noisy')

  const reasons: PhotoAnalysis['reasons'] = []
  for (const f of closed) {
    const name = PEOPLE_NAMES[f.pi]
    reasons.push({ key: 'closed_eyes', params: name ? { person: name } : {} })
  }
  if (issues.includes('blurry')) reasons.push({ key: 'blurry' })
  if (over) reasons.push({ key: 'overexposed' })
  if (under) reasons.push({ key: 'underexposed' })
  if (issues.includes('noisy')) reasons.push({ key: 'noisy' })
  if (sharp > 0.85) reasons.push({ key: 'sharp' })
  if (subjects.length && subjects.every((f) => (f.smile ?? 0) > 0.6 && (f.eyes_open ?? 0) > 0.6)) reasons.push({ key: 'great_expression' })
  if ((aesthetic ?? 0) > 0.7) reasons.push({ key: 'high_aesthetic' })

  return {
    profile,
    scores: {
      sharpness: r2(sharp),
      exposure: r2(exposure),
      noise: r2(noise),
      iqa: r2(iqa),
      aesthetic: aesthetic === null ? null : r2(aesthetic),
      face: faceScore === null ? null : r2(faceScore),
      composition: null,
    },
    contributions,
    reasons,
    faces,
    sceneType: truth.scene,
    aiScore: r2(score),
    issues,
    over,
    under,
  }
}

const ratingOf = (score: number) => Math.min(5, Math.max(0, Math.round(score * 5 * 2) / 2))

function applyAi(p: Photo, ai: PhotoAi) {
  p.analyzed = true
  p.ai_score = ai.aiScore
  p.ai_rating = ratingOf(ai.aiScore)
  p.issues = ai.issues
  p.scene_type = ai.sceneType
  p.face_count = ai.faces.length
  p.subject_face_count = ai.faces.filter((f) => f.is_subject).length
  // The fake picture now shows its faces: bump the cache key.
  p.thumb_version = newThumbVersion(p.id)
  p.thumb_ready = true
}

// ============================================================================
// Groups (bursts / scenes)
// ============================================================================

interface BurstRec {
  id: number
  session_id: number
  photo_ids: number[] // time order
}
const bursts = new Map<number, BurstRec>()
const sceneStore = new Map<number, Scene[]>()
let nextBurstId = 1
let nextSceneId = 1

function rankBurst(rec: BurstRec): Burst {
  const byScore = [...rec.photo_ids].sort((a, b) => (findPhoto(b)?.ai_score ?? 0) - (findPhoto(a)?.ai_score ?? 0) || a - b)
  byScore.forEach((id, i) => {
    const p = findPhoto(id)
    if (!p) return
    p.burst_id = rec.id
    p.rank_in_burst = i
    p.burst_size = rec.photo_ids.length
  })
  return {
    id: rec.id,
    best_photo_id: byScore[0],
    photo_ids: byScore,
    size: rec.photo_ids.length,
    start_at: findPhoto(rec.photo_ids[0])?.taken_at ?? 0,
  }
}

function regroup(sessionId: number): number[] {
  const s = getSession(sessionId)
  if (!s) return []
  for (const [id, b] of bursts) if (b.session_id === sessionId) bursts.delete(id)
  const analysed = s.photos.filter((p) => p.analyzed).sort((a, b) => (a.taken_at ?? 0) - (b.taken_at ?? 0) || a.id - b.id)
  const byKey = new Map<number, BurstRec>()
  const recs: BurstRec[] = []
  for (const p of analysed) {
    const key = burstKeyOf(p.id)
    let rec = byKey.get(key)
    if (!rec) {
      rec = { id: nextBurstId++, session_id: sessionId, photo_ids: [] }
      byKey.set(key, rec)
      recs.push(rec)
      bursts.set(rec.id, rec)
    }
    rec.photo_ids.push(p.id)
  }
  // Scenes: a gap of more than 100s between consecutive bursts starts a new scene.
  const scenes: Scene[] = []
  let cur: Scene | null = null
  let lastEnd = 0
  for (const rec of recs) {
    const b = rankBurst(rec)
    const first = findPhoto(rec.photo_ids[0])?.taken_at ?? 0
    const last = findPhoto(rec.photo_ids[rec.photo_ids.length - 1])?.taken_at ?? first
    if (!cur || first - lastEnd > 100_000) {
      cur = { id: nextSceneId++, start_at: first, end_at: last, bursts: [] }
      scenes.push(cur)
    }
    cur.bursts.push(b)
    cur.end_at = last
    lastEnd = last
  }
  sceneStore.set(sessionId, scenes)
  return analysed.map((p) => p.id)
}

function groupsOf(sessionId: number): GroupsResponse {
  return { scenes: sceneStore.get(sessionId) ?? [] }
}

function refreshScenes(sessionId: number) {
  // Recompute Burst views from the records (after split/merge).
  const scenes = sceneStore.get(sessionId) ?? []
  for (const sc of scenes) {
    sc.bursts = sc.bursts.filter((b) => bursts.has(b.id)).map((b) => rankBurst(bursts.get(b.id)!))
  }
}

// ============================================================================
// People
// ============================================================================

function clusterPeople(sessionId: number) {
  const s = getSession(sessionId)
  if (!s) return
  for (const p of s.photos) {
    const ai = photoAi.get(p.id)
    if (!ai || ai.profile !== 'standard') continue
    for (const f of ai.faces) {
      if (f.assigned) continue
      f.assigned = true
      f.person_id = f.pi + 1
      f.person_name = people.get(f.person_id)?.name ?? null
    }
  }
}

function facesOfSession(sessionId: number): FaceRec[] {
  const s = getSession(sessionId)
  const out: FaceRec[] = []
  if (!s) return out
  for (const p of s.photos) {
    const ai = photoAi.get(p.id)
    if (ai) out.push(...ai.faces)
  }
  return out
}

function peopleList(sessionId: number | null): Person[] {
  const faces = sessionId === null ? [...faceById.values()] : facesOfSession(sessionId)
  const photosOf = new Map<number, Set<number>>()
  const cover = new Map<number, FaceRec>()
  for (const f of faces) {
    if (f.person_id === null) continue
    let set = photosOf.get(f.person_id)
    if (!set) photosOf.set(f.person_id, (set = new Set()))
    set.add(f.photo_id)
    const c = cover.get(f.person_id)
    const score = (x: FaceRec) => (x.is_subject ? 1 : 0) + (x.eyes_open ?? 0) + (x.smile ?? 0) + (x.sharpness ?? 0) + (x.bbox[2] ?? 0) * 2
    if (!c || score(f) > score(c)) cover.set(f.person_id, f)
  }
  const out: Person[] = []
  for (const [id, set] of photosOf) {
    const rec = people.get(id)
    const c = cover.get(id)
    if (!rec || !c) continue
    out.push({ id, name: rec.name, cover_face_id: c.id, photo_count: set.size, hidden: rec.hidden })
  }
  return out.sort((a, b) => b.photo_count - a.photo_count || a.id - b.id)
}

// ============================================================================
// Analysis run
// ============================================================================

interface Run {
  state: AnalysisStatus['state']
  profile: AnalysisProfile | null
  done: number
  total: number
  stage: AnalysisStage | null
  error: string | null
  cancelled: boolean
}
const runs = new Map<number, Run>()
let runSeq = 0

function emitProgress(sid: number, run: Run) {
  emit({ type: 'analysis.progress', session_id: sid, state: run.state, stage: run.stage, done: run.done, total: run.total })
}

const chunked = <T,>(arr: T[], n: number): T[][] => {
  const out: T[][] = []
  for (let i = 0; i < arr.length; i += n) out.push(arr.slice(i, i + n))
  return out
}

function startRun(sid: number, profile: AnalysisProfile, photoIds?: number[]): string {
  const s = getSession(sid)!
  const run: Run = { state: 'running', profile, done: 0, total: 0, stage: 'analyzing', error: null, cancelled: false }
  runs.set(sid, run)
  const todo = s.photos.filter((p) => (photoIds ? photoIds.includes(p.id) : !p.analyzed))
  run.total = todo.length
  const chunk = profile === 'fast' ? 150 : 90
  const batches = chunked(todo, chunk)
  let bi = 0

  const finish = (state: 'done' | 'idle') => {
    run.state = state
    run.stage = null
    emitProgress(sid, run)
    setWorker('ready')
  }

  const startWorker = () => {
    if (worker.state === 'stopped' || worker.state === 'crashed') {
      setWorker('starting')
      setTimeout(() => {
        setWorker('busy')
        setTimeout(analyze, 150)
      }, 700)
    } else {
      setWorker('busy')
      setTimeout(analyze, 150)
    }
  }

  const analyze = () => {
    if (run.cancelled) return finish('idle')
    if (bi >= batches.length) return group()
    const ids: number[] = []
    for (const p of batches[bi++]) {
      const ai = genPhotoAi(p, profile)
      photoAi.set(p.id, ai)
      for (const f of ai.faces) faceById.set(f.id, f)
      applyAi(p, ai)
      ids.push(p.id)
    }
    run.done += ids.length
    emit({ type: 'analysis.updated', session_id: sid, ids })
    emitProgress(sid, run)
    setTimeout(analyze, 130)
  }

  const group = () => {
    if (run.cancelled) return finish('idle')
    run.stage = 'grouping'
    emitProgress(sid, run)
    setTimeout(() => {
      const ids = regroup(sid)
      emit({ type: 'groups.updated', session_id: sid })
      for (const c of chunked(ids, 500)) emit({ type: 'analysis.updated', session_id: sid, ids: c })
      run.stage = 'scoring'
      emitProgress(sid, run)
      setTimeout(cluster, 450)
    }, 500)
  }

  const cluster = () => {
    if (run.cancelled) return finish('idle')
    run.stage = 'clustering'
    emitProgress(sid, run)
    setTimeout(() => {
      if (profile === 'standard') {
        clusterPeople(sid)
        emit({ type: 'people.updated', session_id: sid })
      }
      finish('done')
    }, 500)
  }

  emitProgress(sid, run)
  startWorker()
  return `analysis-${++runSeq}`
}

function missingModels(profile: AnalysisProfile): string[] {
  return MODELS.filter((m) => m.required_for.includes(profile) && !m.installed).map((m) => m.id)
}

// ============================================================================
// Query filters for GET /api/photos
// ============================================================================

const csv = (v: string | null) => (v ? v.split(',').filter(Boolean) : [])

export function filterAi(photos: Photo[], q: URLSearchParams): Photo[] {
  const aiGte = q.get('ai_rating_gte')
  const issuesNone = q.get('issues_none') === '1'
  const issuesAny = csv(q.get('issues_any'))
  const bestOnly = q.get('burst_best_only') === '1'
  const burstId = q.get('burst_id')
  const scene = q.get('scene_type')
  const persons = csv(q.get('persons')).map(Number)
  const excl = csv(q.get('exclude_persons')).map(Number)
  const states = csv(q.get('person_state'))
  const mode = q.get('person_mode') ?? 'all'
  const bg = q.get('include_background') === '1'
  const fmin = q.get('faces_min')
  const fmax = q.get('faces_max')
  const active = aiGte || issuesNone || issuesAny.length || bestOnly || burstId || scene || persons.length || excl.length || fmin || fmax
  if (!active) return photos
  return photos.filter((p) => {
    if (aiGte && (p.ai_rating ?? -1) < Number(aiGte)) return false
    if (issuesNone && (!p.analyzed || p.issues.length)) return false
    if (issuesAny.length && !p.issues.some((i) => issuesAny.includes(i))) return false
    if (bestOnly && p.burst_id !== null && p.rank_in_burst !== 0) return false
    if (burstId && p.burst_id !== Number(burstId)) return false
    if (scene && p.scene_type !== scene) return false
    if (fmin && (p.subject_face_count ?? -1) < Number(fmin)) return false
    if (fmax && (p.subject_face_count === null || p.subject_face_count > Number(fmax))) return false
    if (persons.length || excl.length) {
      const faces = (photoAi.get(p.id)?.faces ?? []).filter((f) => f.person_id !== null && (bg || f.is_subject))
      const ok = (f: Face) => {
        if (states.includes('eyes_open') && (f.eyes_open ?? 0) < 0.6) return false
        if (states.includes('smiling') && (f.smile ?? 0) < 0.55) return false
        if (states.includes('looking') && (f.gaze ?? 0) < 0.6) return false
        if (states.includes('subject') && !f.is_subject) return false
        return true
      }
      const has = (id: number) => faces.some((f) => f.person_id === id && ok(f))
      if (persons.length) {
        if (mode === 'any' ? !persons.some(has) : !persons.every(has)) return false
      }
      if (excl.some((id) => (photoAi.get(p.id)?.faces ?? []).some((f) => f.person_id === id))) return false
    }
    return true
  })
}

export function sortAi(photos: Photo[]): Photo[] {
  return [...photos].sort((a, b) => (b.ai_score ?? -1) - (a.ai_score ?? -1) || a.id - b.id)
}

// ============================================================================
// Rendering helpers used by svg.ts
// ============================================================================

export function renderableFaces(photoId: number): { face: Face; pi: number }[] {
  return (photoAi.get(photoId)?.faces ?? []).map((f) => ({ face: f, pi: f.pi }))
}
export const photoFlags = (id: number) => ({ over: photoAi.get(id)?.over ?? false, under: photoAi.get(id)?.under ?? false })
export const avatarStyleOf = (pi: number) => AVATAR_STYLES[pi % AVATAR_STYLES.length]

// ============================================================================
// Handlers
// ============================================================================

function toFace(f: FaceRec): Face {
  const { pi: _pi, assigned: _assigned, ...face } = f
  void _pi
  void _assigned
  return face
}

// ============================================================================
// M4 helpers (people of a photo, session faces) used by mock/m4.ts and the mock renderer
// ============================================================================

export interface MockPhotoPerson {
  face_id: number
  person_id: number | null
  person_name: string | null
  face_box: [number, number, number, number]
  is_subject: boolean
  has_pose: boolean
  /** ground-truth identity index (avatar style) */
  pi: number
}

/** First id of the synthetic faces of photos that were never analysed (`SYNTHETIC_FACE_BASE + pi`). */
export const SYNTHETIC_FACE_BASE = 9_000_000

/** People in a photo: analysed faces when present, else the photo's ground-truth tracks (so the beauty panel works before analysis). */
export function mockPeopleOfPhoto(p: Photo): MockPhotoPerson[] {
  const ai = photoAi.get(p.id)
  const hasPose = (box: [number, number, number, number], subject: boolean, pi: number) => subject && box[2] >= 0.11 && !(pi === 6 && p.id % 2 === 1)
  if (ai) {
    return ai.faces.map((f) => ({
      face_id: f.id,
      person_id: f.person_id,
      person_name: f.person_name,
      face_box: f.bbox,
      is_subject: f.is_subject,
      has_pose: hasPose(f.bbox, f.is_subject, f.pi),
      pi: f.pi,
    }))
  }
  const aspect = (p.width ?? 3) / (p.height ?? 2)
  return truthFor(burstKeyOf(p.id)).tracks.map((tr) => {
    const h = Math.min(0.9, tr.w * aspect)
    const box: [number, number, number, number] = [r2(Math.min(0.98 - tr.w, Math.max(0.01, tr.x))), r2(Math.min(0.98 - h, tr.y)), r2(tr.w), r2(h)]
    return {
      face_id: SYNTHETIC_FACE_BASE + tr.pi,
      person_id: tr.pi + 1,
      person_name: people.get(tr.pi + 1)?.name ?? null,
      face_box: box,
      is_subject: !tr.bg,
      has_pose: hasPose(box, !tr.bg, tr.pi),
      pi: tr.pi,
    }
  })
}

export const peopleListOf = (sessionId: number | null): Person[] => peopleList(sessionId)
export const sessionFaces = (sessionId: number): Face[] => facesOfSession(sessionId).map(toFace)
export const personAvatarStyle = (personId: number): number => people.get(personId)?.style ?? personId

export function seedAiMock(): void {
  // nothing to seed: everything is derived on demand
}

export const aiHandlers = [
  http.get('/api/system/hardware', async () => {
    await lat()
    return HttpResponse.json({ worker: { ...worker } })
  }),

  http.get('/api/models', async () => {
    await lat()
    return HttpResponse.json({ models: MODELS.map((m) => ({ ...m })) })
  }),
  http.post('/api/models/ensure', async ({ request }) => {
    await lat()
    const { ids } = (await request.json()) as { ids: string[] }
    return HttpResponse.json({ task_id: simulateEnsure(ids ?? []) }, { status: 202 })
  }),

  http.post('/api/analysis/run', async ({ request }) => {
    await lat()
    const body = (await request.json()) as { session_id: number; profile: AnalysisProfile; photo_ids?: number[] }
    if (!getSession(body.session_id)) return err(404, 'not_found', 'session not found')
    if (runs.get(body.session_id)?.state === 'running') return err(409, 'busy', 'analysis already running')
    const missing = missingModels(body.profile)
    if (missing.length) {
      return HttpResponse.json(
        { error: { code: 'models_missing', message: 'required models are not installed' }, models: missing },
        { status: 409 },
      )
    }
    return HttpResponse.json({ task_id: startRun(body.session_id, body.profile, body.photo_ids) }, { status: 202 })
  }),
  http.post('/api/analysis/cancel', async ({ request }) => {
    const { session_id } = (await request.json()) as { session_id: number }
    const run = runs.get(session_id)
    if (run) run.cancelled = true
    return new HttpResponse(null, { status: 204 })
  }),
  http.get('/api/analysis/status', async ({ request }) => {
    await lat()
    const sid = Number(new URL(request.url).searchParams.get('session_id'))
    const run = runs.get(sid)
    const body: AnalysisStatus = run
      ? { state: run.state, profile: run.profile, done: run.done, total: run.total, stage: run.stage, error: run.error }
      : { state: 'idle', profile: null, done: 0, total: 0, stage: null, error: null }
    return HttpResponse.json(body)
  }),

  http.get('/api/photos/:id/analysis', async ({ params }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const ai = photoAi.get(p.id)
    if (!ai) {
      const empty: PhotoAnalysis = {
        photo_id: p.id,
        analyzed: false,
        profile: null,
        scores: { sharpness: null, exposure: null, noise: null, iqa: null, aesthetic: null, face: null, composition: null },
        ai_score: null,
        ai_rating: null,
        contributions: [],
        reasons: [],
        scene_type: null,
        faces: [],
      }
      return HttpResponse.json(empty)
    }
    const reasons = [...ai.reasons]
    if (p.rank_in_burst === 0 && (p.burst_size ?? 0) > 1) reasons.unshift({ key: 'best_in_burst', params: { n: p.burst_size ?? 0 } })
    const body: PhotoAnalysis = {
      photo_id: p.id,
      analyzed: true,
      profile: ai.profile,
      scores: ai.scores,
      ai_score: ai.aiScore,
      ai_rating: p.ai_rating,
      contributions: ai.contributions,
      reasons,
      scene_type: ai.sceneType,
      faces: ai.faces.map(toFace),
    }
    return HttpResponse.json(body)
  }),

  http.get('/api/groups', async ({ request }) => {
    await lat()
    const sid = Number(new URL(request.url).searchParams.get('session_id'))
    if (!getSession(sid)) return err(404, 'not_found', 'session not found')
    return HttpResponse.json(groupsOf(sid))
  }),
  http.post('/api/groups/split', async ({ request }) => {
    await lat()
    const { burst_id, at_photo_id } = (await request.json()) as { burst_id: number; at_photo_id: number }
    const rec = bursts.get(burst_id)
    if (!rec) return err(404, 'not_found', 'burst not found')
    const idx = rec.photo_ids.indexOf(at_photo_id)
    if (idx <= 0) return err(400, 'bad_request', 'cannot split at the first photo')
    const a: BurstRec = { id: nextBurstId++, session_id: rec.session_id, photo_ids: rec.photo_ids.slice(0, idx) }
    const b: BurstRec = { id: nextBurstId++, session_id: rec.session_id, photo_ids: rec.photo_ids.slice(idx) }
    bursts.delete(rec.id)
    bursts.set(a.id, a)
    bursts.set(b.id, b)
    for (const sc of sceneStore.get(rec.session_id) ?? []) {
      const i = sc.bursts.findIndex((x) => x.id === rec.id)
      if (i >= 0) sc.bursts.splice(i, 1, rankBurst(a), rankBurst(b))
    }
    emit({ type: 'groups.updated', session_id: rec.session_id })
    emit({ type: 'analysis.updated', session_id: rec.session_id, ids: [...a.photo_ids, ...b.photo_ids] })
    return HttpResponse.json({ burst_ids: [a.id, b.id] })
  }),
  http.post('/api/groups/merge', async ({ request }) => {
    await lat()
    const { burst_ids } = (await request.json()) as { burst_ids: number[] }
    const recs = burst_ids.map((id) => bursts.get(id)).filter((x): x is BurstRec => !!x)
    if (recs.length < 2) return err(400, 'bad_request', 'need at least two bursts')
    const sid = recs[0].session_id
    const ids = recs.flatMap((r) => r.photo_ids).sort((x, y) => (findPhoto(x)?.taken_at ?? 0) - (findPhoto(y)?.taken_at ?? 0) || x - y)
    const merged: BurstRec = { id: nextBurstId++, session_id: sid, photo_ids: ids }
    for (const r of recs) bursts.delete(r.id)
    bursts.set(merged.id, merged)
    const scenes = sceneStore.get(sid) ?? []
    let placed = false
    for (const sc of scenes) {
      const keep: Burst[] = []
      for (const b of sc.bursts) {
        if (burst_ids.includes(b.id)) {
          if (!placed) {
            keep.push(rankBurst(merged))
            placed = true
          }
        } else keep.push(b)
      }
      sc.bursts = keep
    }
    for (let i = scenes.length - 1; i >= 0; i--) if (scenes[i].bursts.length === 0) scenes.splice(i, 1)
    refreshScenes(sid)
    emit({ type: 'groups.updated', session_id: sid })
    emit({ type: 'analysis.updated', session_id: sid, ids })
    return HttpResponse.json({ burst_id: merged.id })
  }),

  http.get('/api/bursts/:id/faces', async ({ params }) => {
    await lat()
    const rec = bursts.get(Number(params.id))
    if (!rec) return err(404, 'not_found', 'burst not found')
    const photoIds = rec.photo_ids
    const byPi = new Map<number, Map<number, FaceRec>>()
    for (const pid of photoIds) {
      for (const f of photoAi.get(pid)?.faces ?? []) {
        let m = byPi.get(f.pi)
        if (!m) byPi.set(f.pi, (m = new Map()))
        m.set(pid, f)
      }
    }
    const tracks = [...byPi.entries()]
      .sort((a, b) => {
        const sub = (m: Map<number, FaceRec>) => ([...m.values()][0]?.is_subject ? 0 : 1)
        return sub(a[1]) - sub(b[1]) || a[0] - b[0]
      })
      .map(([pi, m]) => {
        const first = [...m.values()][0]
        const cells: Record<string, Face | null> = {}
        for (const pid of photoIds) cells[String(pid)] = m.get(pid) ? toFace(m.get(pid)!) : null
        const best = [...m.entries()]
          .sort((a, b) => (b[1].expression_score ?? 0) - (a[1].expression_score ?? 0))
          .slice(0, 3)
          .map(([pid]) => pid)
        const pid = first.person_id
        return { track_id: pi + 1, person_id: pid, person_name: pid !== null ? (people.get(pid)?.name ?? null) : null, cells, best_photo_ids: best }
      })
    const body: BurstFaces = { photo_ids: photoIds, tracks }
    return HttpResponse.json(body)
  }),

  http.get('/api/faces/:id/crop', ({ params, request }) => {
    const fid = Number(params.id)
    const s = Number(new URL(request.url).searchParams.get('s') ?? 128)
    if (fid >= SYNTHETIC_FACE_BASE) {
      const pi = fid - SYNTHETIC_FACE_BASE
      return new HttpResponse(avatarSvg(avatarStyleOf(pi), 1, 0.4, s), {
        headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'public, max-age=3600' },
      })
    }
    const f = faceById.get(fid)
    if (!f) return err(404, 'not_found', 'face not found')
    const style = people.get(f.person_id ?? -1)?.style ?? f.pi
    return new HttpResponse(avatarSvg(avatarStyleOf(style), f.eyes_open ?? 1, f.smile ?? 0.3, s), {
      headers: { 'Content-Type': 'image/svg+xml', 'Cache-Control': 'public, max-age=3600' },
    })
  }),

  http.post('/api/photos/accept-ai', async ({ request }) => {
    await lat()
    const { ids } = (await request.json()) as { ids: number[] }
    const items: { id: number; user_rating: number }[] = []
    for (const id of ids) {
      const p = findPhoto(id)
      if (!p || !p.analyzed || p.ai_rating === null) continue
      p.user_rating = Math.round(p.ai_rating)
      items.push({ id, user_rating: p.user_rating })
    }
    if (items.length) emit({ type: 'photos.updated', items })
    return HttpResponse.json({ updated: items.length })
  }),

  http.get('/api/people', async ({ request }) => {
    await lat()
    const sid = new URL(request.url).searchParams.get('session_id')
    return HttpResponse.json({ people: peopleList(sid ? Number(sid) : null) })
  }),
  http.patch('/api/people/:id', async ({ params, request }) => {
    await lat()
    const rec = people.get(Number(params.id))
    if (!rec) return err(404, 'not_found', 'person not found')
    const body = (await request.json()) as { name?: string | null; hidden?: boolean }
    if ('name' in body) rec.name = body.name?.trim() ? body.name.trim() : null
    if (body.hidden !== undefined) rec.hidden = body.hidden
    for (const f of faceById.values()) if (f.person_id === rec.id) f.person_name = rec.name
    const sid = [...runs.keys()][0]
    if (sid !== undefined) emit({ type: 'people.updated', session_id: sid })
    const p = peopleList(null).find((x) => x.id === rec.id)
    return HttpResponse.json({ person: p ?? { id: rec.id, name: rec.name, cover_face_id: 0, photo_count: 0, hidden: rec.hidden } })
  }),
  http.post('/api/people/merge', async ({ request }) => {
    await lat()
    const { ids, into } = (await request.json()) as { ids: number[]; into: number }
    if (!people.has(into)) return err(404, 'not_found', 'person not found')
    for (const id of ids) {
      if (id === into) continue
      for (const f of faceById.values()) {
        if (f.person_id === id) {
          f.person_id = into
          f.person_name = people.get(into)?.name ?? null
        }
      }
      people.delete(id)
    }
    const sid = [...runs.keys()][0]
    if (sid !== undefined) emit({ type: 'people.updated', session_id: sid })
    return HttpResponse.json({ person: peopleList(null).find((x) => x.id === into) })
  }),
  http.post('/api/faces/:id/person', async ({ params, request }) => {
    await lat()
    const f = faceById.get(Number(params.id))
    if (!f) return err(404, 'not_found', 'face not found')
    const { person_id } = (await request.json()) as { person_id: number | null }
    let target = person_id
    if (target === null) {
      target = nextPersonId++
      people.set(target, { id: target, name: null, hidden: false, style: f.pi })
    }
    f.person_id = target
    f.person_name = people.get(target)?.name ?? null
    const sid = [...runs.keys()][0]
    if (sid !== undefined) emit({ type: 'people.updated', session_id: sid })
    return HttpResponse.json({ face: toFace(f) })
  }),
]
