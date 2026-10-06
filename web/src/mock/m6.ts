/**
 * Mock backend for M6 (docs/api-contract-m6.md): rules-like assistant planner (~18 commands, zh + en), plan execution with
 * progress and `assistant.done` (+ undo payload), settings store, cache stats, onboarding, LAN status with a fake SVG QR,
 * auth (login / me / logout with owner + guest roles, rate limit, 401 / 403 gate) and XMP sync.
 *
 * URL switches read once at start-up (so tests can pick a scenario):
 *   ?mock_auth=locked     LAN-style auth required (owner password "owner123", guest password "guest123")
 *   ?mock_onboarding=1    first-run onboarding is shown
 *   ?mock_vlm=0           VLM unavailable (no "修图建议" button)
 * Debug switches on `globalThis`: `__m6StepMs` (duration of one plan step, default 600).
 */
import { delay, http, HttpResponse } from 'msw'
import type {
  Adjust,
  AssistantContext,
  AssistantPlan,
  AssistantResult,
  AssistantStatus,
  AssistantUndo,
  EditStack,
  Photo,
  PlanStep,
  Role,
  Settings,
  SettingsPatch,
} from '@/api/types'
import { applySections, mergeAuto, normalizeStack } from '@/lib/edit'
import { DEFAULT_SETTINGS, mergeSettings } from '@/lib/settingsForm'
import { clearFaceData, MODELS, peopleListOf, workerInfo } from './ai'
import { emit } from './bus'
import { burstKeyOf, computeCounts, findPhoto, getSession } from './db'
import { autoAdjust, BUILTIN, savedStack, store } from './edits'
import { filterPhotos } from './query'

const err = (status: number, code: string, message: string) => HttpResponse.json({ error: { code, message } }, { status })
const lat = () => delay(15 + Math.random() * 30)
const json = <T,>(body: T, status = 200) => HttpResponse.json(body as never, { status })
const no = () => new HttpResponse(null, { status: 204 })

// ============================================================================
// scenario switches + auth
// ============================================================================

const params = (): URLSearchParams => new URLSearchParams(typeof location !== 'undefined' ? location.search : '')

interface AuthState {
  required: boolean
  ownerPw: string | null
  guestPw: string | null
  /** the "session cookie" of this browser tab */
  role: Role | null
  failures: number[]
}
const auth: AuthState = { required: false, ownerPw: null, guestPw: null, role: 'owner', failures: [] }
let firstRun = false
let vlmAvailable = true

const ROLE_KEY = 'mock.role'
function loadRole(): Role | null {
  try {
    const r = sessionStorage.getItem(ROLE_KEY)
    return r === 'owner' || r === 'guest' ? r : null
  } catch {
    return null
  }
}
function saveRole(r: Role | null) {
  try {
    if (r) sessionStorage.setItem(ROLE_KEY, r)
    else sessionStorage.removeItem(ROLE_KEY)
  } catch {
    /* ignore */
  }
}

/** Applies the URL switches. Called once from `enableMock()`. */
export function seedM6Mock(): void {
  const p = params()
  if (p.get('mock_auth') === 'locked') {
    auth.required = true
    auth.ownerPw = 'owner123'
    auth.guestPw = 'guest123'
    auth.role = loadRole()
    settings.lan.enabled = true
    settings.lan.guest_enabled = true
  }
  firstRun = p.get('mock_onboarding') === '1'
  vlmAvailable = p.get('mock_vlm') !== '0'
  ;(globalThis as { __mockM6?: unknown }).__mockM6 = {
    setRole: (r: Role | null) => {
      auth.role = r
      saveRole(r)
    },
    auth,
  }
}

// real backend: guests get 403 for people, faces, taste, settings, assistant, system, fs, xmp, cache, onboarding, masks, models, besttake, analysis
const GUEST_BLOCKED_GET = [/^\/api\/people/, /^\/api\/faces/, /^\/api\/taste/, /^\/api\/settings/, /^\/api\/assistant/, /^\/api\/system/, /^\/api\/fs/, /^\/api\/xmp/, /^\/api\/cache/, /^\/api\/onboarding/, /^\/api\/masks/, /^\/api\/models/, /^\/api\/bursts\/\d+\/besttake/, /^\/api\/analysis/, /^\/api\/photos\/\d+\/(analysis|people|bystanders)/]
const GUEST_ALLOWED_WRITE = [/^\/api\/viewport/]

/** First handler: 401 for anonymous callers, 403 for guests writing / reading private data. Falls through otherwise. */
export const m6Gate = http.all('/api/*', ({ request }) => {
  if (!auth.required) return undefined
  const path = new URL(request.url).pathname
  if (path === '/api/health' || path.startsWith('/api/auth/')) return undefined
  if (!auth.role) return err(401, 'unauthorized', 'authentication required')
  if (auth.role === 'guest') {
    if (request.method === 'GET') {
      if (GUEST_BLOCKED_GET.some((r) => r.test(path))) return err(403, 'forbidden', 'guests have read-only access')
    } else if (!GUEST_ALLOWED_WRITE.some((r) => r.test(path))) return err(403, 'forbidden', 'guests have read-only access')
  }
  return undefined
})

