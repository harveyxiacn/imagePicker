import { describe, expect, it } from 'vitest'
import { DIAGNOSTICS_EXCLUDED, DIAGNOSTICS_INCLUDED, diagnosticsFilename } from './diagnostics'
import { en } from '@/i18n/en'
import { zhCN } from '@/i18n/zh-CN'

describe('diagnosticsFilename', () => {
  it('reads the attachment name', () => {
    expect(diagnosticsFilename('attachment; filename="imagepicker-diagnostics-20261013-120000.zip"')).toBe('imagepicker-diagnostics-20261013-120000.zip')
  })
  it('falls back for anything else', () => {
    expect(diagnosticsFilename(null)).toBe('imagepicker-diagnostics.zip')
    expect(diagnosticsFilename('attachment')).toBe('imagepicker-diagnostics.zip')
    expect(diagnosticsFilename('attachment; filename="../evil.exe"')).toBe('imagepicker-diagnostics.zip')
    expect(diagnosticsFilename('attachment; filename="a/b.zip"')).toBe('imagepicker-diagnostics.zip')
  })
})

describe('diagnostics texts', () => {
  it.each([
    ['en', en],
    ['zh-CN', zhCN],
  ])('%s describes every included and excluded item', (_n, dict) => {
    const d = dict.settings.diagnostics as { include: Record<string, string>; exclude: Record<string, string> }
    for (const k of DIAGNOSTICS_INCLUDED) expect(d.include[k], k).toBeTruthy()
    for (const k of DIAGNOSTICS_EXCLUDED) expect(d.exclude[k], k).toBeTruthy()
  })
})
