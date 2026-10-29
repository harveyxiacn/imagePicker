import { describe, expect, it } from 'vitest'
import { ApiError } from '@/api/client'
import { errorParts, errorText, statusKey } from './errors'
import i18n from '@/i18n'

const api = (status: number, code: string, message = 'boom') => new ApiError(status, code, message)

describe('errorParts', () => {
  it('maps known codes to keys without server detail', () => {
    expect(errorParts(api(403, 'forbidden_path'))).toEqual({ key: 'errors.forbiddenPath' })
    expect(errorParts(api(409, 'models_missing'))).toEqual({ key: 'errors.modelsMissing' })
    expect(errorParts(api(503, 'worker_unavailable'))).toEqual({ key: 'errors.workerUnavailable' })
    expect(errorParts(api(504, 'worker_timeout'))).toEqual({ key: 'errors.workerTimeout' })
    expect(errorParts(api(422, 'hair_pose_too_different'))).toEqual({ key: 'errors.hairPoseTooDifferent' })
  })

  it('falls back to the status class for unknown codes, keeping a short detail', () => {
    expect(errorParts(api(404, 'weird', 'no such thing'))).toEqual({ key: 'errors.notFound', detail: 'no such thing' })
    expect(errorParts(api(500, 'internal', 'sqlite: disk I/O error'))).toEqual({ key: 'errors.server', detail: 'sqlite: disk I/O error' })
    expect(errorParts(api(502, 'worker_error', 'cuda oom')).key).toBe('errors.workerError')
  })

  it('does not echo the synthesized "status text" message', () => {
    expect(errorParts(api(500, 'http_error', '500 Internal Server Error'))).toEqual({ key: 'errors.server', detail: undefined })
  })

  it('covers status classes', () => {
    const cases: [number, string][] = [
      [0, 'errors.network'], [401, 'errors.unauthorized'], [403, 'errors.forbidden'], [404, 'errors.notFound'],
      [408, 'errors.timeout'], [409, 'errors.conflict'], [413, 'errors.tooLarge'], [422, 'errors.unprocessable'],
      [429, 'errors.rateLimited'], [503, 'errors.workerUnavailable'], [500, 'errors.server'], [400, 'errors.badRequest'],
    ]
    for (const [s, k] of cases) expect(statusKey(s)).toBe(k)
  })

  it('maps network and abort errors', () => {
    expect(errorParts(new TypeError('Failed to fetch')).key).toBe('errors.network')
    expect(errorParts(new DOMException('x', 'AbortError')).key).toBe('errors.timeout')
  })

  it('unknown values get the generic key with a clipped detail', () => {
    expect(errorParts(new Error('x'.repeat(500))).detail!.length).toBeLessThanOrEqual(140)
    expect(errorParts('plain task error')).toEqual({ key: 'errors.unknown', detail: 'plain task error' })
    expect(errorParts(undefined)).toEqual({ key: 'errors.unknown', detail: undefined })
  })
})

describe('errorText', () => {
  it('is localized and never only the raw text', () => {
    const txt = errorText(new Error('raw boom'))
    expect(txt).toContain(i18n.t('errors.unknown'))
    expect(txt).not.toBe('raw boom')
    expect(errorText(api(409, 'models_missing', 'raw'))).toBe(i18n.t('errors.modelsMissing'))
  })
})
