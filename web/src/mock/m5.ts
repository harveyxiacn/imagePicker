/**
 * Mock backend for M5 (docs/api-contract-m5.md): best take plans + compositing, bystander detection, inpainting,
 * denoise / face restore, patch assets, and the `task.progress` / `*.done` / `edits.updated` events.
 *
 * Patch assets are real RGBA PNGs generated on a canvas so the mock renderer visibly composites them:
 *  - best take  : the SOURCE frame's face pasted over the base face (radial feather)
 *  - inpaint    : neighbouring "background" of the base frame, blurred, masked by the bbox / brush strokes
 *  - denoise    : the whole frame, smoothed
 *  - face restore: the face region with contrast / saturation lifted
 *
 * Debug switches (set on `globalThis` from tests): `__m5StepMs` task step duration, `__m5Fail` = '503' | 'task' (one-shot).
 */
import { delay, http, HttpResponse } from 'msw'
import type {
  BestTakeChoice,
  BestTakePerson,
  BestTakePlan,
  BestTakeResult,
  BestTakeWarning,
  EditStack,
  EnhanceBody,
  InpaintBody,
  PatchOp,
  Photo,
  Stroke,
} from '@/api/types'
import { addPatch, replaceByKind } from '@/lib/patches'
import { aiOf, burstRecOf, faceRecOf, missingModelIds, mockPeopleOfPhoto } from './ai'
import { assetBlob, putAsset } from './assets'
import { emit } from './bus'
import { findPhoto } from './db'
import { savedStack, store } from './edits'
import { photoSvg } from './svg'
import { endTask, isCancelled, taskProgress, trackTask } from './tasks'

const err = (status: number, code: string, message: string) => HttpResponse.json({ error: { code, message } }, { status })
const lat = () => delay(15 + Math.random() * 30)
const missing409 = (models: string[]) =>
  HttpResponse.json({ error: { code: 'models_missing', message: 'required models are not installed' }, models }, { status: 409 })

type Box = [number, number, number, number]
const clamp01 = (v: number) => Math.min(1, Math.max(0, v))
const r4 = (v: number) => Math.round(v * 1e4) / 1e4

interface DebugFlags {
  __m5StepMs?: number
  __m5Fail?: '503' | 'task' | null
}
const dbg = () => globalThis as unknown as DebugFlags
/** One-shot failure injection. */
function takeFail(kind: '503' | 'task'): boolean {
  if (dbg().__m5Fail !== kind) return false
  dbg().__m5Fail = null
  return true
}

// ---------------------------------------------------------------- canvas helpers

async function photoImage(p: Photo, longEdge: number): Promise<HTMLImageElement> {
  const url = URL.createObjectURL(new Blob([photoSvg(p, longEdge, false)], { type: 'image/svg+xml' }))
  const img = new Image()
  img.src = url
  await img.decode()
  return img
}

/** Grow a normalised box by `m` of its size on every side, clamped to the image. */
function expand(b: Box, m: number): Box {
  const x0 = clamp01(b[0] - b[2] * m)
  const y0 = clamp01(b[1] - b[3] * m)
  const x1 = clamp01(b[0] + b[2] * (1 + m))
  const y1 = clamp01(b[1] + b[3] * (1 + m))
  return [x0, y0, x1 - x0, y1 - y0]
}

/** Opaque centre, transparent rim: the blend mask of face-shaped patches. */
function featherEllipse(ctx: OffscreenCanvasRenderingContext2D, w: number, h: number, inner = 0.6) {
  ctx.save()
  ctx.globalCompositeOperation = 'destination-in'
  ctx.translate(w / 2, h / 2)
  ctx.scale(w / 2, h / 2)
  const g = ctx.createRadialGradient(0, 0, 0, 0, 0, 1)
  g.addColorStop(0, 'rgba(0,0,0,1)')
  g.addColorStop(inner, 'rgba(0,0,0,1)')
  g.addColorStop(1, 'rgba(0,0,0,0)')
  ctx.fillStyle = g
  ctx.fillRect(-1, -1, 2, 2)
  ctx.restore()
}

