import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'

const css = readFileSync(new URL('../index.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n')

/** WCAG relative luminance / contrast ratio of two #rrggbb colours. */
function lum(hex: string): number {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((x) => (x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4))
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
function ratio(a: string, b: string): number {
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

/** Token map of one selector block of index.css, with `var(--x)` references resolved against `base`. */
function tokens(selector: string, base: Record<string, string> = {}): Record<string, string> {
  const start = css.indexOf(`${selector} {`)
  if (start < 0) throw new Error(`selector not found: ${selector}`)
  const body = css.slice(start, css.indexOf('}', start))
  const out = { ...base }
  for (const m of body.matchAll(/--([a-z-]+):\s*([^;]+);/g)) {
    const v = m[2].trim()
    out[m[1]] = v.startsWith('var(') ? (out[v.slice(6, -1)] ?? v) : v
  }
  return out
}

const dark = tokens(':root')
const light = tokens(":root[data-theme='light']", dark)
const darkHigh = tokens(":root[data-contrast='high']", dark)
const lightHigh = tokens(":root[data-theme='light'][data-contrast='high']", darkHigh)

const SURFACES = ['bg', 'panel', 'elevated']
const TEXT = ['fg', 'muted', 'faint', 'accent', 'ai', 'danger', 'success', 'warning']

describe.each([
  ['dark high contrast', darkHigh],
  ['light high contrast', lightHigh],
])('%s palette meets WCAG AA', (_name, t) => {
  it('text colours reach 4.5:1 on every surface', () => {
    for (const s of SURFACES) for (const k of TEXT) expect(ratio(t[k], t[s]), `${k} on ${s}`).toBeGreaterThanOrEqual(4.5)
  })
  it('borders and focus rings reach 3:1 on the surfaces', () => {
    for (const s of SURFACES) {
      expect(ratio(t.line, t[s]), `line on ${s}`).toBeGreaterThanOrEqual(3)
      expect(ratio(t.focus, t[s]), `focus on ${s}`).toBeGreaterThanOrEqual(3)
    }
  })
  it('text on accent buttons reaches 4.5:1', () => {
    expect(ratio(t['accent-fg'], t.accent)).toBeGreaterThanOrEqual(4.5)
  })
})

describe('regular themes keep readable body text', () => {
  it.each([
    ['dark', dark],
    ['light', light],
  ])('%s: fg / muted / faint on bg and panel, accent button text', (_n, t) => {
    for (const s of ['bg', 'panel']) for (const k of ['fg', 'muted', 'faint']) expect(ratio(t[k], t[s]), `${k} on ${s}`).toBeGreaterThanOrEqual(4.5)
    expect(ratio(t['accent-fg'], t.accent)).toBeGreaterThanOrEqual(4.5)
  })
})
