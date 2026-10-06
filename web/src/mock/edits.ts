import { delay, http, HttpResponse } from 'msw'
import type { Adjust, AutoMode, EditSection, EditStack, MaskTarget, Photo, Preset, SyncBody } from '@/api/types'
import { MASK_TARGETS } from '@/api/types'
import { applySections, emptyStack, isEmptyStack, normalizeStack, stackKey } from '@/lib/edit'
import { missingModelIds } from './ai'
import { emit } from './bus'
import { findPhoto, hashString } from './db'
import { MASK_MODELS, maskPng } from './masks'
import { renderMock } from './render'

const err = (status: number, code: string, message: string) =>
  HttpResponse.json({ error: { code, message } }, { status })

// ---------------------------------------------------------------- store

interface Saved {
  stack: EditStack
  updated_at: number
}
const saved = new Map<number, Saved>()
const baseVersion = new Map<number, string>()

const importedLuts: { id: string; name: string; builtin: boolean }[] = []

export const savedStack = (id: number): EditStack => saved.get(id)?.stack ?? emptyStack()

/** Thumb version that changes with the edit stack (contract B: "includes the edit-stack hash"). */
function versionFor(p: Photo, stack: EditStack): string {
  if (!baseVersion.has(p.id)) baseVersion.set(p.id, p.thumb_version)
  return isEmptyStack(stack) ? baseVersion.get(p.id)! : `e${(hashString(stackKey(stack)) & 0xffffff).toString(16)}`
}

export function store(p: Photo, stackIn: EditStack): { stack: EditStack; updated_at: number; thumb_version: string } {
  const stack = normalizeStack(stackIn)
  const updated_at = Date.now()
  if (isEmptyStack(stack)) saved.delete(p.id)
  else saved.set(p.id, { stack, updated_at })
  p.has_edits = !isEmptyStack(stack)
  p.thumb_version = versionFor(p, stack)
  return { stack, updated_at, thumb_version: p.thumb_version }
}

// ---------------------------------------------------------------- presets

const g = (a: Adjust): EditStack => ({ version: 1, ops: [{ type: 'global', ...a }] })

const BUILTIN: Preset[] = [
  {
    id: 'film_warm',
    name: 'preset.film_warm',
    builtin: true,
    stack: {
      version: 1,
      ops: [
        { type: 'global', exposure: 0.1, contrast: 8, temp: 450, tint: 4, saturation: -6, blacks: 6, curve: { rgb: [[0, 0.04], [0.5, 0.52], [1, 0.97]] } },
        { type: 'lut', file: 'film_warm', amount: 0.6 },
      ],
    },
  },
  {
    id: 'film_cool',
    name: 'preset.film_cool',
    builtin: true,
    stack: { version: 1, ops: [{ type: 'global', contrast: 10, temp: -600, saturation: -8, grading: { shadows: [215, 0.18], highlights: [45, 0.06], midtones: [0, 0], balance: 0 } }, { type: 'lut', file: 'film_cool', amount: 0.5 }] },
  },
  { id: 'vivid', name: 'preset.vivid', builtin: true, stack: g({ contrast: 14, vibrance: 28, saturation: 12, clarity: 12, dehaze: 8 }) },
  { id: 'matte', name: 'preset.matte', builtin: true, stack: { version: 1, ops: [{ type: 'global', contrast: -12, blacks: 25, saturation: -8 }, { type: 'lut', file: 'matte', amount: 0.8 }] } },
  { id: 'bw_contrast', name: 'preset.bw_contrast', builtin: true, stack: { version: 1, ops: [{ type: 'global', contrast: 32, clarity: 18, whites: 10, blacks: -14 }, { type: 'lut', file: 'bw', amount: 1 }] } },
  { id: 'teal_orange', name: 'preset.teal_orange', builtin: true, stack: { version: 1, ops: [{ type: 'global', contrast: 10, vibrance: 14 }, { type: 'lut', file: 'teal_orange', amount: 0.75 }] } },
  {
    id: 'golden_hour',
    name: 'preset.golden_hour',
    builtin: true,
    stack: g({ exposure: 0.15, temp: 900, tint: 6, highlights: -20, shadows: 18, vibrance: 14, grading: { shadows: [20, 0.12], midtones: [35, 0.08], highlights: [48, 0.14], balance: 20 } }),
  },
  { id: 'soft_portrait', name: 'preset.soft_portrait', builtin: true, stack: g({ exposure: 0.2, contrast: -8, highlights: -15, shadows: 15, clarity: -12, temp: 200, vibrance: 8 }) },
]