const dims = (p: Photo) => ({ W: p.width ?? 6000, H: p.height ?? 4000 })

let assetSeq = 0
const newAssetId = (prefix: string, photoId: number, n: number) => `${prefix}_${photoId}_${n}_${++assetSeq}`

async function savePng(photoId: number, id: string, canvas: OffscreenCanvas): Promise<void> {
  putAsset(photoId, id, await canvas.convertToBlob({ type: 'image/png' }))
}

// ---------------------------------------------------------------- patch generators

/** Source face pasted at the base face position (scaled to fit). */
async function bestTakePatch(base: Photo, source: Photo, baseBox: Box, srcBox: Box): Promise<{ asset: string; rect: Box }> {
  const m = 0.35
  const rb: Box = [baseBox[0] - baseBox[2] * m, baseBox[1] - baseBox[3] * m, baseBox[2] * (1 + 2 * m), baseBox[3] * (1 + 2 * m)]
  const rs: Box = [srcBox[0] - srcBox[2] * m, srcBox[1] - srcBox[3] * m, srcBox[2] * (1 + 2 * m), srcBox[3] * (1 + 2 * m)]
  // clamp the base rect to the image and take the same fraction of the source rect
  const cx0 = clamp01(rb[0])
  const cy0 = clamp01(rb[1])
  const cx1 = clamp01(rb[0] + rb[2])
  const cy1 = clamp01(rb[1] + rb[3])
  const cb: Box = [cx0, cy0, cx1 - cx0, cy1 - cy0]
  const f = [(cb[0] - rb[0]) / rb[2], (cb[1] - rb[1]) / rb[3], cb[2] / rb[2], cb[3] / rb[3]]
  const cs: Box = [rs[0] + f[0] * rs[2], rs[1] + f[1] * rs[3], f[2] * rs[2], f[3] * rs[3]]

  const img = await photoImage(source, 1400)
  const { W, H } = dims(base)
  const aw = 360
  const ah = Math.max(16, Math.round((aw * (cb[3] * H)) / (cb[2] * W)))
  const cv = new OffscreenCanvas(aw, ah)
  const ctx = cv.getContext('2d')!
  ctx.drawImage(img, cs[0] * img.naturalWidth, cs[1] * img.naturalHeight, cs[2] * img.naturalWidth, cs[3] * img.naturalHeight, 0, 0, aw, ah)
  featherEllipse(ctx, aw, ah, 0.55)
  const asset = newAssetId('bt', base.id, srcBox[0] * 1000)
  await savePng(base.id, asset, cv)
  return { asset, rect: [r4(cb[0]), r4(cb[1]), r4(cb[2]), r4(cb[3])] }
}

/** Fill the masked area with blurred nearby "background" (a shifted copy of the frame). */
async function inpaintPatch(photo: Photo, bounds: Box, drawMask: (ctx: OffscreenCanvasRenderingContext2D, rect: Box, aw: number, ah: number) => void): Promise<{ asset: string; rect: Box }> {
  const rect = expand(bounds, 0.25)
  const img = await photoImage(photo, 1400)
  const { W, H } = dims(photo)
  const aw = Math.min(480, Math.max(48, Math.round(rect[2] * 900)))
  const ah = Math.max(16, Math.round((aw * (rect[3] * H)) / (rect[2] * W)))
  const cv = new OffscreenCanvas(aw, ah)
  const ctx = cv.getContext('2d')!
  // neighbouring pixels: sample beside the region (wide shift), blurred = plausible fill
  const shift = rect[0] + rect[2] * 2.4 < 1 ? rect[2] * 2.2 : -rect[2] * 2.2
  const sx = clamp01(rect[0] + shift)
  ctx.filter = 'blur(5px)'
  ctx.drawImage(img, Math.min(sx, 1 - rect[2]) * img.naturalWidth, rect[1] * img.naturalHeight, rect[2] * img.naturalWidth, rect[3] * img.naturalHeight, -8, -8, aw + 16, ah + 16)
  ctx.filter = 'none'
  // the blend mask: what was painted / detected
  const mask = new OffscreenCanvas(aw, ah)
  const mctx = mask.getContext('2d')!
  mctx.filter = 'blur(3px)'
  drawMask(mctx, rect, aw, ah)
  ctx.globalCompositeOperation = 'destination-in'
  ctx.drawImage(mask, 0, 0)
  const asset = newAssetId('ip', photo.id, rect[0] * 1000)
  await savePng(photo.id, asset, cv)
  return { asset, rect: [r4(rect[0]), r4(rect[1]), r4(rect[2]), r4(rect[3])] }
}