// ============================================================================
// settings / cache / LAN / onboarding
// ============================================================================

let settings: Settings = mergeSettings(DEFAULT_SETTINGS, {
  models: { dir: 'C:\\Users\\demo\\AppData\\Local\\imagePicker\\models', source: 'auto' },
  roots: ['C:\\Users\\demo\\Pictures', 'D:\\Archive'],
})

const cacheBytes = { thumbs: 1_840_000_000, previews: 3_210_000_000, masks: 248_000_000, edits: 12_400_000, gen: 655_000_000 }

/** Deterministic QR-looking SVG (finder squares + hash noise). Mock only: it does NOT encode the URL. */
export function fakeQr(text: string): string {
  const n = 29
  let h = 2166136261
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619)
  const rnd = () => {
    h = Math.imul(h ^ (h >>> 15), 2246822507) >>> 0
    h = Math.imul(h ^ (h >>> 13), 3266489909) >>> 0
    return ((h ^ (h >>> 16)) >>> 0) / 4294967296
  }
  const finder = (x: number, y: number) => {
    const inF = (ox: number, oy: number) => x >= ox && x < ox + 7 && y >= oy && y < oy + 7
    for (const [ox, oy] of [[0, 0], [n - 7, 0], [0, n - 7]] as const) {
      if (inF(ox, oy)) {
        const dx = x - ox
        const dy = y - oy
        const ring = Math.max(Math.abs(dx - 3), Math.abs(dy - 3))
        return ring !== 2 ? 1 : 0
      }
      if (x >= ox - 1 && x <= ox + 7 && y >= oy - 1 && y <= oy + 7) return 0
    }
    return null
  }
  let d = ''
  for (let y = 0; y < n; y++)
    for (let x = 0; x < n; x++) {
      const f = finder(x, y)
      const on = f === null ? rnd() < 0.48 : f === 1
      if (on) d += `M${x + 2} ${y + 2}h1v1h-1z`
    }
  const size = n + 4
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}" shape-rendering="crispEdges"><rect width="${size}" height="${size}" fill="#fff"/><path d="${d}" fill="#000"/></svg>`
}

/** LAN listener state of the mock server (turns on 1.5 s after enabling to show "restart required" first). */
let lanActive = false
const lanUrls = () => [`http://192.168.1.23:${settings.lan.port}`, `http://imagepicker.local:${settings.lan.port}`]

// ============================================================================
// assistant: planner
// ============================================================================

type Loc = 'zh-CN' | 'en'
interface StoredPlan {
  plan: AssistantPlan
  sessionId: number
  ctx: AssistantContext
}
const plans = new Map<string, StoredPlan>()
let planSeq = 0

const has = (text: string, re: RegExp) => re.test(text)
const num = (text: string, re: RegExp): number | null => {
  const m = text.match(re)
  return m ? Number(m[1]) : null
}
const CN_NUM: Record<string, number> = { 一: 1, 二: 2, 两: 2, 三: 3, 四: 4, 五: 5 }
const cn = (s: string) => (CN_NUM[s] !== undefined ? CN_NUM[s] : Number(s))

const PRESET_WORDS: [RegExp, string, string, string][] = [
  [/胶片|film|暖调|warm/, 'film_warm', '暖调胶片', 'Warm Film'],
  [/冷调|cool/, 'film_cool', '冷调胶片', 'Cool Film'],
  [/鲜艳|vivid/, 'vivid', '鲜艳', 'Vivid'],
  [/哑光|matte/, 'matte', '哑光', 'Matte'],
  [/黑白|b&w|bw|mono/, 'bw_contrast', '黑白高反差', 'B&W Contrast'],
  [/青橙|teal/, 'teal_orange', '青橙', 'Teal & Orange'],
  [/黄金|golden/, 'golden_hour', '黄金时刻', 'Golden Hour'],
  [/柔和|soft/, 'soft_portrait', '柔和人像', 'Soft Portrait'],
]

const ISSUE_WORDS: [RegExp, string, string, string][] = [
  [/闭眼|closed eyes?|eyes closed/, 'closed_eyes', '闭眼', 'closed eyes'],
  [/模糊|虚焦|blurry|blur/, 'blurry', '模糊', 'blurry'],
  [/过曝|overexpos/, 'overexposed', '过曝', 'overexposed'],
  [/欠曝|曝光不足|underexpos/, 'underexposed', '欠曝', 'underexposed'],
  [/噪点|noisy|noise/, 'noisy', '噪点', 'noisy'],
  [/歪|倾斜|tilted/, 'tilted', '倾斜', 'tilted'],
]
const SCENE_WORDS: [RegExp, string, string, string][] = [
  [/人像|portrait/, 'portrait', '人像', 'portrait'],
  [/合影|group photo|group shot/, 'group', '合影', 'group'],
  [/风景|风光|landscape/, 'landscape', '风景', 'landscape'],
  [/美食|food/, 'food', '美食', 'food'],
  [/建筑|architecture/, 'architecture', '建筑', 'architecture'],
  [/夜景|night/, 'night', '夜景', 'night'],
  [/宠物|pet/, 'pet', '宠物', 'pet'],
]

