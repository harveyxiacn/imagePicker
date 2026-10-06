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

export function formatDate(ms: number | null, locale?: string): string {
  if (ms === null) return '—'
  return new Date(ms).toLocaleString(locale, { hour12: false })
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
