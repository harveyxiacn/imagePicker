export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`
}

export function formatShutter(s: number | null): string {
  if (s === null) return '—'
  if (s >= 1) return `${s}s`
  return `1/${Math.round(1 / s)}s`
}

/**
 * Capture time as the wall clock where the photo was taken (not the viewer's time zone).
 * `offsetMin` is the UTC offset recorded in EXIF; when unknown, `ms` already encodes the
 * naive wall-clock time as UTC. Either way, render in UTC after applying the offset.
 */
export function formatDate(ms: number | null, locale?: string, offsetMin?: number | null): string {
  if (ms === null) return '—'
  const wall = new Date(ms + (offsetMin ?? 0) * 60_000).toLocaleString(locale, { hour12: false, timeZone: 'UTC' })
  if (offsetMin == null) return wall
  const sign = offsetMin < 0 ? '−' : '+'
  const a = Math.abs(offsetMin)
  return `${wall} (UTC${sign}${String(Math.floor(a / 60)).padStart(2, '0')}:${String(a % 60).padStart(2, '0')})`
}

export function formatDims(w: number | null, h: number | null): string {
  if (!w || !h) return '—'
  return `${w} × ${h} (${((w * h) / 1e6).toFixed(1)} MP)`
}

export function fileBaseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

/** Join a directory entry from /api/fs/list: entries may be full paths or bare names. */
export function resolveDirEntry(parent: string, entry: string): string {
  if (/[\\/]/.test(entry)) return entry
  const sep = parent.includes('\\') ? '\\' : '/'
  return parent.endsWith(sep) ? parent + entry : parent + sep + entry
}

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v))

/** "10月1日 14:32" style capture clock (wall time at the location, like formatDate). */
export function formatClock(ms: number, locale?: string, offsetMin?: number | null, withDate = true): string {
  return new Date(ms + (offsetMin ?? 0) * 60_000).toLocaleString(locale, {
    timeZone: 'UTC',
    hour12: false,
    ...(withDate ? { month: 'short', day: 'numeric' } : {}),
    hour: '2-digit',
    minute: '2-digit',
  })
}