function describeFilter(args: Record<string, unknown>, L: (zh: string, en: string) => string): string {
  const parts: string[] = []
  if (args.flag === 'picked') parts.push(L('精选', 'picked'))
  if (args.flag === 'rejected') parts.push(L('已淘汰', 'rejected'))
  if (args.flag === 'not_rejected') parts.push(L('未淘汰', 'not rejected'))
  if (typeof args.rating_gte === 'number') parts.push(L(`${args.rating_gte} 星以上`, `${args.rating_gte}+ stars`))
  if (typeof args.ai_rating_gte === 'number') parts.push(L(`AI ${args.ai_rating_gte} 星以上`, `AI ${args.ai_rating_gte}+ stars`))
  if (Array.isArray(args.issues_any)) {
    const names = (args.issues_any as string[]).map((i) => ISSUE_WORDS.find((w) => w[1] === i)).filter(Boolean) as (typeof ISSUE_WORDS)[number][]
    parts.push(names.map((n) => L(n[2], n[3])).join(' / '))
  }
  if (args.issues_none) parts.push(L('无问题', 'no issues'))
  if (typeof args.scene_type === 'string') {
    const w = SCENE_WORDS.find((x) => x[1] === args.scene_type)
    if (w) parts.push(L(w[2], w[3]))
  }
  if (args.burst_best_only) parts.push(L('每组最佳', 'best of each group'))
  if (Array.isArray(args.persons)) {
    const names = peopleListOf(null)
    parts.push((args.persons as number[]).map((id) => names.find((p) => p.id === id)?.name ?? `#${id}`).join(' + '))
  }
  return parts.join(' · ') || L('全部照片', 'all photos')
}

const toQuery = (args: Record<string, unknown>): string => {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(args)) {
    if (v === true) p.set(k, '1')
    else if (Array.isArray(v)) p.set(k, v.join(','))
    else if (v !== null && v !== undefined && v !== false) p.set(k, String(v))
  }
  return p.toString()
}

function sessionPhotos(sessionId: number): Photo[] {
  return getSession(sessionId)?.photos ?? []
}

type Sel = { ids: number[] } | { query: string } | 'current_filter'

function resolveSel(sessionId: number, ctx: AssistantContext, sel: Sel | undefined): Photo[] {
  const all = sessionPhotos(sessionId)
  if (sel && typeof sel === 'object' && 'ids' in sel) return sel.ids.map((id) => findPhoto(id)).filter((p): p is Photo => !!p)
  const q = sel && typeof sel === 'object' ? sel.query : ctx.filter
  return filterPhotos(all, new URLSearchParams(q))
}

/** Per-burst ranking (ground-truth burst keys, so it also works before analysis). */
function groupKeep(photos: Photo[], key: (p: Photo) => number | string, n: number): { keep: number[]; rest: number[] } {
  const groups = new Map<number | string, Photo[]>()
  for (const p of photos) {
    const k = key(p)
    const g = groups.get(k)
    if (g) g.push(p)
    else groups.set(k, [p])
  }
  const keep: number[] = []
  const rest: number[] = []
  for (const g of groups.values()) {
    if (g.length < 2) continue
    const sorted = [...g].sort((a, b) => (b.ai_score ?? -1) - (a.ai_score ?? -1) || a.id - b.id)
    sorted.forEach((p, i) => (i < n ? keep : rest).push(p.id))
  }
  return { keep, rest }
}
const burstKey = (p: Photo): number => p.burst_id ?? burstKeyOf(p.id)
const sceneKey = (p: Photo): string => p.scene_type ?? `t${Math.floor((p.taken_at ?? 0) / 1_800_000)}`

