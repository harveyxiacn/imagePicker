/**
 * Import from a memory card (PRD F1.8, docs/api-contract-m1.md "从存储卡导入"): destination pattern presets, the
 * folder preview, and the summary of a finished `card_import` task. Pure helpers; the UI is components/CardImport.tsx.
 */
import type { CardFailure, CardReport, ImportSource, TaskRecord } from '@/api/types'

export const CARD_TASK_KIND = 'card_import'

/** Folder structures offered in the dialog; the server accepts any valid pattern (tokens {yyyy} {mm} {dd} {yyyy-mm-dd} {yyyymmdd}). */
export const PATTERN_PRESETS = [
  { id: 'yearDay', pattern: '{yyyy}/{yyyy-mm-dd}' },
  { id: 'day', pattern: '{yyyy-mm-dd}' },
  { id: 'yearMonth', pattern: '{yyyy}/{mm}' },
  { id: 'flat', pattern: '' },
] as const

const pad = (n: number, w = 2) => String(n).padStart(w, '0')

/** The folders (below the destination) a photo taken on `date` goes to, like the server expands the pattern. */
export function expandPattern(pattern: string, date: Date): string[] {
  const p = pattern.trim()
  if (!p) return []
  const [y, m, d] = [date.getFullYear(), date.getMonth() + 1, date.getDate()]
  return p.split('/').map((c) =>
    c
      .replaceAll('{yyyy-mm-dd}', `${pad(y, 4)}-${pad(m)}-${pad(d)}`)
      .replaceAll('{yyyymmdd}', `${pad(y, 4)}${pad(m)}${pad(d)}`)
      .replaceAll('{yyyy}', pad(y, 4))
      .replaceAll('{mm}', pad(m))
      .replaceAll('{dd}', pad(d)),
  )
}

/** `dest` + the pattern's folders for `date`, with the separator `dest` already uses. */
export function previewPath(dest: string, pattern: string, date: Date): string {
  const parts = expandPattern(pattern, date)
  if (parts.length === 0) return dest
  const sep = dest.includes('\\') && !dest.includes('/') ? '\\' : '/'
  return [dest.replace(/[\\/]+$/, '') || dest, ...parts].join(sep)
}

/** "1.5 GB"; null when unknown. */
export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n) || n < 0) return ''
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let v = n
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${i === 0 || v >= 10 ? Math.round(v) : v.toFixed(1)} ${units[i]}`
}

/** i18n key + options of a card's one-line description (photo estimate, free space). */
export function cardCaption(s: Pick<ImportSource, 'photo_estimate' | 'estimate_truncated' | 'free_bytes' | 'total_bytes'>): { key: string; opts: Record<string, unknown> }[] {
  const out: { key: string; opts: Record<string, unknown> }[] = [
    { key: s.estimate_truncated ? 'card.photosMore' : 'card.photos', opts: { n: s.photo_estimate } },
  ]
  if (s.total_bytes !== null && s.free_bytes !== null) out.push({ key: 'card.space', opts: { free: formatBytes(s.free_bytes), total: formatBytes(s.total_bytes) } })
  return out
}

const num = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0)

/** `params.report` of a finished (or running) card import, defensively parsed; null while there is none. */
export function cardReportOf(rec: Pick<TaskRecord, 'params'>): CardReport | null {
  const r = rec.params?.report
  if (!r || typeof r !== 'object') return null
  const o = r as Record<string, unknown>
  const failed = Array.isArray(o.failed)
    ? (o.failed as unknown[]).flatMap((f): CardFailure[] => {
        const x = f as Record<string, unknown> | null
        return x && typeof x.path === 'string' ? [{ path: x.path, error: typeof x.error === 'string' ? x.error : '' }] : []
      })
    : []
  return {
    found: num(o.found),
    skipped_existing: num(o.skipped_existing),
    copied: num(o.copied),
    reused: num(o.reused),
    bytes: num(o.bytes),
    failed_count: num(o.failed_count) || failed.length,
    failed,
    not_attempted: num(o.not_attempted),
    cancelled: o.cancelled === true,
    aborted: typeof o.aborted === 'string' ? o.aborted : null,
    session_id: typeof o.session_id === 'number' ? o.session_id : null,
  }
}

export interface SummaryLine {
  key: string
  opts?: Record<string, unknown>
}

export interface CardSummary {
  tone: 'success' | 'warning' | 'error' | 'muted'
  headline: SummaryLine
  lines: SummaryLine[]
  failed: CardFailure[]
  sessionId: number | null
}

/** What to tell the user about a task that is no longer running. */
export function cardSummary(rec: Pick<TaskRecord, 'status' | 'error' | 'params'>): CardSummary {
  const r = cardReportOf(rec) ?? {
    found: 0, skipped_existing: 0, copied: 0, reused: 0, bytes: 0, failed_count: 0, failed: [], not_attempted: 0, cancelled: false, aborted: null, session_id: null,
  }
  const imported = r.copied + r.reused
  const lines: SummaryLine[] = []
  if (r.copied > 0) lines.push({ key: 'card.sum_copied', opts: { n: r.copied, size: formatBytes(r.bytes) } })
  if (r.reused > 0) lines.push({ key: 'card.sum_reused', opts: { n: r.reused } })
  if (r.skipped_existing > 0) lines.push({ key: 'card.sum_skipped', opts: { n: r.skipped_existing } })
  if (r.failed_count > 0) lines.push({ key: 'card.sum_failed', opts: { n: r.failed_count } })
  if (r.not_attempted > 0) lines.push({ key: 'card.sum_left', opts: { n: r.not_attempted } })
  if (r.aborted) lines.push({ key: 'card.sum_aborted', opts: { msg: r.aborted } })
  const base = { lines, failed: r.failed, sessionId: r.session_id }
  if (rec.status === 'interrupted') return { ...base, tone: 'error', headline: { key: 'card.result_interrupted' } }
  if (rec.status === 'cancelled' || r.cancelled) return { ...base, tone: 'muted', headline: { key: 'card.result_cancelled', opts: { n: imported } } }
  if (rec.status === 'failed') return { ...base, tone: 'error', headline: { key: 'card.result_failed', opts: { msg: rec.error ?? r.aborted ?? '' } } }
  if (imported === 0 && r.failed_count === 0) return { ...base, tone: 'muted', headline: { key: 'card.result_nothing' } }
  if (r.failed_count > 0 || r.aborted) return { ...base, tone: 'warning', headline: { key: 'card.result_partial', opts: { n: imported } } }
  return { ...base, tone: 'success', headline: { key: 'card.result_ok', opts: { n: imported } } }
}

const DEST_KEY = 'ip.cardDest'

/** The destination used last time ('' when none or storage is unavailable). */
export function loadCardDest(): string {
  try {
    return localStorage.getItem(DEST_KEY) ?? ''
  } catch {
    return ''
  }
}

export function saveCardDest(path: string): void {
  try {
    localStorage.setItem(DEST_KEY, path)
  } catch {
    /* private mode / blocked storage: the next import just asks again */
  }
}
