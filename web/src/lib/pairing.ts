/** Remote AI pairing (docs/api-contract-m8.md section C): input validation + a small state machine. */

export const CODE_LENGTH = 6

/** Keeps only digits, at most 6 (so pasted "123 456" or "123-456" work). */
export function normalizeCode(raw: string): string {
  return raw.replace(/\D/g, '').slice(0, CODE_LENGTH)
}

export const isValidCode = (code: string): boolean => new RegExp(`^\\d{${CODE_LENGTH}}$`).test(code)

/** Accepts `192.168.1.5:7878`, `http://host:7878/`, `host.local`; returns a normalised origin or null. */
export function normalizeHostUrl(raw: string): string | null {
  let s = raw.trim()
  if (!s) return null
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) s = `http://${s}`
  let u: URL
  try {
    u = new URL(s)
  } catch {
    return null
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return null
  if (!u.hostname) return null
  return u.origin
}

export interface PairInput {
  url: string
  code: string
}

export interface PairErrors {
  url?: 'required' | 'invalid'
  code?: 'required' | 'invalid'
}

export function validatePairForm(url: string, code: string): { ok: boolean; errors: PairErrors; value: PairInput | null } {
  const errors: PairErrors = {}
  const origin = normalizeHostUrl(url)
  if (!url.trim()) errors.url = 'required'
  else if (!origin) errors.url = 'invalid'
  if (!code) errors.code = 'required'
  else if (!isValidCode(code)) errors.code = 'invalid'
  const ok = !errors.url && !errors.code
  return { ok, errors, value: ok && origin ? { url: origin, code } : null }
}

/** Parses the QR payload `imagepicker://pair?url=<host>&code=<code>`. */
export function parsePairUri(text: string): PairInput | null {
  const m = /^imagepicker:\/\/pair\?(.*)$/i.exec(text.trim())
  if (!m) return null
  const q = new URLSearchParams(m[1])
  const url = normalizeHostUrl(q.get('url') ?? '')
  const code = normalizeCode(q.get('code') ?? '')
  return url && isValidCode(code) ? { url, code } : null
}

// ---------------------------------------------------------------- phone-side state machine

export type PairState = { phase: 'idle'; error: string | null } | { phase: 'submitting'; input: PairInput } | { phase: 'connected' }

export type PairEvent =
  | { type: 'submit'; input: PairInput }
  | { type: 'success' }
  | { type: 'failure'; error: string }
  | { type: 'disconnect' }
  | { type: 'edit' }

export const PAIR_INITIAL: PairState = { phase: 'idle', error: null }

export function pairReducer(s: PairState, e: PairEvent): PairState {
  switch (e.type) {
    case 'submit':
      return s.phase === 'submitting' ? s : { phase: 'submitting', input: e.input }
    case 'success':
      return s.phase === 'submitting' ? { phase: 'connected' } : s
    case 'failure':
      return s.phase === 'submitting' ? { phase: 'idle', error: e.error } : s
    case 'disconnect':
      return { phase: 'idle', error: null }
    case 'edit':
      return s.phase === 'idle' && s.error ? { phase: 'idle', error: null } : s
  }
}

// ---------------------------------------------------------------- host side

/** Whole seconds left until `expiresAt` (unix seconds or ms; ISO strings accepted), never negative. */
export function secondsLeft(expiresAt: number | string, nowMs: number): number {
  const ms = typeof expiresAt === 'string' ? Date.parse(expiresAt) : expiresAt < 1e12 ? expiresAt * 1000 : expiresAt
  if (!Number.isFinite(ms)) return 0
  return Math.max(0, Math.ceil((ms - nowMs) / 1000))
}

export function formatCountdown(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

/** Spaced for reading: `123 456`. */
export const formatCode = (code: string): string => (code.length === CODE_LENGTH ? `${code.slice(0, 3)} ${code.slice(3)}` : code)

/** Largest sensible unit for "N units ago" (feeds `Intl.RelativeTimeFormat`). */
export function agoParts(elapsedSeconds: number): { unit: 'second' | 'minute' | 'hour' | 'day'; value: number } {
  const s = Math.max(0, Math.floor(elapsedSeconds))
  if (s < 60) return { unit: 'second', value: s }
  if (s < 3600) return { unit: 'minute', value: Math.floor(s / 60) }
  if (s < 86400) return { unit: 'hour', value: Math.floor(s / 3600) }
  return { unit: 'day', value: Math.floor(s / 86400) }
}

export function formatAgo(unixSeconds: number | null, nowMs: number, lang: string): string {
  if (unixSeconds === null) return ''
  const { unit, value } = agoParts(nowMs / 1000 - unixSeconds)
  return new Intl.RelativeTimeFormat(lang, { numeric: 'auto' }).format(-value, unit)
}