function plan(sessionId: number, message: string, ctx: AssistantContext): AssistantPlan {
  const loc: Loc = ctx.locale
  const L = (zh: string, en: string) => (loc === 'en' ? en : zh)
  const text = message.toLowerCase()
  const steps: PlanStep[] = []
  let curQuery = ctx.filter
  const useSel = ctx.selection.length > 0 && has(text, /选中|所选|selected|selection/)
  const sel = (): Sel => (useSel ? { ids: ctx.selection } : steps.some((s) => s.tool === 'filter') ? { query: curQuery } : 'current_filter')
  const count = (s: Sel) => resolveSel(sessionId, ctx, s).length
  const push = (tool: string, args: Record<string, unknown>, summary: string, affects: number, destructive = false) =>
    steps.push({ tool, args, summary, affects, destructive })

  const bystander = has(text, /路人|bystander/)
  const rejectVerb = !bystander && has(text, /淘汰|删除|删掉|reject|delete|discard/) && !has(text, /淘汰其[余它]|reject (the )?rest/)
  const setRating = has(text, /(打|评|设为?|标)\s*[1-5一二三四五两]\s*星|rate .*[1-5] stars?|set (the )?rating/)

  // ---- filters ----
  const filterArgs: Record<string, unknown> = {}
  if (has(text, /只看精选|精选的|show (only )?picked|picked only|only picked/)) filterArgs.flag = 'picked'
  else if (has(text, /未淘汰|not rejected/)) filterArgs.flag = 'not_rejected'
  else if (!rejectVerb && has(text, /已淘汰|淘汰的|rejected photos|show rejected/)) filterArgs.flag = 'rejected'
  const stars = !setRating ? (num(text, /([1-5])\s*(?:星|stars?)\s*(?:以上|及以上|或以上|\+|or (?:more|higher))?/) ?? (has(text, /四星|五星/) ? cn(text.match(/([四五])星/)![1]) : null)) : null
  if (stars && !has(text, /ai/)) filterArgs.rating_gte = stars
  const issues = ISSUE_WORDS.filter((w) => w[0].test(text)).map((w) => w[1])
  if (issues.length && !has(text, /消除|修复|retouch/)) filterArgs.issues_any = issues
  const scene = SCENE_WORDS.find((w) => w[0].test(text))
  if (scene) filterArgs.scene_type = scene[1]
  if (has(text, /只看(?:每组)?最佳|best only|only the best/)) filterArgs.burst_best_only = true
  const ppl = peopleListOf(null).filter((p) => p.name && text.includes(p.name.toLowerCase()))
  if (ppl.length) filterArgs.persons = ppl.map((p) => p.id)
  const wantsFilter = Object.keys(filterArgs).length > 0 && !has(text, /导出|export/)
  if (wantsFilter) {
    curQuery = toQuery(filterArgs)
    push('filter', filterArgs, L(`筛选：${describeFilter(filterArgs, L)}`, `Filter: ${describeFilter(filterArgs, L)}`), filterPhotos(sessionPhotos(sessionId), new URLSearchParams(curQuery)).length)
  }

  // ---- actions ----
  if (rejectVerb && (issues.length || has(text, /这些|它们|them|these/))) {
    const s = sel()
    const n = count(s)
    push('set_flag', { selection: s, flag: -1 }, L(`将 ${n} 张照片标为淘汰`, `Reject ${n} photos`), n, true)
  }
  if (setRating) {
    const r = cn(text.match(/([1-5一二三四五两])\s*星|([1-5]) stars?/)?.[1] ?? text.match(/([1-5]) stars?/)![1])
    const s = sel()
    const n = count(s)
    push('set_rating', { selection: s, rating: r }, L(`将 ${n} 张照片评为 ${r} 星`, `Rate ${n} photos ${r} stars`), n, n > 1)
  }
  if (has(text, /精选|picked?/) && has(text, /(标|设|打|mark|flag|set).{0,6}(精选|pick)/) && !wantsFilter) {
    const s = sel()
    const n = count(s)
    push('set_flag', { selection: s, flag: 1 }, L(`将 ${n} 张照片标为精选`, `Mark ${n} photos as picked`), n)
  }
  const keepN = num(text, /(?:前|top\s*)(\d+)|保留\s*(\d+)/) ?? (cn(text.match(/前([一二三四五两])/)?.[1] ?? '0') || null)
  const rejectRest = has(text, /淘汰其[余它]|淘汰剩|reject (the )?rest|reject others/)
  if (has(text, /每个?场景|each scene|per scene/) && has(text, /保留|keep|前|top/)) {
    const n = keepN ?? 3
    const s = sel()
    const g = groupKeep(resolveSel(sessionId, ctx, s), sceneKey, n)
    // scene groups are coarse: treat every non-kept photo of a scene as "rest"
    push('scene_keep_top', { selection: s, n, reject_rest: rejectRest }, L(`每个场景保留前 ${n} 张${rejectRest ? '，淘汰其余' : ''}`, `Keep the top ${n} of each scene${rejectRest ? ', reject the rest' : ''}`), g.keep.length + (rejectRest ? g.rest.length : 0), rejectRest)
  } else if (has(text, /挑(出)?(每组)?最佳|选(出)?最佳|最好的|每(个)?(连拍)?组|pick (the )?best|best (of|from) (each|every)|keep (the )?best|每组/) && !filterArgs.burst_best_only) {
    const n = keepN ?? 1
    const s = sel()
    const g = groupKeep(resolveSel(sessionId, ctx, s), burstKey, n)
    push('group_keep_top', { selection: s, n, reject_rest: rejectRest }, L(`每个连拍组保留前 ${n} 张${rejectRest ? '，淘汰其余' : '（标为精选）'}`, `Keep the top ${n} of each burst${rejectRest ? ', reject the rest' : ' (mark as picked)'}`), g.keep.length + (rejectRest ? g.rest.length : 0), rejectRest)
  }
  if (has(text, /统一(色调|风格)|套用|应用预设|调色|preset|unify|consistent (look|tone|color)|match (the )?(tone|look)/)) {
    const pw = PRESET_WORDS.find((w) => w[0].test(text)) ?? PRESET_WORDS[0]
    const s = sel()
    const n = count(s)
    push('apply_preset', { selection: s, preset_id: pw[1] }, L(`为 ${n} 张照片套用预设「${pw[2]}」，统一色调`, `Apply preset "${pw[3]}" to ${n} photos for a consistent look`), n)
  }
  if (has(text, /一键(修图|调整|优化)|自动(调整|修图|优化)|auto[- ]?(adjust|enhance|edit)|enhance all/)) {
    const s = sel()
    const n = count(s)
    push('auto_adjust', { selection: s, mode: 'auto' }, L(`对 ${n} 张照片逐张 AI 一键修图并保存`, `AI auto-adjust ${n} photos one by one and save`), n)
  }
  if (has(text, /接受.*(ai|AI)|accept ai|accept the ai/)) {
    const s = sel()
    const n = count(s)
    push('accept_ai', { selection: s }, L(`接受 ${n} 张照片的 AI 星级`, `Accept AI ratings of ${n} photos`), n)
  }
  if (bystander) {
    const s = sel()
    const n = count(s)
    push('remove_bystanders', { selection: s }, L(`消除 ${n} 张照片中的路人`, `Remove bystanders from ${n} photos`), n)
  }
  if (has(text, /美颜档案|人像档案|beauty profile|apply profiles?/)) {
    const s = sel()
    const n = count(s)
    push('apply_profiles', { selection: s }, L(`按人物美颜档案处理 ${n} 张照片`, `Apply beauty profiles to ${n} photos`), n)
  }
  if (has(text, /全员最佳|最佳合成|best take/)) {
    const s = sel()
    const n = count(s)
    push('besttake_auto', { selection: s }, L(`对所选连拍组一键全员最佳（${n} 张）`, `Best-take every selected burst (${n} photos)`), n)
  }
  if (has(text, /导出|export/)) {
    const preset = has(text, /微信|wechat/) ? 'wechat' : has(text, /小红书|xiaohongshu/) ? 'xiaohongshu' : has(text, /instagram|ins\b/) ? 'instagram' : 'original'
    const s = sel()
    const n = count(s)
    const names: Record<string, [string, string]> = { wechat: ['微信', 'WeChat'], xiaohongshu: ['小红书', 'Xiaohongshu'], instagram: ['Instagram', 'Instagram'], original: ['原图', 'original'] }
    push('export', { selection: s, preset }, L(`导出 ${n} 张照片（${names[preset][0]}尺寸）`, `Export ${n} photos (${names[preset][1]} size)`), n, true)
  }
  if (has(text, /描述|describe|caption/)) {
    const pid = ctx.current_photo_id
    if (pid !== null) push('describe', { photo_id: pid }, L('用 VLM 描述当前照片', 'Describe the current photo with the VLM'), 1)
  }
  if (has(text, /修图建议|怎么修|suggest|how (should|do) i (edit|fix)/)) {
    const pid = ctx.current_photo_id
    if (pid !== null) push('suggest_edits', { photo_id: pid }, L('分析当前照片并给出修图建议（不保存）', 'Analyse the current photo and suggest edits (not saved)'), 1)
  }

  const id = `plan-${++planSeq}`
  if (!steps.length) {
    const unsupported = L(
      '我还没听懂这条指令。可以试试：「只看 4 星以上」「挑出每组最佳」「淘汰闭眼的照片」「每个场景保留前 3 张」「统一色调」「消除路人」「一键修图」。',
      'I did not understand that. Try: "show 4 stars and above", "pick the best of each group", "reject the closed-eyes photos", "keep the top 3 of each scene", "unify the tone", "remove bystanders", "auto adjust".',
    )
    return { plan_id: id, reply: L('抱歉，我没能理解这条指令。', 'Sorry, I could not understand that.'), steps: [], needs_confirmation: false, engine: 'rules', unsupported }
  }
  const reply = steps.length === 1 && steps[0].tool === 'filter' ? L('已为你筛选。', 'Filtered for you.') : L(`好的，我准备了 ${steps.length} 个步骤，确认后执行。`, `OK, I prepared ${steps.length} step(s). Review and run.`)
  const out: AssistantPlan = { plan_id: id, reply, steps, needs_confirmation: steps.some((s) => s.destructive), engine: settings.assistant.engine === 'llm' ? 'llm' : 'rules', unsupported: null }
  plans.set(id, { plan: out, sessionId, ctx })
  return out
}

