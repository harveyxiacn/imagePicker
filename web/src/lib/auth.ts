/**
 * LAN-mode authentication helpers (docs/api-contract-m6.md B): 401 -> login redirect logic (loop free),
 * role gating, and the module-level role used by non-React code (actions, shortcuts).
 */
import type { Role } from '@/api/types'

export const LOGIN_PATH = '/login'

export type Capability = 'view' | 'rate' | 'edit' | 'export' | 'people' | 'settings' | 'assistant' | 'analyze'

/** Owners (and the desktop app, which reports `owner`) may do everything; guests are read-only; unauthenticated nothing. */
export function can(role: Role | null | undefined, cap: Capability): boolean {
  if (role === 'owner') return true
  if (role === 'guest') return cap === 'view'
  return false
}

let currentRole: Role | null = 'owner'
/** Set by the app shell from `/api/auth/me`; non-React code reads it through `getRole()`. */
export const setRole = (r: Role | null): void => {
  currentRole = r
}
export const getRole = (): Role | null => currentRole
export const canNow = (cap: Capability): boolean => can(currentRole, cap)

/** Only same-origin app paths are accepted as a post-login target (no open redirect, never the login page itself). */
export function sanitizeNext(next: string | null | undefined): string {
  if (!next || !next.startsWith('/') || next.startsWith('//') || next.includes('\\')) return '/'
  if (next === LOGIN_PATH || next.startsWith(`${LOGIN_PATH}?`) || next.startsWith(`${LOGIN_PATH}/`)) return '/'
  return next
}

export const loginUrl = (current: { pathname: string; search?: string }): string => {
  const next = sanitizeNext(current.pathname + (current.search ?? ''))
  return next === '/' ? LOGIN_PATH : `${LOGIN_PATH}?next=${encodeURIComponent(next)}`
}

const LOGOUT_GRACE_MS = 3000
let loggedOutAt = -Infinity
/** The user signed out on purpose: the login page that follows must not remember the route they left. */
export const noteLogout = (now = Date.now()): void => {
  loggedOutAt = now
}
/** Where to send an unauthenticated user: `/login?next=<here>`, or plain `/login` right after an explicit logout. */
export const loginTarget = (current: { pathname: string; search?: string }, now = Date.now()): string =>
  now - loggedOutAt < LOGOUT_GRACE_MS ? LOGIN_PATH : loginUrl(current)

/**
 * Should a 401 on `requestPath` (relative to /api) navigate to the login page?
 * Never for the auth endpoints themselves (a wrong password is a 401 too) and never while already on /login.
 */
export function shouldRedirectOn401(requestPath: string, currentPathname: string): boolean {
  if (requestPath === '/auth' || requestPath.startsWith('/auth/')) return false
  if (currentPathname === LOGIN_PATH) return false
  return true
}

/** Login failure -> UX message key. */
export function loginErrorKey(status: number, code?: string): 'rateLimited' | 'wrong' | 'lanDisabled' | 'failed' {
  if (status === 429) return 'rateLimited'
  if (status === 403 && code === 'lan_disabled') return 'lanDisabled'
  if (status === 401 || status === 403) return 'wrong'
  return 'failed'
}

/** Seconds to wait after a 429 `too_many_requests` (`retry_after` in the body; defaults to 60). */
export function retryAfterOf(body: unknown): number {
  const b = body as { retry_after?: unknown; error?: { retry_after?: unknown } } | undefined
  const v = b?.retry_after ?? b?.error?.retry_after
  return typeof v === 'number' && Number.isFinite(v) && v > 0 ? Math.ceil(v) : 60
}

/** 403 / error codes -> i18n key for a clear message (null = use the server message). */
export function errorCodeKey(code: string): string | null {
  switch (code) {
    case 'forbidden_path':
      return 'errors.forbiddenPath'
    case 'forbidden':
      return 'errors.forbidden'
    case 'forbidden_origin':
      return 'errors.forbiddenOrigin'
    case 'lan_disabled':
      return 'errors.lanDisabled'
    default:
      return null
  }
}

/** Password rules of POST /api/auth/password: 8-256 chars, guest differs from owner. */
export const PASSWORD_MIN = 8
export const PASSWORD_MAX = 256
export const passwordValid = (p: string): boolean => p.length >= PASSWORD_MIN && p.length <= PASSWORD_MAX
