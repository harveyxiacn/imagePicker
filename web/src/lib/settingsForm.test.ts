import { describe, expect, it } from 'vitest'
import {
  cacheFraction,
  DEFAULT_SETTINGS,
  diffSettings,
  getAt,
  isTypedConfirm,
  mergeSettings,
  parseCacheGb,
  parsePort,
  patchAt,
} from './settingsForm'

describe('settings form <-> API mapping', () => {
  it('patchAt builds the minimal nested PATCH body', () => {
    expect(patchAt('language', 'en')).toEqual({ language: 'en' })
    expect(patchAt('analysis.auto_analyze_on_import', true)).toEqual({ analysis: { auto_analyze_on_import: true } })
    expect(patchAt('lan.port', 8080)).toEqual({ lan: { port: 8080 } })
    expect(patchAt('xmp_mode', 'sidecar')).toEqual({ xmp_mode: 'sidecar' })
  })

  it('getAt reads the value a control displays', () => {
    expect(getAt(DEFAULT_SETTINGS, 'analysis.group_strictness')).toBe('normal')
    expect(getAt(DEFAULT_SETTINGS, 'faces.enabled')).toBe(true)
    expect(getAt(DEFAULT_SETTINGS, 'xmp_mode')).toBe('off')
  })

  it('mergeSettings applies a patch optimistically without touching siblings', () => {
    const next = mergeSettings(DEFAULT_SETTINGS, patchAt('lan.enabled', true))
    expect(next.lan).toEqual({ enabled: true, port: 7878, guest_enabled: false })
    expect(next.analysis).toBe(DEFAULT_SETTINGS.analysis)
    expect(DEFAULT_SETTINGS.lan.enabled).toBe(false)
  })

  it('arrays are replaced, not merged', () => {
    const s = { ...DEFAULT_SETTINGS, roots: ['a', 'b'] }
    expect(mergeSettings(s, { roots: ['c'] }).roots).toEqual(['c'])
  })

  it('diffSettings is the inverse of mergeSettings', () => {
    const patch = { privacy: { allow_network: false }, cache: { max_gb: 50 } }
    const next = mergeSettings(DEFAULT_SETTINGS, patch)
    expect(diffSettings(DEFAULT_SETTINGS, next)).toEqual(patch)
    expect(diffSettings(DEFAULT_SETTINGS, DEFAULT_SETTINGS)).toEqual({})
  })

  it('validates port and cache size', () => {
    expect(parsePort('7878')).toBe(7878)
    expect(parsePort('80')).toBeNull()
    expect(parsePort('70000')).toBeNull()
    expect(parsePort('78a')).toBeNull()
    expect(parseCacheGb('20')).toBe(20)
    expect(parseCacheGb('0')).toBeNull()
    expect(parseCacheGb('abc')).toBeNull()
  })

  it('typed confirmation needs the exact word', () => {
    expect(isTypedConfirm(' DELETE ', 'delete')).toBe(true)
    expect(isTypedConfirm('del', 'delete')).toBe(false)
    expect(isTypedConfirm('', '')).toBe(false)
  })

  it('cache usage fraction is clamped', () => {
    expect(cacheFraction(5e9, 20)).toBeCloseTo(0.25)
    expect(cacheFraction(50e9, 20)).toBe(1)
    expect(cacheFraction(1, 0)).toBe(0)
  })
})