// ============================================================================
// assistant: execution
// ============================================================================

const stepMs = () => (globalThis as { __m6StepMs?: number }).__m6StepMs ?? 600

class Run {
  undoPhotos = new Map<number, AssistantUndo['photos'][number]>()
  undoEdits = new Map<number, EditStack>()
  photoItems = new Map<number, { id: number; user_rating?: number | null; flag?: Photo['flag']; color_label?: Photo['color_label'] }>()
  editItems = new Map<number, { id: number; has_edits: boolean; thumb_version: string }>()
  sessions = new Set<number>()

  patchPhoto(p: Photo, patch: { user_rating?: number | null; flag?: Photo['flag'] }) {
    if (!this.undoPhotos.has(p.id)) this.undoPhotos.set(p.id, { id: p.id, user_rating: p.user_rating, flag: p.flag, color_label: p.color_label })
    if ('user_rating' in patch) p.user_rating = patch.user_rating ?? null
    if ('flag' in patch && patch.flag !== undefined) p.flag = patch.flag
    this.sessions.add(p.session_id)
    this.photoItems.set(p.id, { id: p.id, user_rating: p.user_rating, flag: p.flag, color_label: p.color_label })
  }
  setStack(p: Photo, next: EditStack) {
    if (!this.undoEdits.has(p.id)) this.undoEdits.set(p.id, savedStack(p.id))
    store(p, normalizeStack(next))
    this.editItems.set(p.id, { id: p.id, has_edits: p.has_edits, thumb_version: p.thumb_version })
  }
  flush() {
    for (const sid of this.sessions) {
      const s = getSession(sid)
      if (s) {
        computeCounts(s)
        emit({ type: 'session.updated', session: { ...s.session } })
      }
    }
    if (this.photoItems.size) emit({ type: 'photos.updated', items: [...this.photoItems.values()] })
    if (this.editItems.size) emit({ type: 'edits.updated', items: [...this.editItems.values()] })
  }
  undo(): AssistantUndo {
    return { edits: [...this.undoEdits].map(([photo_id, before]) => ({ photo_id, before })), photos: [...this.undoPhotos.values()] }
  }
}

