export type HapticKind = 'tick' | 'pick' | 'reject' | 'select'

const PATTERNS: Record<HapticKind, number | number[]> = { tick: 8, pick: 18, reject: [14, 40, 14], select: 25 }

/** Light haptic feedback where `navigator.vibrate` exists (Android WebView / Chrome); a silent no-op elsewhere. */
export function haptic(kind: HapticKind): boolean {
  try {
    if (typeof navigator !== 'undefined' && typeof navigator.vibrate === 'function') return navigator.vibrate(PATTERNS[kind])
  } catch {
    /* vibration can be blocked by permissions policy */
  }
  return false
}
