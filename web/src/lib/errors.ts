import { ApiError } from '@/api/client'
import i18n from '@/i18n'
import { errorCodeKey } from './auth'

/** API error code -> dictionary key. Codes listed here are fully explained by the localized text (no server detail is appended). */
const CODE_KEYS: Record<string, string> = {
  not_found: 'errors.notFound',
  bad_request: 'errors.badRequest',
  unprocessable: 'errors.unprocessable',
  models_missing: 'errors.modelsMissing',
  worker_unavailable: 'errors.workerUnavailable',
  worker_timeout: 'errors.workerTimeout',
  confirmation_required: 'errors.confirmationRequired',
  invalid_code: 'errors.invalidCode',
  pair_expired: 'errors.pairExpired',
  host_unreachable: 'errors.hostUnreachable',
  rate_limited: 'errors.rateLimited',
  too_many_requests: 'errors.rateLimited',
  hair_pose_too_different: 'errors.hairPoseTooDifferent',
  hair_pose_unknown: 'errors.hairPoseUnknown',
  hair_person_mismatch: 'errors.hairPersonMismatch',
}

/** Codes without a precise message: the generic localized line plus the short server detail. */
const CODE_GENERIC_KEYS: Record<string, string> = {
  worker_error: 'errors.workerError',
  pair_failed: 'errors.pairFailed',
  hair_refused: 'errors.unprocessable',
}

/** HTTP status -> dictionary key (used when the code is unknown). */
export function statusKey(status: number): string {
  if (status === 0) return 'errors.network'
  if (status === 401) return 'errors.unauthorized'
  if (status === 403) return 'errors.forbidden'
  if (status === 404) return 'errors.notFound'
  if (status === 408 || status === 504) return 'errors.timeout'
  if (status === 409) return 'errors.conflict'
  if (status === 413) return 'errors.tooLarge'
  if (status === 422) return 'errors.unprocessable'
  if (status === 429) return 'errors.rateLimited'
  if (status === 503) return 'errors.workerUnavailable'
  if (status >= 500) return 'errors.server'
  if (status >= 400) return 'errors.badRequest'
  return 'errors.unknown'
}

const DETAIL_MAX = 140

export interface ErrorParts {
  /** dictionary key of the localized message */
  key: string
  /** short raw server/JS text, shown only as a secondary hint (never the only text) */
  detail?: string
}

const clip = (s: string): string => {
  const one = s.replace(/\s+/g, ' ').trim()
  return one.length > DETAIL_MAX ? `${one.slice(0, DETAIL_MAX - 1)}…` : one
}

/** Map any thrown value (ApiError, network TypeError, abort, plain string, task error text) to a key + optional raw detail. */
export function errorParts(e: unknown): ErrorParts {
  if (e instanceof ApiError) {
    const key = errorCodeKey(e.code) ?? CODE_KEYS[e.code]
    if (key) return { key }
    const generic = CODE_GENERIC_KEYS[e.code] ?? statusKey(e.status)
    // the synthesized "500 Internal Server Error" line carries nothing the status text does not
    const synthetic = /^\d{3}\b/.test(e.message)
    return { key: generic, detail: e.message && !synthetic ? clip(e.message) : undefined }
  }
  if (e instanceof DOMException && (e.name === 'AbortError' || e.name === 'TimeoutError')) return { key: 'errors.timeout' }
  if (e instanceof TypeError) return { key: 'errors.network' }
  const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : e == null ? '' : String(e)
  return { key: 'errors.unknown', detail: msg ? clip(msg) : undefined }
}

/** Localized, user-facing text of a failed request/task. Unknown failures get a generic line with the short server detail in parentheses. */
export function errorText(e: unknown): string {
  const { key, detail } = errorParts(e)
  const text = i18n.t(key) as string
  return detail ? (i18n.t('errors.withDetail', { message: text, detail }) as string) : text
}