const tweak = (kind: 'bystanders' | 'profiles' | 'besttake', p: Photo): Adjust => {
  const base = autoAdjust(p, kind === 'profiles' ? 'portrait' : 'auto')
  return kind === 'profiles' ? { ...base, source: 'profile@1' } : { exposure: 0.1, clarity: kind === 'besttake' ? 4 : 0, shadows: 6, source: `${kind}@1` }
}

const PRESET_SECTIONS = ['global', 'lut'] as never

function runStep(run: Run, sp: StoredPlan, step: PlanStep): AssistantResult {
  const sel = resolveSel(sp.sessionId, sp.ctx, step.args.selection as Sel | undefined)
  switch (step.tool) {
    case 'filter':
      return { tool: step.tool, ok: true, affected: 0 }
    case 'set_rating':
      for (const p of sel) run.patchPhoto(p, { user_rating: (step.args.rating as number | null) ?? null })
      return { tool: step.tool, ok: true, affected: sel.length }
    case 'set_flag':
      for (const p of sel) run.patchPhoto(p, { flag: step.args.flag as Photo['flag'] })
      return { tool: step.tool, ok: true, affected: sel.length }
    case 'accept_ai': {
      let n = 0
      for (const p of sel)
        if (p.user_rating === null && p.ai_rating !== null) {
          run.patchPhoto(p, { user_rating: p.ai_rating })
          n++
        }
      return { tool: step.tool, ok: true, affected: n }
    }
    case 'group_keep_top':
    case 'scene_keep_top': {
      const g = groupKeep(sel, step.tool === 'group_keep_top' ? burstKey : sceneKey, (step.args.n as number) ?? 1)
      const byId = new Map(sel.map((p) => [p.id, p]))
      for (const id of g.keep) run.patchPhoto(byId.get(id)!, { flag: 1 })
      if (step.args.reject_rest) for (const id of g.rest) run.patchPhoto(byId.get(id)!, { flag: -1 })
      return { tool: step.tool, ok: true, affected: g.keep.length + (step.args.reject_rest ? g.rest.length : 0) }
    }
    case 'apply_preset': {
      const preset = BUILTIN.find((b) => b.id === step.args.preset_id)
      if (!preset) return { tool: step.tool, ok: false, affected: 0, error: `unknown preset ${String(step.args.preset_id)}` }
      for (const p of sel) run.setStack(p, applySections(savedStack(p.id), preset.stack, PRESET_SECTIONS))
      return { tool: step.tool, ok: true, affected: sel.length }
    }
    case 'auto_adjust':
      for (const p of sel) run.setStack(p, mergeAuto(savedStack(p.id), autoAdjust(p, 'auto')))
      return { tool: step.tool, ok: true, affected: sel.length }
    case 'apply_profiles':
    case 'besttake_auto':
    case 'remove_bystanders': {
      const kind = step.tool === 'apply_profiles' ? 'profiles' : step.tool === 'besttake_auto' ? 'besttake' : 'bystanders'
      for (const p of sel) run.setStack(p, mergeAuto(savedStack(p.id), tweak(kind, p)))
      return { tool: step.tool, ok: true, affected: sel.length }
    }
    case 'export':
      if (!step.args.dest) return { tool: step.tool, ok: false, affected: 0, error: 'dest_required' }
      return { tool: step.tool, ok: true, affected: sel.length }
    case 'describe': {
      const p = findPhoto(step.args.photo_id as number)
      return { tool: step.tool, ok: !!p, affected: p ? 1 : 0, data: p ? describePhoto(p) : undefined, error: p ? null : 'not_found' }
    }
    case 'suggest_edits': {
      const p = findPhoto(step.args.photo_id as number)
      return { tool: step.tool, ok: !!p, affected: p ? 1 : 0, data: p ? suggest(p) : undefined, error: p ? null : 'not_found' }
    }
    default:
      return { tool: step.tool, ok: false, affected: 0, error: 'unknown_tool' }
  }
}