const boxMask = (b: Box) => (ctx: OffscreenCanvasRenderingContext2D, rect: Box, aw: number, ah: number) => {
  ctx.fillStyle = '#000'
  const x = ((b[0] - rect[0]) / rect[2]) * aw
  const y = ((b[1] - rect[1]) / rect[3]) * ah
  const w = (b[2] / rect[2]) * aw
  const h = (b[3] / rect[3]) * ah
  ctx.beginPath()
  ctx.ellipse(x + w / 2, y + h / 2, (w / 2) * 1.15, (h / 2) * 1.2, 0, 0, Math.PI * 2)
  ctx.fill()
}

const strokesMask = (strokes: Stroke[], photo: Photo) => (ctx: OffscreenCanvasRenderingContext2D, rect: Box, aw: number, ah: number) => {
  const { W, H } = dims(photo)
  void H
  void W
  ctx.fillStyle = '#000'
  ctx.strokeStyle = '#000'
  ctx.lineCap = 'round'
  ctx.lineJoin = 'round'
  for (const s of strokes) {
    const pts = s.points.map(([x, y]) => [((x - rect[0]) / rect[2]) * aw, ((y - rect[1]) / rect[3]) * ah] as const)
    const rpx = (s.radius / rect[2]) * aw
    if (pts.length === 1) {
      ctx.beginPath()
      ctx.arc(pts[0][0], pts[0][1], rpx, 0, Math.PI * 2)
      ctx.fill()
      continue
    }
    ctx.lineWidth = rpx * 2
    ctx.beginPath()
    pts.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)))
    ctx.stroke()
  }
}

async function denoisePatch(photo: Photo): Promise<{ asset: string; rect: Box }> {
  const img = await photoImage(photo, 900)
  const cv = new OffscreenCanvas(img.naturalWidth, img.naturalHeight)
  const ctx = cv.getContext('2d')!
  ctx.filter = 'blur(2.2px) saturate(1.04)'
  ctx.drawImage(img, 0, 0)
  const asset = newAssetId('dn', photo.id, 0)
  await savePng(photo.id, asset, cv)
  return { asset, rect: [0, 0, 1, 1] }
}

async function faceRestorePatch(photo: Photo, box: Box, n: number): Promise<{ asset: string; rect: Box }> {
  const rect = expand(box, 0.25)
  const img = await photoImage(photo, 1400)
  const { W, H } = dims(photo)
  const aw = 320
  const ah = Math.max(16, Math.round((aw * (rect[3] * H)) / (rect[2] * W)))
  const cv = new OffscreenCanvas(aw, ah)
  const ctx = cv.getContext('2d')!
  ctx.filter = 'contrast(1.22) saturate(1.28) brightness(1.07)'
  ctx.drawImage(img, rect[0] * img.naturalWidth, rect[1] * img.naturalHeight, rect[2] * img.naturalWidth, rect[3] * img.naturalHeight, 0, 0, aw, ah)
  ctx.filter = 'none'
  featherEllipse(ctx, aw, ah, 0.6)
  const asset = newAssetId('fr', photo.id, n)
  await savePng(photo.id, asset, cv)
  return { asset, rect: [r4(rect[0]), r4(rect[1]), r4(rect[2]), r4(rect[3])] }
}

// ---------------------------------------------------------------- tasks

