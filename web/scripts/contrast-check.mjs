// WCAG contrast check for the design tokens in src/index.css (npm run contrast).
// Reads the token blocks straight from the stylesheet, resolves the cascade for each
// theme / contrast combination and fails (exit 1) when a text or boundary pair is too weak.
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const cssPath = process.argv[2] ?? fileURLToPath(new URL('../src/index.css', import.meta.url))
const css = readFileSync(cssPath, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')

/** Token blocks keyed by the data-attributes their selector requires, in file order. */
function parseBlocks(src) {
  const blocks = []
  for (const m of src.matchAll(/(:root(?:\[[^\]]+\])*)\s*\{([^}]*)\}/g)) {
    const need = {}
    for (const a of m[1].matchAll(/\[data-(\w+)=['"]?(\w+)['"]?\]/g)) need[a[1]] = a[2]
    const vars = {}
    for (const d of m[2].matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) vars[d[1]] = d[2].trim()
    blocks.push({ need, vars })
  }
  return blocks
}

function resolveTheme(blocks, theme, contrast) {
  const attrs = { theme, contrast }
  const out = {}
  for (const b of blocks) {
    if (Object.entries(b.need).every(([k, v]) => attrs[k] === v)) Object.assign(out, b.vars)
  }
  return out
}

const lin = (c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)
function lum(hex) {
  const n = parseInt(hex.slice(1), 16)
  return 0.2126 * lin((n >> 16) / 255) + 0.7152 * lin(((n >> 8) & 255) / 255) + 0.0722 * lin((n & 255) / 255)
}
function ratio(a, b) {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p)
  return (x + 0.05) / (y + 0.05)
}

const TEXT = ['fg', 'muted', 'faint', 'accent-text', 'ai', 'success', 'warning', 'danger']
const SURFACES = ['bg', 'panel', 'elevated']
const THEMES = [
  { name: 'dark', theme: 'dark', contrast: 'normal', text: 4.5 },
  { name: 'light', theme: 'light', contrast: 'normal', text: 4.5 },
  { name: 'dark+high', theme: 'dark', contrast: 'high', text: 7 },
  { name: 'light+high', theme: 'light', contrast: 'high', text: 7 },
]

function check(src) {
  const blocks = parseBlocks(src)
  const rows = []
  for (const t of THEMES) {
    const v = resolveTheme(blocks, t.theme, t.contrast)
    const hex = (k) => {
      let val = v[`--${k}`]
      for (let i = 0; i < 4 && val?.startsWith('var('); i++) val = v[val.slice(4, -1).trim()]
      if (!val || !/^#[0-9a-f]{6}$/i.test(val)) throw new Error(`${t.name}: --${k} is missing or not a #rrggbb value (${val})`)
      return val
    }
    const add = (kind, fg, bg, min) => {
      const r = ratio(hex(fg), hex(bg))
      rows.push({ theme: t.name, kind, fg, bg, ratio: r, min, ok: r >= min })
    }
    for (const s of SURFACES) for (const k of TEXT) add('text', k, s, t.text)
    for (const k of ['accent-fg']) add('text', k, 'accent', t.text)
    add('text', 'ai-fg', 'ai', t.text)
    // Accent text also sits on tinted chips (bg-accent/10..20 over the panel).
    {
      const mix = (a, b, p) => '#' + [16, 8, 0].map((sh) => Math.round(((parseInt(a.slice(1), 16) >> sh) & 255) * p + ((parseInt(b.slice(1), 16) >> sh) & 255) * (1 - p)).toString(16).padStart(2, '0')).join('')
      const r = ratio(hex('accent-text'), mix(hex('accent'), hex('panel'), 0.2))
      rows.push({ theme: t.name, kind: 'text', fg: 'accent-text', bg: 'accent/20', ratio: r, min: t.text, ok: r >= t.text })
    }
    // Non-text (WCAG 1.4.11): control borders and the focus ring need 3:1.
    for (const s of ['bg', 'panel']) add('border', 'line-strong', s, 3)
    add('border', 'line-strong', 'elevated', 3)
    add('focus', 'focus', 'bg', 3)
    add('focus', 'focus', 'panel', 3)
  }
  return rows
}

const rows = check(css)
const pad = (s, n) => String(s).padEnd(n)
console.log(`${pad('theme', 11)}${pad('kind', 7)}${pad('foreground', 13)}${pad('on', 10)}${pad('ratio', 8)}${pad('min', 5)}result`)
for (const r of rows) {
  console.log(`${pad(r.theme, 11)}${pad(r.kind, 7)}${pad(r.fg, 13)}${pad(r.bg, 10)}${pad(r.ratio.toFixed(2), 8)}${pad(r.min, 5)}${r.ok ? 'ok' : 'FAIL'}`)
}
const bad = rows.filter((r) => !r.ok)
console.log(bad.length ? `\n${bad.length} of ${rows.length} pairs below threshold` : `\nall ${rows.length} pairs pass`)
process.exit(bad.length ? 1 : 0)