function describePhoto(p: Photo) {
  const scene = p.scene_type ?? 'other'
  return { caption: `A ${scene} photo taken with ${p.camera ?? 'an unknown camera'}.`, keywords: [scene, p.camera ?? 'camera', p.format] }
}

function suggest(p: Photo) {
  const base = autoAdjust(p, p.scene_type === 'landscape' ? 'landscape' : p.scene_type === 'portrait' ? 'portrait' : 'auto')
  const problems = p.issues.length ? p.issues.map((i) => `issue:${i}`) : ['flat_contrast', 'dull_colors']
  return {
    problems,
    adjust: { ...base, source: 'vlm_suggest@1' },
    reason: 'Slightly lifted shadows and a gentle contrast / vibrance boost make the subject stand out without clipping highlights.',
  }
}

let taskSeq = 0
function startExecution(sp: StoredPlan): string {
  const task_id = `assistant-${++taskSeq}`
  const total = sp.plan.steps.length
  const run = new Run()
  const results: AssistantResult[] = []
  let i = 0
  emit({ type: 'task.progress', task_id, kind: 'assistant', done: 0, total, state: 'running' })
  const tick = () => {
    const step = sp.plan.steps[i]
    let r: AssistantResult
    try {
      r = runStep(run, sp, step)
    } catch (e) {
      r = { tool: step.tool, ok: false, affected: 0, error: e instanceof Error ? e.message : String(e) }
    }
    results.push(r)
    i++
    const finished = i >= total
    emit({ type: 'task.progress', task_id, kind: 'assistant', done: i, total, state: finished ? 'done' : 'running' })
    if (!finished) return void setTimeout(tick, stepMs())
    run.flush()
    emit({ type: 'assistant.done', plan_id: sp.plan.plan_id, ok: results.every((x) => x.ok), results, undo: run.undo() })
  }
  setTimeout(tick, stepMs())
  return task_id
}

// ============================================================================
// handlers
// ============================================================================

const status = (): AssistantStatus => ({
  engine: settings.assistant.engine === 'llm' ? 'llm' : 'rules',
  llm_model: 'qwen2.5-3b-instruct-q4',
  vlm_model: vlmAvailable ? 'qwen2.5-vl-3b-q4' : null,
  llm_available: true,
  vlm_available: vlmAvailable,
})

const windowMs = 60_000

