import { ApiError } from '@/api/client'
import i18n from '@/i18n'
import { errorCodeKey } from './auth'

/** User-facing text of a failed request: known 403 codes get a clear explanation, everything else the server message. */
export function errorText(e: unknown): string {
  if (e instanceof ApiError) {
    const key = errorCodeKey(e.code)
    if (key) return i18n.t(key) as string
  }
  return e instanceof Error ? e.message : String(e)
}
