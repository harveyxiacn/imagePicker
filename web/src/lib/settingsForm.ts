/** Settings form <-> `/api/settings` mapping (docs/api-contract-m6.md D): paths, patches, optimistic merge, validation. */
import type { Settings, SettingsPatch } from '@/api/types'

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)

/** Every form control addresses one setting by its dotted path, e.g. `analysis.default_profile`. */
export type SettingPath =
  | 'language'
  | 'theme'
  | 'analysis.default_profile'
  | 'analysis.auto_analyze_on_import'
  | 'analysis.group_strictness'
  | 'faces.enabled'
  | 'faces.consented'
  | 'privacy.allow_network'
  | 'models.source'
  | 'cache.max_gb'
  | 'render.backend'
  | 'xmp_mode'
  | 'lan.enabled'
  | 'lan.port'
  | 'lan.guest_enabled'
  | 'assistant.engine'
  | 'roots'

export function getAt(s: Settings, path: SettingPath): unknown {
  let cur: unknown = s
  for (const k of path.split('.')) cur = isObj(cur) ? cur[k] : undefined
  return cur
}

/** `{ "a.b": v }` -> `{ a: { b: v } }`: the minimal PATCH body for one control. */
export function patchAt(path: SettingPath, value: unknown): SettingsPatch {
  const keys = path.split('.')
  const out: Obj = {}
  let cur = out
  keys.forEach((k, i) => {
    if (i === keys.length - 1) cur[k] = value
    else cur = cur[k] = {} as Obj
  })
  return out as SettingsPatch
}

/** Apply a patch to settings (optimistic UI; mirrors the server's merge: nested objects merge, arrays replace). */
export function mergeSettings(s: Settings, patch: SettingsPatch): Settings {
  const merge = (a: Obj, b: Obj): Obj => {
    const out: Obj = { ...a }
    for (const [k, v] of Object.entries(b)) out[k] = isObj(v) && isObj(a[k]) ? merge(a[k] as Obj, v) : v
    return out
  }
  return merge(s as unknown as Obj, patch as Obj) as unknown as Settings
}

/** Smallest patch turning `prev` into `next` (empty object when equal). */
export function diffSettings(prev: Settings, next: Settings): SettingsPatch {
  const diff = (a: Obj, b: Obj): Obj => {
    const out: Obj = {}
    for (const k of Object.keys(b)) {
      if (isObj(b[k]) && isObj(a[k])) {
        const d = diff(a[k] as Obj, b[k] as Obj)
        if (Object.keys(d).length) out[k] = d
      } else if (JSON.stringify(a[k]) !== JSON.stringify(b[k])) out[k] = b[k]
    }
    return out
  }
  return diff(prev as unknown as Obj, next as unknown as Obj) as SettingsPatch
}

export const PORT_MIN = 1024
export const PORT_MAX = 65535
export const CACHE_GB_MIN = 1
export const CACHE_GB_MAX = 2048

/** Parses a port input; null when invalid (privileged / out of range / not an integer). */
export function parsePort(raw: string): number | null {
  if (!/^\d{1,5}$/.test(raw.trim())) return null
  const n = Number(raw)
  return n >= PORT_MIN && n <= PORT_MAX ? n : null
}

/** Parses the cache limit (GB); null when invalid. */
export function parseCacheGb(raw: string): number | null {
  const n = Number(raw)
  return Number.isFinite(n) && n >= CACHE_GB_MIN && n <= CACHE_GB_MAX ? Math.round(n * 10) / 10 : null
}

/** Typed confirmation for destructive actions (clear all face data): exact match, trimmed, case-insensitive. */
export const isTypedConfirm = (input: string, word: string): boolean => input.trim().toLowerCase() === word.trim().toLowerCase() && word.trim().length > 0

/** Share of the cache budget used by `bytes`, clamped to 0..1. */
export const cacheFraction = (bytes: number, maxGb: number): number => (maxGb > 0 ? Math.min(1, Math.max(0, bytes / (maxGb * 1e9))) : 0)

/** Default settings used until the server answers (also the base of the mock store). */
export const DEFAULT_SETTINGS: Settings = {
  language: 'zh-CN',
  theme: 'dark',
  analysis: { default_profile: 'standard', auto_analyze_on_import: false, group_strictness: 'normal' },
  faces: { enabled: true, consented: false },
  privacy: { allow_network: true },
  models: { dir: '', source: 'auto' },
  cache: { max_gb: 20 },
  render: { backend: 'auto' },
  xmp_mode: 'off',
  lan: { enabled: false, port: 7878, guest_enabled: false },
  roots: [],
  assistant: { engine: 'auto' },
}
