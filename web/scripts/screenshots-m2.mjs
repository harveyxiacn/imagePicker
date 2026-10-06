// M2 visual smoke test against the mock backend:
//   pnpm dev:mock -- --port 5199   then   BASE_URL=http://localhost:5199 node scripts/screenshots-m2.mjs
// Saves PNGs to web/screenshots/m2/ (git-ignored) and fails on console errors.
import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'

const BASE = process.env.BASE_URL ?? 'http://localhost:5199'
const OUT = new URL('../screenshots/m2/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')
mkdirSync(OUT, { recursive: true })

const errors = []
const browser = await chromium.launch()
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'zh-CN' })
const page = await ctx.newPage()
page.on('console', (m) => {
  // The 409 models_missing response is the documented first-run flow, not a bug.
  if (m.type() === 'error' && !/status of 409/.test(m.text())) errors.push(`console: ${m.text()}`)
})
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))

const shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
const settle = (ms = 500) => page.waitForTimeout(ms)
const step = (s) => console.log('·', s)

await page.goto(BASE)
await page.waitForSelector('a[href^="/s/"]')
await page.locator('a[href^="/s/"]', { hasText: '小明生日' }).click()
await page.waitForSelector('[data-id]')
await settle(800)
await shot('00-library-before')

// ---- 1. analysis: consent -> download -> progress
step('consent')
await page.getByTestId('analyze-button').click()
await page.waitForSelector('[data-testid="analyze-menu"]')
await settle(350)
await shot('01-analyze-menu')
await page.getByTestId('analyze-start').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await settle(400)
await shot('02-consent-dialog')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="models-progress"]')
await settle(900)
await shot('03-model-download')
await page.waitForSelector('[data-testid="analysis-progress"]', { timeout: 20000 })
await settle(700)
await shot('04-analysis-progress')
await page.waitForSelector('[data-testid="analysis-progress"]', { state: 'detached', timeout: 60000 })
await settle(1800)
await shot('05-grouped-grid')

// ---- 2. stacks
step('stacks')
const badge = page.getByTestId('stack-badge').first()
await badge.scrollIntoViewIfNeeded()
await badge.click()
await settle(500)
await shot('06-stack-expanded')
await page.keyboard.press('s') // collapse again with S
await settle(300)

// ---- 3. inspector with a face-heavy photo
step('inspector')
const cells = page.locator('[role="gridcell"]')
const n = await cells.count()
for (let i = 0; i < n; i++) {
  await cells.nth(i).click()
  await settle(250)
  if ((await page.getByTestId('face-chip').count()) >= 3) break
}
await settle(600)
await shot('07-inspector-breakdown')

// ---- 4. group view
step('group view')
// pick a burst whose group has several people (landscape bursts have an empty matrix)
const badgeCount = await page.getByTestId('stack-badge').count()
let found = false
for (let i = 0; i < badgeCount && !found; i++) {
  await page.getByTestId('stack-badge').nth(i).locator('xpath=ancestor::div[@data-id]').click()
  await page.keyboard.press('b')
  await settle(1100)
  if ((await page.getByTestId('matrix-row').count()) >= 2) found = true
  else {
    await page.keyboard.press('g')
    await settle(300)
  }
}
if (!found) throw new Error('no burst with a multi-person matrix found')
await settle(500)
await shot('08-group-view')
await page.keyboard.press('Shift+F')
await settle(500)
await shot('09-group-view-faces')
await page.keyboard.press('Shift+F')
await page.keyboard.press('g')
await settle(400)

// ---- 5. person filter
step('person filter')
await page.keyboard.press('Shift+P')
await page.waitForSelector('[data-testid="person-filter-popover"]')
await settle(500)
const tiles = page.locator('[data-testid="person-grid"] button')
await tiles.nth(0).click()
await tiles.nth(1).click()
await tiles.nth(1).click() // include -> exclude
await page.getByRole('button', { name: '睁眼' }).first().click()
await settle(700)
await shot('10-person-filter')
await page.keyboard.press('Escape')
await settle(500)
await shot('11-person-filter-applied')
console.log('filtered:', await page.getByTestId('showing').innerText())

// ---- 6. loupe + face boxes
step('loupe faces')
await page.locator('[role="gridcell"]').first().click()
await page.keyboard.press('e')
await settle(500)
await page.keyboard.press('Shift+F')
await settle(1200)
await shot('12-loupe-face-boxes')
await page.keyboard.press('Shift+F')
await page.keyboard.press('g')

// ---- 7. people page
step('people page')
await page.getByTestId('people-link').click()
await page.waitForSelector('[data-testid="person-card"]')
await settle(900)
await shot('13-people')

await browser.close()
if (errors.length) {
  console.error('ERRORS:\n' + errors.join('\n'))
  process.exit(1)
}
console.log('OK: no console errors')