export const m6Handlers = [
  http.get('/api/assistant/status', async () => {
    await lat()
    return json(status())
  }),
  http.post('/api/assistant/plan', async ({ request }) => {
    await delay(250 + Math.random() * 250)
    const body = (await request.json()) as { session_id: number; message: string; context: AssistantContext }
    if (!body?.message?.trim()) return err(400, 'bad_request', 'message required')
    if (!getSession(body.session_id)) return err(404, 'not_found', 'session not found')
    return json(plan(body.session_id, body.message, body.context ?? { filter: '', selection: [], current_photo_id: null, locale: 'zh-CN' }))
  }),
  http.post('/api/assistant/execute', async ({ request }) => {
    await lat()
    const { plan_id } = (await request.json()) as { plan_id: string }
    const sp = plans.get(plan_id)
    if (!sp) return err(404, 'not_found', 'plan not found or expired')
    return json({ task_id: startExecution(sp) }, 202)
  }),
  http.post('/api/assistant/describe', async ({ request }) => {
    await lat()
    if (!vlmAvailable) return err(503, 'vlm_unavailable', 'VLM is not available')
    const { photo_id } = (await request.json()) as { photo_id: number }
    const p = findPhoto(photo_id)
    return p ? json(describePhoto(p)) : err(404, 'not_found', 'photo not found')
  }),

  http.get('/api/settings', async () => {
    await lat()
    return json(settings)
  }),
  http.patch('/api/settings', async ({ request }) => {
    await lat()
    const patch = (await request.json()) as SettingsPatch
    if (patch.lan?.enabled && !auth.ownerPw) return err(400, 'password_required', 'set an owner password before enabling LAN access')
    if (patch.lan?.port !== undefined && (patch.lan.port < 1024 || patch.lan.port > 65535)) return err(400, 'bad_request', 'invalid port')
    if (patch.cache?.max_gb !== undefined && patch.cache.max_gb < 1) return err(400, 'bad_request', 'cache limit too small')
    settings = mergeSettings(settings, patch)
    if (patch.lan?.enabled !== undefined) setTimeout(() => (lanActive = settings.lan.enabled), 1200)
    emit({ type: 'settings.updated', settings })
    return json(settings)
  }),

  http.get('/api/cache', async () => {
    await lat()
    return json({ bytes: Object.values(cacheBytes).reduce((a, b) => a + b, 0), items: { ...cacheBytes } })
  }),
  http.post('/api/cache/clear', async ({ request }) => {
    await lat()
    const { kinds } = (await request.json()) as { kinds: (keyof typeof cacheBytes)[] }
    for (const k of kinds ?? []) if (k in cacheBytes) cacheBytes[k] = 0
    return no()
  }),

  http.delete('/api/models/:id', async ({ params }) => {
    await lat()
    const m = MODELS.find((x) => x.id === params.id)
    if (!m) return err(404, 'not_found', 'model not found')
    m.installed = false
    return no()
  }),

  http.delete('/api/faces', async ({ request }) => {
    await lat()
    if (new URL(request.url).searchParams.get('confirm') !== 'true') return err(400, 'bad_request', 'confirm=true required')
    clearFaceData()
    emit({ type: 'people.updated', session_id: 0 })
    return no()
  }),

  http.get('/api/onboarding', async () => {
    await lat()
    const rec = MODELS.filter((m) => m.required_for.includes('standard') && !m.installed)
    return json({
      first_run: firstRun,
      hardware: workerInfo(),
      recommended_tier: 'T3',
      recommended_download_mb: Math.round(rec.reduce((n, m) => n + m.size_mb, 0)),
      recommended_models: rec.map((m) => m.id),
    })
  }),
  http.post('/api/onboarding/done', async () => {
    await lat()
    firstRun = false
    return no()
  }),

  http.get('/api/system/lan', async () => {
    await lat()
    const urls = lanUrls()
    // restart_required: the enabled flag / port differ from what the (mock) server is actually listening on
    const active = settings.lan.enabled && lanActive
    return json({ enabled: settings.lan.enabled, urls: active ? urls : [], qr_svg: active ? fakeQr(urls[0]) : null, restart_required: settings.lan.enabled !== lanActive })
  }),

  http.get('/api/auth/me', async () => {
    await lat()
    return json({ role: auth.required ? auth.role : 'owner', lan: auth.required })
  }),
  http.post('/api/auth/login', async ({ request }) => {
    await lat()
    if (!auth.required) return err(403, 'lan_disabled', 'LAN access is not enabled')
    const now = Date.now()
    auth.failures = auth.failures.filter((t) => now - t < windowMs)
    if (auth.failures.length >= 5) {
      const retry = Math.max(1, Math.ceil((windowMs - (now - auth.failures[0])) / 1000))
      return HttpResponse.json(
        { error: { code: 'too_many_requests', message: 'too many attempts' }, retry_after: retry },
        { status: 429, headers: { 'Retry-After': String(retry) } },
      )
    }
    const { password } = (await request.json()) as { password: string }
    const role: Role | null = password && password === auth.ownerPw ? 'owner' : password && settings.lan.guest_enabled && password === auth.guestPw ? 'guest' : null
    if (!role) {
      auth.failures.push(now)
      return err(401, 'unauthorized', 'wrong password')
    }
    auth.role = role
    saveRole(role)
    return json({ role })
  }),
  http.post('/api/auth/logout', async () => {
    auth.role = null
    saveRole(null)
    return no()
  }),
  http.post('/api/auth/password', async ({ request }) => {
    await lat()
    const { password, guest_password } = (await request.json()) as { password: string; guest_password?: string }
    const ok = (p: string) => p.length >= 8 && p.length <= 256
    if (!password || !ok(password)) return err(400, 'bad_request', 'password must be 8-256 characters')
    if (guest_password !== undefined && guest_password !== '' && (!ok(guest_password) || guest_password === password))
      return err(400, 'bad_request', 'guest password must be 8-256 characters and differ from the owner password')
    auth.ownerPw = password
    if (guest_password !== undefined) auth.guestPw = guest_password === '' ? null : guest_password
    return json({ ok: true })
  }),

  http.post('/api/xmp/sync', async ({ request }) => {
    await delay(300)
    const { session_id, direction } = (await request.json()) as { session_id: number; direction: 'read' | 'write' }
    const s = getSession(session_id)
    if (!s) return err(404, 'not_found', 'session not found')
    if (direction !== 'read' && direction !== 'write') return err(400, 'bad_request', 'bad direction')
    if (settings.xmp_mode === 'off') return err(409, 'xmp_off', 'XMP interop is switched off')
    return json({ updated: Math.min(s.photos.length, 42) })
  }),
]