let taskSeq = 0

interface TaskSpec {
  /** recorded in the task history (`GET /api/tasks`) */
  params: Record<string, unknown>
  /** the `*.done` event of a cancelled task (`reason: "cancelled"`) */
  onCancel: () => void
}

/**
 * Emit `task.progress` steps, run `work` at the end, then broadcast. Mirrors the real task lifecycle, including the
 * task history and `POST /api/tasks/:id/cancel` (checked between steps; a cancelled task writes nothing).
 */
function startTask(kind: 'besttake' | 'inpaint' | 'enhance', steps: number, work: () => Promise<void>, spec: TaskSpec): string {
  const task_id = `${kind}-${++taskSeq}`
  const total = steps + 1
  trackTask(task_id, kind, spec.params, total)
  const failTask = takeFail('task')
  let done = 0
  const tick = () => {
    const stepMs = dbg().__m5StepMs ?? 260
    if (isCancelled(task_id)) {
      endTask(task_id, 'cancelled')
      spec.onCancel()
      emit({ type: 'task.progress', task_id, kind, done, total, state: 'cancelled' })
      return
    }
    if (done < steps) {
      done += 1
      taskProgress(task_id, done, total)
      emit({ type: 'task.progress', task_id, kind, done, total, state: 'running' })
      setTimeout(tick, stepMs)
      return
    }
    if (failTask) {
      endTask(task_id, 'failed', 'worker out of memory')
      emit({ type: 'task.progress', task_id, kind, done, total, state: 'failed', error: 'worker out of memory' })
      return
    }
    void work()
      .catch(() => undefined)
      .then(() => {
        endTask(task_id, 'done')
        emit({ type: 'task.progress', task_id, kind, done: total, total, state: 'done' })
      })
  }
  setTimeout(tick, 180)
  return task_id
}

const cancelledResults = (choices: BestTakeChoice[]): BestTakeResult[] =>
  choices.map((c) => ({ base_face_id: c.base_face_id, ok: false, warnings: [], reason: 'cancelled' }))

function saveStack(p: Photo, stack: EditStack) {
  store(p, stack)
  emit({ type: 'edits.updated', items: [{ id: p.id, has_edits: p.has_edits, thumb_version: p.thumb_version }] })
}

// ---------------------------------------------------------------- best take plan

const boxOf = (faceId: number): Box | null => {
  const f = faceRecOf(faceId)
  return f ? [f.bbox[0], f.bbox[1], f.bbox[2], f.bbox[3]] : null
}

/** Reasons a frame's face cannot be pasted (contract: `composable:false` + `reason`). Deterministic. */
function candidateReason(photoId: number, f: { bbox: number[]; yaw: number | null; pi: number }): string | null {
  if (f.bbox[2] < 0.045) return 'face_too_small'
  if (Math.abs(f.yaw ?? 0) > 38) return 'face_not_aligned'
  if ((photoId * 7 + f.pi * 5) % 11 === 0) return 'face_occluded'
  return null
}

function buildPlan(burstId: number): BestTakePlan | null {
  const rec = burstRecOf(burstId)
  if (!rec) return null
  const photos = rec.photo_ids.map((id) => findPhoto(id)).filter((p): p is Photo => !!p)
  const base = [...photos].sort((a, b) => (a.rank_in_burst ?? 99) - (b.rank_in_burst ?? 99))[0]
  if (!base) return null
  const people: BestTakePerson[] = []
  const baseFaces = (aiOf(base.id)?.faces ?? []).filter((f) => f.is_subject)
  for (const bf of baseFaces) {
    const candidates = photos
      .flatMap((p) => {
        const f = (aiOf(p.id)?.faces ?? []).find((x) => x.pi === bf.pi && x.is_subject)
        if (!f) return []
        const reason = p.id === base.id ? null : candidateReason(p.id, f)
        return [{ photo_id: p.id, face_id: f.id, expression_score: Math.round((f.expression_score ?? 0) * 1000) / 1000, composable: reason === null, reason }]
      })
      .sort((a, b) => b.expression_score - a.expression_score || a.photo_id - b.photo_id)
    const best = candidates.find((c) => c.composable)
    people.push({
      track_id: bf.pi + 1,
      person_id: bf.person_id,
      person_name: bf.person_name,
      base_face_id: bf.id,
      candidates,
      best_photo_id: best?.photo_id ?? base.id,
    })
  }
  return { base_photo_id: base.id, people }
}