const userPresets: Preset[] = []
let presetSeq = 0

// ---------------------------------------------------------------- auto

function autoAdjust(p: Photo, mode: AutoMode): Adjust {
  const h = hashString(`${p.id}:${mode}`)
  const r = (k: number) => ((h >> (k * 3)) & 7) / 7 // 0..1, deterministic per photo
  const base: Adjust = {
    exposure: Math.round((0.15 + r(0) * 0.5) * 100) / 100,
    contrast: 8 + Math.round(r(1) * 10),
    highlights: -(18 + Math.round(r(2) * 18)),
    shadows: 14 + Math.round(r(3) * 16),
    whites: 4 + Math.round(r(4) * 8),
    blacks: -(3 + Math.round(r(5) * 6)),
    vibrance: 10 + Math.round(r(6) * 8),
    source: 'ai_auto@1',
  }
  if (mode === 'portrait') return { ...base, exposure: (base.exposure ?? 0) + 0.1, contrast: 4, clarity: -8, temp: 250, tint: 3, vibrance: 6, saturation: -2, highlights: -25, shadows: 22 }
  if (mode === 'landscape') return { ...base, clarity: 14, dehaze: 16, saturation: 8, vibrance: 18, contrast: 14, highlights: -32, shadows: 26 }
  return { ...base, clarity: 6, temp: Math.round((r(7) - 0.5) * 400) }
}

// ---------------------------------------------------------------- rendering glue (also used by thumb/preview)

const jpeg = (blob: Blob, ms: number, extra?: Record<string, string>) =>
  new HttpResponse(blob, {
    headers: { 'Content-Type': 'image/jpeg', 'X-Render-Ms': String(Math.max(1, Math.round(ms))), 'X-Render-Backend': 'cpu', ...extra },
  })

/** Rendered (edited) thumb/preview of a photo with saved edits. */
export async function renderEdited(p: Photo, longEdge: number): Promise<Response> {
  const t0 = performance.now()
  const blob = await renderMock(p, savedStack(p.id), { longEdge: Math.min(longEdge, 1280) })
  return jpeg(blob, performance.now() - t0, { 'Cache-Control': 'public, max-age=31536000, immutable' })
}

// ---------------------------------------------------------------- handlers

const lat = () => delay(10 + Math.random() * 25)

