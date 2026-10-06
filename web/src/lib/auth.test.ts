import { describe, expect, it } from 'vitest'
import { can, canNow, loginErrorKey, loginTarget, loginUrl, noteLogout, passwordValid, retryAfterOf, errorCodeKey, sanitizeNext, setRole, shouldRedirectOn401 } from './auth'

describe('401 -> login redirect', () => {
  it('redirects for ordinary API calls', () => {
    expect(shouldRedirectOn401('/photos?session_id=1', '/s/1')).toBe(true)
  })
  it('never redirects for auth endpoints (wrong password is a 401 too)', () => {
    expect(shouldRedirectOn401('/auth/login', '/s/1')).toBe(false)
    expect(shouldRedirectOn401('/auth/me', '/s/1')).toBe(false)
  })
  it('does not loop while on the login page', () => {
    expect(shouldRedirectOn401('/sessions', '/login')).toBe(false)
  })
  it('builds a login url that returns to the previous route', () => {
    expect(loginUrl({ pathname: '/s/3/edit/9', search: '?x=1' })).toBe('/login?next=%2Fs%2F3%2Fedit%2F9%3Fx%3D1')
    expect(loginUrl({ pathname: '/' })).toBe('/login')
    expect(loginUrl({ pathname: '/login', search: '?next=%2Fa' })).toBe('/login')
  })
  it('forgets the route after an explicit logout (briefly)', () => {
    expect(loginTarget({ pathname: '/s/2' }, 1_000_000)).toBe('/login?next=%2Fs%2F2')
    noteLogout(1_000_000)
    expect(loginTarget({ pathname: '/s/2' }, 1_000_500)).toBe('/login')
    expect(loginTarget({ pathname: '/s/2' }, 1_010_000)).toBe('/login?next=%2Fs%2F2')
  })
  it('sanitizes the post-login target', () => {
    expect(sanitizeNext('/s/1')).toBe('/s/1')
    expect(sanitizeNext('//evil.com')).toBe('/')
    expect(sanitizeNext('https://evil.com')).toBe('/')
    expect(sanitizeNext('/login')).toBe('/')
    expect(sanitizeNext('/login?next=/x')).toBe('/')
    expect(sanitizeNext(null)).toBe('/')
    expect(sanitizeNext('/\\evil')).toBe('/')
  })
  it('maps login failures', () => {
    expect(loginErrorKey(429)).toBe('rateLimited')
    expect(loginErrorKey(401)).toBe('wrong')
    expect(loginErrorKey(500)).toBe('failed')
    expect(loginErrorKey(403, 'lan_disabled')).toBe('lanDisabled')
  })
  it('reads retry_after and error codes', () => {
    expect(retryAfterOf({ retry_after: 12.2 })).toBe(13)
    expect(retryAfterOf({})).toBe(60)
    expect(errorCodeKey('forbidden_path')).toBe('errors.forbiddenPath')
    expect(errorCodeKey('nope')).toBeNull()
  })
  it('validates passwords (8-256)', () => {
    expect(passwordValid('1234567')).toBe(false)
    expect(passwordValid('12345678')).toBe(true)
    expect(passwordValid('x'.repeat(257))).toBe(false)
  })
})

describe('role gating', () => {
  it('owner can do everything, guest only view, anonymous nothing', () => {
    for (const c of ['view', 'rate', 'edit', 'export', 'people', 'settings', 'assistant', 'analyze'] as const) {
      expect(can('owner', c)).toBe(true)
      expect(can('guest', c)).toBe(c === 'view')
      expect(can(null, c)).toBe(false)
    }
  })
  it('module role feeds canNow', () => {
    setRole('guest')
    expect(canNow('edit')).toBe(false)
    expect(canNow('view')).toBe(true)
    setRole('owner')
    expect(canNow('edit')).toBe(true)
  })
})
