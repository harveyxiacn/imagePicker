import i18n from '@/i18n'

/** Dictionary key for a backend reason / warning code: `<ns>.<prefix>_<code>` when it exists, else `<ns>.<prefix>_unknown` (never the raw snake_case). */
export function reasonKey(ns: 'besttake' | 'hair', code: string | null | undefined, prefix: 'reason' | 'warn' = 'reason', has: (k: string) => boolean = (k) => i18n.exists(k)): string {
  // some servers send the already-prefixed "reason_unknown"
  const c = (code ?? '').replace(/^reason_/, '')
  const key = `${ns}.${prefix}_${c}`
  return c && has(key) ? key : `${ns}.${prefix}_unknown`
}

/** Localized text of a backend reason / warning code. */
export function reasonText(ns: 'besttake' | 'hair', code: string | null | undefined, prefix: 'reason' | 'warn' = 'reason'): string {
  return i18n.t(reasonKey(ns, code, prefix)) as string
}