export const editHandlers = [
  http.get('/api/edits/:id', async ({ params }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const s = saved.get(p.id)
    return HttpResponse.json({ photo_id: p.id, stack: s?.stack ?? emptyStack(), updated_at: s?.updated_at ?? null })
  }),
  http.put('/api/edits/:id', async ({ params, request }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const body = (await request.json()) as { stack?: EditStack }
    if (!body?.stack || !Array.isArray(body.stack.ops)) return err(400, 'bad_request', 'stack required')
    const res = store(p, body.stack)
    emit({ type: 'edits.updated', items: [{ id: p.id, has_edits: p.has_edits, thumb_version: p.thumb_version }] })
    return HttpResponse.json({ photo_id: p.id, ...res })
  }),
  http.delete('/api/edits/:id', async ({ params }) => {
    await lat()
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    store(p, emptyStack())
    emit({ type: 'edits.updated', items: [{ id: p.id, has_edits: false, thumb_version: p.thumb_version }] })
    return new HttpResponse(null, { status: 204 })
  }),

  http.post('/api/render/preview', async ({ request }) => {
    const body = (await request.json()) as { photo_id: number; stack?: EditStack; long_edge?: number; original?: boolean }
    const p = findPhoto(body.photo_id)
    if (!p) return err(404, 'not_found', 'photo not found')
    // emulate GPU-ish latency growing with size
    const long = body.long_edge ?? 1600
    await delay(8 + (long / 2048) * 50 + Math.random() * 12)
    const t0 = performance.now()
    const blob = await renderMock(p, body.stack ?? savedStack(p.id), { longEdge: long, original: body.original })
    return jpeg(blob, performance.now() - t0)
  }),

  http.post('/api/edits/:id/auto', async ({ params, request }) => {
    await delay(140 + Math.random() * 120)
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const { mode } = (await request.json()) as { mode: AutoMode }
    if (!['auto', 'portrait', 'landscape'].includes(mode)) return err(400, 'bad_request', 'bad mode')
    return HttpResponse.json({ adjust: autoAdjust(p, mode) })
  }),

  http.post('/api/edits/sync', async ({ request }) => {
    await lat()
    const body = (await request.json()) as SyncBody
    const from = findPhoto(body.from_id)
    if (!from) return err(404, 'not_found', 'photo not found')
    const src = savedStack(from.id)
    const include = (body.include ?? []) as EditSection[]
    const items: { id: number; has_edits: boolean; thumb_version: string }[] = []
    for (const id of body.to_ids ?? []) {
      const p = findPhoto(id)
      if (!p || p.id === from.id) continue
      let next = applySections(savedStack(id), src, include)
      if (body.adaptive !== false) {
        // adaptive: small deterministic per-photo exposure / white-balance compensation
        const h = hashString(`sync:${id}`)
        const dEv = (((h & 15) - 7) / 7) * 0.2
        const dT = ((((h >> 4) & 15) - 7) / 7) * 150
        next = {
          ...next,
          ops: next.ops.map((o) =>
            o.type === 'global'
              ? { ...o, exposure: Math.round((((o.exposure as number | undefined) ?? 0) + dEv) * 100) / 100, temp: ((o.temp as number | undefined) ?? 0) + Math.round(dT) }
              : o,
          ),
        }
      }
      store(p, next)
      items.push({ id, has_edits: p.has_edits, thumb_version: p.thumb_version })
    }
    if (items.length) emit({ type: 'edits.updated', items })
    return HttpResponse.json({ updated: items.length })
  }),

  http.get('/api/presets', async () => {
    await lat()
    return HttpResponse.json({ presets: [...BUILTIN, ...userPresets] })
  }),
  http.post('/api/presets', async ({ request }) => {
    await lat()
    const { name, stack } = (await request.json()) as { name: string; stack: EditStack }
    if (!name?.trim()) return err(400, 'bad_request', 'name required')
    // only global/local(non-person)/lut/output_sharpen are kept; crop is dropped
    const kept = applySections(emptyStack(), stack, ['global', 'local', 'lut', 'output_sharpen'])
    const preset: Preset = { id: `user-${++presetSeq}`, name: name.trim(), builtin: false, stack: kept }
    userPresets.push(preset)
    return HttpResponse.json({ preset }, { status: 201 })
  }),
  http.delete('/api/presets/:id', async ({ params }) => {
    await lat()
    const id = String(params.id)
    if (BUILTIN.some((b) => b.id === id)) return err(400, 'bad_request', 'built-in presets cannot be deleted')
    const i = userPresets.findIndex((u) => u.id === id)
    if (i < 0) return err(404, 'not_found', 'preset not found')
    userPresets.splice(i, 1)
    return new HttpResponse(null, { status: 204 })
  }),

  http.get('/api/luts', async () => {
    await lat()
    const builtin = ['film_warm', 'film_cool', 'teal_orange', 'matte', 'bw'].map((id) => ({ id, name: `lut.${id}`, builtin: true }))
    return HttpResponse.json({ luts: [...builtin, ...importedLuts] })
  }),

  http.post('/api/luts/import', async ({ request }) => {
    await lat()
    const { path } = (await request.json()) as { path: string }
    if (!path || !/\.cube$/i.test(path)) return err(400, 'bad_request', 'expected a .cube file path')
    const name = path.split(/[\\/]/).pop()!.replace(/\.cube$/i, '')
    const id = `lut_${(hashString(path) & 0xffff).toString(16)}`
    if (!importedLuts.some((l) => l.id === id)) importedLuts.push({ id, name, builtin: false })
    return HttpResponse.json({ id, name })
  }),

  http.get('/api/masks/:id', async ({ params, request }) => {
    await delay(60 + Math.random() * 60)
    const p = findPhoto(Number(params.id))
    if (!p) return err(404, 'not_found', 'photo not found')
    const target = new URL(request.url).searchParams.get('target') as MaskTarget | null
    if (!target || !MASK_TARGETS.includes(target)) return err(400, 'bad_request', 'unknown target')
    const missing = missingModelIds([MASK_MODELS[target]])
    if (missing.length) {
      return HttpResponse.json(
        { error: { code: 'models_missing', message: 'required models are not installed' }, models: missing },
        { status: 409 },
      )
    }
    return new HttpResponse(await maskPng(p, target), { headers: { 'Content-Type': 'image/png' } })
  }),
]
