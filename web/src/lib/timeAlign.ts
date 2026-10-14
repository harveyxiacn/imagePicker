import type { DeviceOffset } from '@/api/types'

/** Confidence from which "accept all" takes an estimate (same as the server's `ACCEPT_MIN_CONFIDENCE`). */
export const ACCEPT_MIN_CONFIDENCE = 0.5
/** Largest correction that can be typed in (the server refuses more): 7 days. */
export const MAX_MANUAL_OFFSET_MS = 7 * 24 * 3_600_000

/** `+1h 02m 05s`, `-42s`, `0s`: a clock offset to the second (sub-second parts are rounded). */
export function formatOffset(ms: number): string {
  const total = Math.round(ms / 1000)
  if (total === 0) return '0s'
  const sign = total < 0 ? '-' : '+'
  const abs = Math.abs(total)
  const h = Math.floor(abs / 3600)
  const m = Math.floor((abs % 3600) / 60)
  const s = abs % 60
  const pad = (n: number) => String(n).padStart(2, '0')
  if (h > 0) return `${sign}${h}h ${pad(m)}m ${pad(s)}s`
  if (m > 0) return `${sign}${m}m ${pad(s)}s`
  return `${sign}${s}s`
}

/**
 * Reads an offset typed by the user, in ms; `null` when it is not understood. Accepts
 * `1h 2m 5s` / `-90s` / `+2m` (units), `1:02:05` or `1:02` (hours:minutes[:seconds]) and a plain
 * number of seconds (`-3725`, `42.5`). A leading `+`/`-` (or the typographic minus) sets the sign.
 */
export function parseOffset(input: string): number | null {
  const text = input.trim().replace(/[−–]/g, '-').toLowerCase()
  if (!text) return null
  const m = /^([+-]?)\s*(.+)$/.exec(text)
  if (!m) return null
  const sign = m[1] === '-' ? -1 : 1
  const body = m[2].trim()
  let seconds: number | null = null
  let hit: RegExpExecArray | null
  if ((hit = /^(\d+(?:\.\d+)?)$/.exec(body))) {
    seconds = Number(hit[1])
  } else if ((hit = /^(\d+):(\d{1,2})(?::(\d{1,2}(?:\.\d+)?))?$/.exec(body))) {
    const mm = Number(hit[2])
    const ss = hit[3] === undefined ? 0 : Number(hit[3])
    if (mm >= 60 || ss >= 60) return null
    seconds = Number(hit[1]) * 3600 + mm * 60 + ss
  } else if ((hit = /^(?:(\d+)\s*h)?\s*(?:(\d+)\s*m)?\s*(?:(\d+(?:\.\d+)?)\s*s)?$/.exec(body)) && (hit[1] || hit[2] || hit[3])) {
    seconds = Number(hit[1] ?? 0) * 3600 + Number(hit[2] ?? 0) * 60 + Number(hit[3] ?? 0)
  }
  if (seconds === null || !Number.isFinite(seconds)) return null
  const ms = Math.round(sign * seconds * 1000)
  return ms === 0 ? 0 : ms // never -0
}

export type OffsetCheck = 'ok' | 'invalid' | 'too_large'

/** Validates the text of the offset field. */
export function checkOffsetInput(input: string): { check: OffsetCheck; ms: number | null } {
  const ms = parseOffset(input)
  if (ms === null) return { check: 'invalid', ms }
  if (Math.abs(ms) > MAX_MANUAL_OFFSET_MS) return { check: 'too_large', ms }
  return { check: 'ok', ms }
}

export type ConfidenceLevel = 'high' | 'medium' | 'low'

export function confidenceLevel(confidence: number): ConfidenceLevel {
  if (confidence >= 0.75) return 'high'
  if (confidence >= ACCEPT_MIN_CONFIDENCE) return 'medium'
  return 'low'
}

/** Confidence as a whole percentage. */
export function confidencePercent(confidence: number): number {
  return Math.round(Math.min(1, Math.max(0, confidence)) * 100)
}

/** An estimate that would change the applied offset of the device (the reference's own 0 never does). */
export function canAccept(d: DeviceOffset): boolean {
  const e = d.estimate
  return !!e && e.source !== 'reference' && e.offset_ms !== d.offset_ms
}

/** Devices "accept all" would take: a changing estimate with enough confidence. */
export function acceptableDevices(devices: DeviceOffset[]): DeviceOffset[] {
  return devices.filter((d) => canAccept(d) && (d.estimate?.confidence ?? 0) >= ACCEPT_MIN_CONFIDENCE)
}

/** Whether any device has a correction applied. */
export function hasCorrection(devices: DeviceOffset[]): boolean {
  return devices.some((d) => d.offset_ms !== 0)
}

/** The panel is only useful for sessions with two or more devices. */
export function needsAlignment(devices: DeviceOffset[]): boolean {
  return devices.length >= 2
}

/** Request body of the three PUT actions. */
export type OffsetsAction =
  | { action: 'accept'; device_ids?: number[] }
  | { action: 'set'; device_id: number; offset_ms: number }
  | { action: 'reset'; device_id?: number }

export function acceptBody(devices: DeviceOffset[], only?: number): OffsetsAction {
  return only === undefined ? { action: 'accept', device_ids: acceptableDevices(devices).map((d) => d.device_id) } : { action: 'accept', device_ids: [only] }
}