/** Auto base: the frame where the most people are already at (or near) their best expression. */
function autoBase(plan: BestTakePlan, photoIds: number[]): number {
  let best = plan.base_photo_id
  let bestScore = -1
  for (const pid of photoIds) {
    const s = plan.people.reduce((n, p) => n + (p.candidates.find((c) => c.photo_id === pid)?.expression_score ?? 0), 0)
    if (s > bestScore) {
      bestScore = s
      best = pid
    }
  }
  return best
}

function warningsFor(base: Photo, source: Photo, baseFaceId: number, srcFaceId: number, order: number[]): BestTakeWarning[] {
  const w: BestTakeWarning[] = []
  const bf = faceRecOf(baseFaceId)
  const sf = faceRecOf(srcFaceId)
  if (bf && sf && Math.abs((bf.yaw ?? 0) - (sf.yaw ?? 0)) > 18) w.push('large_pose_change')
  if (Math.abs(order.indexOf(base.id) - order.indexOf(source.id)) >= 4) w.push('camera_moved')
  return w
}

async function composeChoices(base: Photo, choices: BestTakeChoice[], order: number[]): Promise<BestTakeResult[]> {
  let stack = savedStack(base.id)
  const results: BestTakeResult[] = []
  for (const c of choices) {
    const source = faceRecOf(c.source_face_id)
    const src = source ? findPhoto(source.photo_id) : undefined
    const bf = faceRecOf(c.base_face_id)
    const bb = boxOf(c.base_face_id)
    const sb = boxOf(c.source_face_id)
    if (!src || !bf || !bb || !sb || src.id !== c.source_photo_id) {
      results.push({ base_face_id: c.base_face_id, ok: false, warnings: [], reason: 'face_not_found' })
      continue
    }
    const reason = candidateReason(src.id, { bbox: sb, yaw: source?.yaw ?? null, pi: source?.pi ?? 0 })
    if (reason && src.id !== base.id) {
      results.push({ base_face_id: c.base_face_id, ok: false, warnings: [], reason })
      continue
    }
    const { asset, rect } = await bestTakePatch(base, src, bb, sb)
    const patch: PatchOp = {
      type: 'patch',
      kind: 'best_take',
      asset,
      rect,
      feather: 0.08,
      amount: 1,
      enabled: true,
      ...(bf.person_id !== null ? { person_id: bf.person_id } : {}),
      source_photo_id: src.id,
    }
    stack = replaceByKind(stack, 'best_take', [patch], (p) => p.person_id !== undefined && p.person_id === bf.person_id)
    results.push({ base_face_id: c.base_face_id, ok: true, warnings: warningsFor(base, src, c.base_face_id, c.source_face_id, order), reason: null })
  }
  if (results.some((r) => r.ok)) saveStack(base, stack)
  return results
}

// ---------------------------------------------------------------- bystanders / inpaint

const removed = new Map<number, Set<number>>()

function bystandersOf(p: Photo): { face_id: number; bbox: Box }[] {
  const gone = removed.get(p.id)
  return mockPeopleOfPhoto(p)
    .filter((x) => !x.is_subject && !gone?.has(x.face_id))
    .map((x) => ({ face_id: x.face_id, bbox: [r4(x.face_box[0]), r4(x.face_box[1]), r4(x.face_box[2]), r4(x.face_box[3])] as Box }))
}

// ---------------------------------------------------------------- handlers

