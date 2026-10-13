import { describe, expect, it } from 'vitest'
import { applyAppearance, clampFontScale, resolveTheme, type RootLike } from './appearance'

function fakeRoot() {
  const vars: Record<string, string> = {}
  const root: RootLike = { dataset: {}, style: { setProperty: (k, v) => void (vars[k] = v) } }
  return { root, vars }
}

describe('clampFontScale', () => {
  it('keeps values inside 90-150 and snaps to 5', () => {
    expect(clampFontScale(100)).toBe(100)
    expect(clampFontScale(50)).toBe(90)
    expect(clampFontScale(400)).toBe(150)
    expect(clampFontScale(112)).toBe(110)
    expect(clampFontScale(113)).toBe(115)
  })
  it('falls back to 100 for junk', () => {
    expect(clampFontScale(NaN)).toBe(100)
    expect(clampFontScale(undefined)).toBe(100)
    expect(clampFontScale('abc')).toBe(100)
    expect(clampFontScale('125')).toBe(125)
  })
})

describe('resolveTheme', () => {
  it('follows the OS only for system', () => {
    expect(resolveTheme('system', true)).toBe('light')
    expect(resolveTheme('system', false)).toBe('dark')
    expect(resolveTheme('dark', true)).toBe('dark')
    expect(resolveTheme('light', false)).toBe('light')
  })
})

describe('applyAppearance', () => {
  it('writes theme, contrast and the font-scale variable', () => {
    const { root, vars } = fakeRoot()
    applyAppearance(root, { theme: 'system', contrast: 'high', fontScale: 125 }, true)
    expect(root.dataset).toEqual({ theme: 'light', contrast: 'high' })
    expect(vars['--font-scale']).toBe('1.25')
  })
  it('clamps an out-of-range scale and defaults contrast to normal', () => {
    const { root, vars } = fakeRoot()
    applyAppearance(root, { theme: 'dark', contrast: 'normal', fontScale: 999 }, false)
    expect(root.dataset.contrast).toBe('normal')
    expect(vars['--font-scale']).toBe('1.5')
    applyAppearance(root, { theme: 'dark', contrast: 'normal', fontScale: 10 }, false)
    expect(vars['--font-scale']).toBe('0.9')
  })
})
