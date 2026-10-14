import { describe, expect, it } from 'vitest'
import type { DeviceOffset } from '@/api/types'
import {
  ACCEPT_MIN_CONFIDENCE,
  MAX_MANUAL_OFFSET_MS,
  acceptBody,
  acceptableDevices,
  canAccept,
  checkOffsetInput,
  confidenceLevel,
  confidencePercent,
  formatOffset,
  hasCorrection,
  needsAlignment,
  parseOffset,
} from './timeAlign'

function dev(id: number, over: Partial<DeviceOffset> = {}): DeviceOffset {
  return { device_id: id, name: `D${id}`, kind: 'camera', photo_count: 10, offset_ms: 0, source: 'none', estimate: null, ...over }
}

const est = (offset_ms: number, confidence = 0.9, source: 'visual' | 'gps' | 'reference' = 'visual') => ({ offset_ms, confidence, evidence: 12, source })

describe('formatOffset', () => {
  it('formats hours, minutes and seconds with a sign', () => {
    expect(formatOffset(0)).toBe('0s')
    expect(formatOffset(42_000)).toBe('+42s')
    expect(formatOffset(-90_000)).toBe('-1m 30s')
    expect(formatOffset(3_725_000)).toBe('+1h 02m 05s')
    expect(formatOffset(-7_200_000)).toBe('-2h 00m 00s')
  })

  it('rounds to the second', () => {
    expect(formatOffset(499)).toBe('0s')
    expect(formatOffset(1_500)).toBe('+2s')
    expect(formatOffset(-499)).toBe('0s')
  })
})

describe('parseOffset', () => {
  it('reads units', () => {
    expect(parseOffset('1h 2m 5s')).toBe(3_725_000)
    expect(parseOffset('-90s')).toBe(-90_000)
    expect(parseOffset('+2m')).toBe(120_000)
    expect(parseOffset('3h')).toBe(10_800_000)
    expect(parseOffset('1H30M')).toBe(5_400_000)
    expect(parseOffset('-1h 02m 05s')).toBe(-3_725_000)
  })

  it('reads clock notation', () => {
    expect(parseOffset('1:02:05')).toBe(3_725_000)
    expect(parseOffset('-0:30:00')).toBe(-1_800_000)
    expect(parseOffset('2:15')).toBe(8_100_000) // hours:minutes
    expect(parseOffset('+0:00:07.5')).toBe(7_500)
  })

  it('reads plain seconds', () => {
    expect(parseOffset('-3725')).toBe(-3_725_000)
    expect(parseOffset('42.5')).toBe(42_500)
    expect(parseOffset('  7 ')).toBe(7_000)
  })

  it('accepts the typographic minus and never returns -0', () => {
    expect(parseOffset('−90s')).toBe(-90_000)
    expect(Object.is(parseOffset('-0'), 0)).toBe(true)
  })

  it('rejects text it does not understand', () => {
    for (const bad of ['', '   ', 'abc', '1h x', '1:75', '1:02:61', '--5', '5 parsecs', '1.2.3', 'h', '+', ':30']) {
      expect(parseOffset(bad), bad).toBeNull()
    }
  })

  it('round-trips what formatOffset prints', () => {
    for (const ms of [0, 1_000, -1_000, 59_000, 60_000, 3_599_000, 3_600_000, -3_725_000, 86_399_000, -604_800_000]) {
      expect(parseOffset(formatOffset(ms)), formatOffset(ms)).toBe(ms)
    }
  })
})

describe('checkOffsetInput', () => {
  it('flags invalid and out of range input', () => {
    expect(checkOffsetInput('1h')).toEqual({ check: 'ok', ms: 3_600_000 })
    expect(checkOffsetInput('nope').check).toBe('invalid')
    expect(checkOffsetInput('7d').check).toBe('invalid')
    expect(checkOffsetInput('169h').check).toBe('too_large')
    expect(checkOffsetInput('-169h').check).toBe('too_large')
    expect(checkOffsetInput('168h')).toEqual({ check: 'ok', ms: MAX_MANUAL_OFFSET_MS })
  })
})

describe('confidence', () => {
  it('maps to levels and percentages', () => {
    expect(confidenceLevel(0.9)).toBe('high')
    expect(confidenceLevel(0.75)).toBe('high')
    expect(confidenceLevel(ACCEPT_MIN_CONFIDENCE)).toBe('medium')
    expect(confidenceLevel(0.49)).toBe('low')
    expect(confidencePercent(0.874)).toBe(87)
    expect(confidencePercent(1.4)).toBe(100)
    expect(confidencePercent(-1)).toBe(0)
  })
})

describe('accepting estimates', () => {
  it('only an estimate that changes the offset can be accepted', () => {
    expect(canAccept(dev(1))).toBe(false)
    expect(canAccept(dev(1, { estimate: est(-3_600_000) }))).toBe(true)
    expect(canAccept(dev(1, { estimate: est(-3_600_000), offset_ms: -3_600_000, source: 'auto' }))).toBe(false)
    expect(canAccept(dev(1, { estimate: est(0, 1, 'reference') }))).toBe(false)
    expect(canAccept(dev(1, { estimate: est(0), offset_ms: 5_000, source: 'manual' }))).toBe(true)
  })

  it('"accept all" skips weak estimates and the reference', () => {
    const devices = [
      dev(1, { estimate: est(0, 1, 'reference') }),
      dev(2, { estimate: est(-3_600_000, 0.9) }),
      dev(3, { estimate: est(120_000, 0.3) }),
      dev(4),
      dev(5, { estimate: est(-5_000, 0.5, 'gps') }),
    ]
    expect(acceptableDevices(devices).map((d) => d.device_id)).toEqual([2, 5])
    expect(acceptBody(devices)).toEqual({ action: 'accept', device_ids: [2, 5] })
    // one device by choice, even with a weak estimate
    expect(acceptBody(devices, 3)).toEqual({ action: 'accept', device_ids: [3] })
  })

  it('knows when a correction is applied and when alignment matters', () => {
    expect(hasCorrection([dev(1), dev(2)])).toBe(false)
    expect(hasCorrection([dev(1), dev(2, { offset_ms: -1_000, source: 'manual' })])).toBe(true)
    expect(needsAlignment([dev(1)])).toBe(false)
    expect(needsAlignment([dev(1), dev(2)])).toBe(true)
  })
})
