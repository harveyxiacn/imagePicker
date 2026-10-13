// Accessibility smoke test: axe-core in Playwright against the mock backend.
//   terminal 1:  pnpm dev:mock
//   terminal 2:  pnpm a11y            (BASE_URL=http://localhost:5173 by default)
// Visits the main screens (home, grid, loupe, compare, people, settings, export dialog, command palette, help)
// in each appearance mode and exits 1 when axe reports a serious or critical violation (WCAG 2.x A/AA rules).
//   A11Y_MODES=dark,dark-high,light-high   modes to run (default: dark,dark-high)
//   A11Y_FAIL_ON=serious,critical          impacts that fail the run
import { createRequire } from 'node:module'
import { chromium } from 'playwright'

const require = createRequire(import.meta.url)
const AXE_PATH = require.resolve('axe-core/axe.min.js')
const BASE = process.env.BASE_URL ?? 'http://localhost:5173'
const FAIL_ON = new Set((process.env.A11Y_FAIL_ON ?? 'serious,critical').split(','))
const MODES = {
  dark: { theme: 'dark', contrast: 'normal', fontScale: 100 },
  light: { theme: 'light', contrast: 'normal', fontScale: 100 },
  'dark-high': { theme: 'dark', contrast: 'high', fontScale: 100 },
  'light-high': { theme: 'light', contrast: 'high', fontScale: 100 },
  'dark-150': { theme: 'dark', contrast: 'normal', fontScale: 150 },
}
const modes = (process.env.A11Y_MODES ?? 'dark,dark-high').split(',').map((m) => m.trim())
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']

const settle = (page, ms = 500) => page.waitForTimeout(ms)
const browser = await chromium.launch()
const failures = []
const runtimeErrors = []
let checked = 0

async function audit(page, mode, screen) {
  await page.addScriptTag({ path: AXE_PATH })
  const res = await page.evaluate(
    async (tags) => await globalThis.axe.run(document, { runOnly: { type: 'tag', values: tags }, resultTypes: ['violations'] }),
    TAGS,
  )
  checked++
  const bad = res.violations.filter((v) => FAIL_ON.has(v.impact))
  const minor = res.violations.length - bad.length
  console.log(`${bad.length ? 'FAIL' : 'ok  '} [${mode}] ${screen}${minor ? `  (+${minor} moderate/minor)` : ''}`)
  for (const v of bad) {
    console.log(`       ${v.impact} ${v.id}: ${v.help} (${v.nodes.length} node${v.nodes.length > 1 ? 's' : ''})`)
    for (const n of v.nodes.slice(0, 3)) console.log(`         ${n.target.join(' ')}  ${n.failureSummary?.split('\n')[1]?.trim() ?? ''}`)
    failures.push({ mode, screen, id: v.id, impact: v.impact, nodes: v.nodes.length })
  }
}

for (const mode of modes) {
  const ui = MODES[mode]
  if (!ui) throw new Error(`unknown mode ${mode}; use ${Object.keys(MODES).join(', ')}`)
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'en-US' })
  // the same persisted store the app writes (zustand persist, key imagepicker.ui)
  await ctx.addInitScript((state) => {
    localStorage.setItem('imagepicker.ui', JSON.stringify({ state: { lang: 'en', ...state }, version: 0 }))
  }, ui)
  const page = await ctx.newPage()
  page.on('pageerror', (e) => runtimeErrors.push(`[${mode}] pageerror: ${e.message}`))

  // home
  await page.goto(BASE)
  await page.waitForSelector('a[href^="/s/"]')
  await settle(page, 800)
  await audit(page, mode, 'home')

  // grid
  await page.locator('a[href^="/s/"]').first().click()
  await page.waitForSelector('[data-id]')
  await settle(page, 1200)
  await audit(page, mode, 'grid')

  // export dialog (Ctrl+E)
  await page.locator('[data-id]').nth(2).click()
  await page.keyboard.press('Control+e')
  await page.waitForSelector('[role="dialog"]')
  await settle(page, 400)
  await audit(page, mode, 'export dialog')
  await page.keyboard.press('Escape')
  await settle(page, 300)

  // command palette (Ctrl+K) and help overlay
  await page.keyboard.press('Control+k')
  await page.waitForSelector('[data-testid="command-palette"]')
  await page.keyboard.type('undo')
  await settle(page, 300)
  await audit(page, mode, 'command palette')
  await page.keyboard.press('Escape')
  await settle(page, 300)
  await page.keyboard.press('?')
  await page.waitForSelector('[data-testid="help-overlay"]')
  await audit(page, mode, 'help overlay')
  await page.keyboard.press('Escape')
  await settle(page, 300)

  // loupe
  await page.locator('[data-id]').nth(8).dblclick()
  await page.waitForSelector('[data-testid="zoom-pane"]')
  await settle(page, 1200)
  await audit(page, mode, 'loupe')

  // compare
  await page.keyboard.press('c')
  await page.waitForSelector('[data-testid="zoom-pane"]')
  await settle(page, 1000)
  await audit(page, mode, 'compare')

  // people
  const sessionPath = new URL(page.url()).pathname.replace(/\/$/, '')
  await page.goto(`${BASE}${sessionPath}/people`)
  await page.waitForSelector('[data-testid="people-page"]')
  await settle(page, 1000)
  await audit(page, mode, 'people')

  // settings (appearance section is where the new options live)
  await page.goto(`${BASE}/settings`)
  await page.waitForSelector('[data-testid="settings-appearance"], [data-testid="settings-keys"], h1')
  await settle(page, 800)
  await audit(page, mode, 'settings')

  await ctx.close()
}

await browser.close()
console.log(`\n${checked} screens audited in ${modes.length} mode(s) (${modes.join(', ')}).`)
if (runtimeErrors.length) console.log(`page errors:\n  ${runtimeErrors.join('\n  ')}`)
if (failures.length) {
  const rules = [...new Set(failures.map((f) => f.id))]
  console.log(`A11Y SMOKE: FAIL (${failures.length} violation groups: ${rules.join(', ')})`)
  process.exit(1)
}
console.log('A11Y SMOKE: PASS')
