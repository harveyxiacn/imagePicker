import { describe, expect, it } from 'vitest'
import {
  agoParts,
  formatCode,
  formatCountdown,
  isValidCode,
  normalizeCode,
  normalizeHostUrl,
  PAIR_INITIAL,
  pairReducer,
  parsePairUri,
  secondsLeft,
  validatePairForm,
} from './pairing'

describe('code + url input', () => {
  it('normalises codes to at most 6 digits', () => {
    expect(normalizeCode('123 456')).toBe('123456')
    expect(normalizeCode('12-34-56-78')).toBe('123456')
    expect(normalizeCode('abc')).toBe('')
    expect(isValidCode('123456')).toBe(true)
    expect(isValidCode('12345')).toBe(false)
    expect(isValidCode('12345a')).toBe(false)
  })

  it('normalises host addresses', () => {
    expect(normalizeHostUrl('192.168.1.23:7878')).toBe('http://192.168.1.23:7878')
    expect(normalizeHostUrl(' http://studio.local:7878/ ')).toBe('http://studio.local:7878')
    expect(normalizeHostUrl('https://example.com/some/path')).toBe('https://example.com')
    expect(normalizeHostUrl('')).toBeNull()
    expect(normalizeHostUrl('ftp://host')).toBeNull()
    expect(normalizeHostUrl('http://')).toBeNull()
    expect(normalizeHostUrl('not a url')).toBeNull()
  })

  it('validates the manual form', () => {
    expect(validatePairForm('', '')).toMatchObject({ ok: false, errors: { url: 'required', code: 'required' } })
    expect(validatePairForm('http://', '12345')).toMatchObject({ ok: false, errors: { url: 'invalid', code: 'invalid' } })
    const ok = validatePairForm('192.168.1.5:7878', '123456')
    expect(ok.ok).toBe(true)
    expect(ok.value).toEqual({ url: 'http://192.168.1.5:7878', code: '123456' })
  })

  it('parses the QR payload', () => {
    expect(parsePairUri('imagepicker://pair?url=http%3A%2F%2F192.168.1.23%3A7878&code=654321')).toEqual({ url: 'http://192.168.1.23:7878', code: '654321' })
    expect(parsePairUri('imagepicker://pair?url=192.168.1.23:7878&code=65 43 21')).toEqual({ url: 'http://192.168.1.23:7878', code: '654321' })
    expect(parsePairUri('imagepicker://pair?url=192.168.1.23&code=123')).toBeNull()
    expect(parsePairUri('https://evil.example/pair?code=123456')).toBeNull()
  })
})

describe('phone pairing state machine', () => {
  const input = { url: 'http://h:1', code: '123456' }

  it('idle -> submitting -> connected', () => {
    let s = pairReducer(PAIR_INITIAL, { type: 'submit', input })
    expect(s).toEqual({ phase: 'submitting', input })
    s = pairReducer(s, { type: 'success' })
    expect(s.phase).toBe('connected')
    s = pairReducer(s, { type: 'disconnect' })
    expect(s).toEqual(PAIR_INITIAL)
  })

  it('failure returns to idle with the error, editing clears it', () => {
    let s = pairReducer(PAIR_INITIAL, { type: 'submit', input })
    s = pairReducer(s, { type: 'failure', error: 'wrong code' })
    expect(s).toEqual({ phase: 'idle', error: 'wrong code' })
    expect(pairReducer(s, { type: 'edit' })).toEqual(PAIR_INITIAL)
  })

  it('ignores impossible transitions (double submit, stray success)', () => {
    const sub = pairReducer(PAIR_INITIAL, { type: 'submit', input })
    expect(pairReducer(sub, { type: 'submit', input: { ...input, code: '000000' } })).toBe(sub)
    expect(pairReducer(PAIR_INITIAL, { type: 'success' })).toBe(PAIR_INITIAL)
    expect(pairReducer(PAIR_INITIAL, { type: 'failure', error: 'x' })).toBe(PAIR_INITIAL)
  })
})

describe('host countdown', () => {
  it('counts down from unix seconds, ms or ISO', () => {
    const now = 1_700_000_000_000
    expect(secondsLeft(1_700_000_300, now)).toBe(300)
    expect(secondsLeft(now + 1500, now)).toBe(2)
    expect(secondsLeft(new Date(now + 90_000).toISOString(), now)).toBe(90)
    expect(secondsLeft(1_699_999_000, now)).toBe(0)
    expect(secondsLeft('garbage', now)).toBe(0)
  })

  it('formats', () => {
    expect(formatCountdown(300)).toBe('5:00')
    expect(formatCountdown(65)).toBe('1:05')
    expect(formatCountdown(-3)).toBe('0:00')
    expect(formatCode('123456')).toBe('123 456')
    expect(formatCode('12')).toBe('12')
  })

  it('picks a relative-time unit', () => {
    expect(agoParts(30)).toEqual({ unit: 'second', value: 30 })
    expect(agoParts(3599)).toEqual({ unit: 'minute', value: 59 })
    expect(agoParts(7300)).toEqual({ unit: 'hour', value: 2 })
    expect(agoParts(3 * 86400 + 5)).toEqual({ unit: 'day', value: 3 })
  })
})