export const m5Handlers = [
  http.get('/api/bursts/:id/besttake', async ({ params }) => {
    await lat()
    const plan = buildPlan(Number(params.id))
    return plan ? HttpResponse.json(plan) : err(404, 'not_found', 'burst not found')
  }),

  http.post('/api/besttake', async ({ request }) => {
    await lat()
    if (takeFail('503')) return err(503, 'worker_unavailable', 'worker unavailable')
    const body = (await request.json()) as { base_photo_id: number; choices: BestTakeChoice[] }
    const base = findPhoto(body.base_photo_id)
    if (!base) return err(404, 'not_found', 'photo not found')
    if (!body.choices?.length) return err(400, 'bad_request', 'choices required')
    const burst = base.burst_id !== null ? burstRecOf(base.burst_id) : undefined
    const order = burst?.photo_ids ?? []
    const task_id = startTask(
      'besttake',
      3,
      async () => {
        const results = await composeChoices(base, body.choices, order)
        emit({ type: 'besttake.done', photo_id: base.id, results })
      },
      {
        params: { photo_id: base.id, choices: body.choices.length },
        onCancel: () => emit({ type: 'besttake.done', photo_id: base.id, results: cancelledResults(body.choices) }),
      },
    )
    return HttpResponse.json({ task_id }, { status: 202 })
  }),

  http.post('/api/bursts/:id/besttake/auto', async ({ params }) => {
    await lat()
    if (takeFail('503')) return err(503, 'worker_unavailable', 'worker unavailable')
    const plan = buildPlan(Number(params.id))
    if (!plan) return err(404, 'not_found', 'burst not found')
    const rec = burstRecOf(Number(params.id))!
    const baseId = autoBase(plan, rec.photo_ids)
    const base = findPhoto(baseId)!
    const choices: BestTakeChoice[] = []
    for (const p of plan.people) {
      const mine = p.candidates.find((c) => c.photo_id === baseId)
      const best = p.candidates.find((c) => c.composable)
      // the person must exist in the chosen base; paste only when it improves on it
      if (!mine || !best || best.photo_id === baseId || best.expression_score <= mine.expression_score + 0.02) continue
      choices.push({ base_face_id: mine.face_id, source_photo_id: best.photo_id, source_face_id: best.face_id })
    }
    const task_id = startTask(
      'besttake',
      4,
      async () => {
        const results = await composeChoices(base, choices, rec.photo_ids)
        emit({ type: 'besttake.done', photo_id: base.id, results })
      },
      {
        params: { photo_id: base.id, choices: choices.length },
        onCancel: () => emit({ type: 'besttake.done', photo_id: base.id, results: cancelledResults(choices) }),
      },
    )
    return HttpResponse.json({ task_id }, { status: 202 })
  }),

  http.get('/api/photos/:id/bystanders', async ({ params }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    return HttpResponse.json({ faces: bystandersOf(p) })
  }),

  http.post('/api/photos/:id/inpaint', async ({ params, request }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    if (takeFail('503')) return err(503, 'worker_unavailable', 'worker unavailable')
    const miss = missingModelIds(['lama'])
    if (miss.length) return missing409(miss)
    const body = (await request.json()) as InpaintBody
    const strokes = 'strokes' in body ? body.strokes : null
    if (strokes && !strokes.some((s) => s.points.length > 0)) return err(400, 'bad_request', 'empty strokes')
    const faces = 'bystanders' in body ? bystandersOf(p) : 'face_ids' in body ? bystandersOf(p).filter((f) => body.face_ids.includes(f.face_id)) : []
    if (!strokes && faces.length === 0) return err(400, 'bad_request', 'nothing to remove')
    const spec: TaskSpec = {
      params: { photo_id: p.id, faces: faces.length, strokes: strokes?.length ?? 0, model: 'lama' },
      onCancel: () => emit({ type: 'inpaint.done', photo_id: p.id, ok: false, reason: 'cancelled' }),
    }
    const task_id = startTask('inpaint', 3, async () => {
      let stack = savedStack(p.id)
      if (strokes) {
        const b = strokesBoundsNorm(strokes, p)
        const { asset, rect } = await inpaintPatch(p, b, strokesMask(strokes, p))
        stack = addPatch(stack, { type: 'patch', kind: 'inpaint', asset, rect, feather: 0.08, amount: 1, enabled: true })
      } else {
        for (const f of faces) {
          const { asset, rect } = await inpaintPatch(p, f.bbox, boxMask(f.bbox))
          stack = addPatch(stack, { type: 'patch', kind: 'inpaint', asset, rect, feather: 0.08, amount: 1, enabled: true })
        }
        const set = removed.get(p.id) ?? new Set<number>()
        faces.forEach((f) => set.add(f.face_id))
        removed.set(p.id, set)
      }
      saveStack(p, stack)
      emit({ type: 'inpaint.done', photo_id: p.id, ok: true, reason: null })
    }, spec)
    return HttpResponse.json({ task_id }, { status: 202 })
  }),

  http.post('/api/photos/:id/enhance', async ({ params, request }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    if (takeFail('503')) return err(503, 'worker_unavailable', 'worker unavailable')
    const body = (await request.json()) as EnhanceBody
    if (body.op !== 'denoise' && body.op !== 'face_restore') return err(400, 'bad_request', 'unknown op')
    const miss = missingModelIds([body.op === 'denoise' ? 'scunet' : 'gfpgan-v1.4'])
    if (miss.length) return missing409(miss)
    const strength = clamp01(Number(body.strength ?? 0.6))
    const spec: TaskSpec = {
      params: { photo_id: p.id, op: body.op, strength },
      onCancel: () => emit({ type: 'enhance.done', photo_id: p.id, op: body.op, ok: false, reason: 'cancelled' }),
    }
    const task_id = startTask('enhance', 3, async () => {
      let stack = savedStack(p.id)
      if (body.op === 'denoise') {
        const { asset, rect } = await denoisePatch(p)
        stack = replaceByKind(stack, 'denoise', [{ type: 'patch', kind: 'denoise', asset, rect, feather: 0, amount: strength, enabled: true }])
      } else {
        const faces = mockPeopleOfPhoto(p).filter((x) => x.is_subject)
        if (faces.length === 0) {
          emit({ type: 'enhance.done', photo_id: p.id, op: body.op, ok: false, reason: 'no_faces' })
          return
        }
        const next: PatchOp[] = []
        let n = 0
        for (const f of faces) {
          const { asset, rect } = await faceRestorePatch(p, f.face_box, n++)
          next.push({ type: 'patch', kind: 'face_restore', asset, rect, feather: 0.12, amount: strength, enabled: true })
        }
        stack = replaceByKind(stack, 'face_restore', next)
      }
      saveStack(p, stack)
      emit({ type: 'enhance.done', photo_id: p.id, op: body.op, ok: true, reason: null })
    }, spec)
    return HttpResponse.json({ task_id }, { status: 202 })
  }),

  http.get('/api/assets/:photoId/:asset', ({ params }) => {
    const blob = assetBlob(Number(params.photoId), decodeURIComponent(String(params.asset)))
    return blob ? new HttpResponse(blob, { headers: { 'Content-Type': 'image/png', 'Cache-Control': 'no-cache' } }) : err(404, 'not_found', 'asset not found')
  }),
]

function strokesBoundsNorm(strokes: Stroke[], p: Photo): Box {
  const { W, H } = dims(p)
  let x0 = 1
  let y0 = 1
  let x1 = 0
  let y1 = 0
  for (const s of strokes) {
    const ry = (s.radius * W) / H
    for (const [x, y] of s.points) {
      x0 = Math.min(x0, x - s.radius)
      x1 = Math.max(x1, x + s.radius)
      y0 = Math.min(y0, y - ry)
      y1 = Math.max(y1, y + ry)
    }
  }
  x0 = clamp01(x0)
  y0 = clamp01(y0)
  return [x0, y0, Math.max(0.02, clamp01(x1) - x0), Math.max(0.02, clamp01(y1) - y0)]
}
