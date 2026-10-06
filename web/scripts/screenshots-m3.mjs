// M3 edit-page visual smoke test against the mock backend:
//   pnpm dev:mock -- --port 5311   then   BASE_URL=http://localhost:5311 node scripts/screenshots-m3.mjs
// Saves PNGs to web/screenshots/m3/ (git-ignored) and fails on unexpected console errors.
import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'

const BASE = process.env.BASE_URL ?? 'http://localhost:5311'
const OUT = new URL('../screenshots/m3/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')
mkdirSync(OUT, { recursive: true })

const errors = []
const browser = await chromium.launch()
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'zh-CN' })
const page = await ctx.newPage()
page.on('console', (m) => {
  // The 409 models_missing response of the first AI mask is the documented consent flow, not a bug.
  if (m.type() === 'error' && !/status of 409/.test(m.text())) errors.push(`console: ${m.text()}`)
})
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))

const shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
const settle = (ms = 500) => page.waitForTimeout(ms)
const step = (s) => console.log('·', s)
const expect = (cond, msg) => {
  if (!cond) errors.push(`assert: ${msg}`)
}
const frameSrc = () => page.getByTestId('render-frame').getAttribute('src')
const waitFrameChange = async (before) => {
  for (let i = 0; i < 40; i++) {
    const s = await frameSrc().catch(() => null)
    if (s && s !== before) return s
    await settle(100)
  }
  errors.push('assert: preview frame did not change')
  return before
}
const setNum = async (testId, value) => {
  const input = page.getByTestId(testId).locator('input.lr-num')
  await input.click()
  await input.fill(String(value))
  await input.press('Enter')
}
const openSection = async (id) => {
  const sec = page.getByTestId(`section-${id}`)
  if ((await sec.locator('button[aria-expanded]').first().getAttribute('aria-expanded')) === 'false') await sec.locator('button[aria-expanded]').first().click()
  await settle(200)
}

await page.goto(BASE)
await page.waitForSelector('a[href^="/s/"]')
await page.locator('a[href^="/s/"]', { hasText: '小明生日' }).click()
await page.waitForSelector('[data-id]')
await settle(800)

// ---- 1. open the edit page with D
step('edit page')
// multi-select 3 photos (cursor ends on the 4th cell) so paste / sync have several targets
await page.locator('[role="gridcell"]').nth(2).click()
await page.locator('[role="gridcell"]').nth(5).click({ modifiers: ['Control'] })
await page.locator('[role="gridcell"]').nth(3).click({ modifiers: ['Control'] })
await page.keyboard.press('d')
await page.waitForSelector('[data-testid="edit-page"]')
await page.waitForSelector('[data-testid="render-frame"]')
await settle(1200)
expect(/\/edit\/\d+/.test(page.url()), 'D opens /edit/:id')
await shot('01-edit-default')
const f0 = await frameSrc()

// ---- 2. AI auto
step('ai auto')
await page.getByTestId('auto-auto').click()
await page.waitForSelector('[data-testid="auto-applied"]')
await waitFrameChange(f0)
await settle(900)
await shot('02-after-ai-auto')

// ---- 3. basic slider via keyboard (progressive preview) + double-click reset
step('slider')
const exp = page.getByTestId('adj-exposure').locator('input[type=range]')
await exp.focus()
for (let i = 0; i < 6; i++) await page.keyboard.press('ArrowRight')
await settle(800)
const v1 = await exp.inputValue()
expect(Number(v1) > 0.5, `exposure raised (${v1})`)
// wheel over the slider fine-tunes by one step
await exp.hover()
const w0 = Number(await exp.inputValue())
await page.mouse.wheel(0, -100)
await settle(200)
expect(Number(await exp.inputValue()) > w0, 'wheel nudges the slider')
await exp.dblclick()
await settle(600)
expect(Number(await exp.inputValue()) === 0, 'double-click resets exposure')
// progressive preview: drag => small frames (long_edge 800), release => one screen-resolution frame
const edges = []
const onReq = (r) => {
  if (r.url().endsWith('/api/render/preview') && r.method() === 'POST') edges.push(JSON.parse(r.postData() ?? '{}').long_edge)
}
page.on('request', onReq)
const track = await exp.boundingBox()
await page.mouse.move(track.x + track.width * 0.5, track.y + track.height / 2)
await page.mouse.down()
for (let i = 1; i <= 24; i++) {
  await page.mouse.move(track.x + track.width * (0.5 + i * 0.012), track.y + track.height / 2)
  await page.waitForTimeout(12)
}
const midDrag = edges.length
await page.mouse.up()
await settle(1000)
page.off('request', onReq)
console.log('  preview long_edge sequence:', edges.join(','))
expect(edges.slice(0, midDrag).every((e) => e === 800), 'drag frames are long_edge 800')
expect(edges.length > midDrag && edges.at(-1) > 800, 'release requests a screen-resolution frame')
expect(edges.length < 30, `requests are throttled / de-duplicated (${edges.length})`)
await setNum('adj-exposure', 0.4)
await settle(600)

// ---- 4. curves
step('curves')
await openSection('curves')
const curve = page.getByTestId('curve-editor')
await curve.scrollIntoViewIfNeeded()
const cb = await curve.boundingBox()
await page.mouse.click(cb.x + cb.width * 0.3, cb.y + cb.height * 0.62)
await settle(300)
await page.mouse.move(cb.x + cb.width * 0.72, cb.y + cb.height * 0.5)
await page.mouse.down()
await page.mouse.move(cb.x + cb.width * 0.72, cb.y + cb.height * 0.3, { steps: 6 })
await page.mouse.up()
await settle(900)
expect((await page.getByTestId('curve-point').count()) >= 4, 'curve points added')
await shot('03-curves')

// ---- 5. HSL
step('hsl')
await openSection('hsl')
await page.getByTestId('hsl-tab-s').click()
await setNum('hsl-orange-s', 45)
await setNum('hsl-blue-s', -40)
await settle(900)
await page.getByTestId('section-hsl').scrollIntoViewIfNeeded()
await shot('04-hsl')

// ---- 6. colour grading
step('grading')
await openSection('grading')
const wheel = page.getByTestId('wheel-shadows').locator('[role=slider]')
const wb = await wheel.boundingBox()
await page.mouse.click(wb.x + wb.width * 0.25, wb.y + wb.height * 0.3)
await settle(700)
await page.getByTestId('section-grading').scrollIntoViewIfNeeded()
await shot('05-grading')

// ---- 7. local: AI sky mask (409 -> consent -> download -> mask overlay)
step('local sky')
await openSection('local')
await page.getByTestId('target-sky').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await settle(500)
await shot('06-mask-consent')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="local-item"]', { timeout: 30000 })
await page.waitForSelector('[data-testid="mask-overlay"]')
await settle(1200)
await setNum('local0-exposure', -0.8)
await settle(900)
await page.getByTestId('section-local').scrollIntoViewIfNeeded()
await shot('07-local-sky-mask-overlay')
await page.keyboard.press('o')
await settle(500)
expect((await page.getByTestId('mask-overlay').count()) === 0, 'O hides the mask overlay')
await shot('08-local-sky-result')
// radial gradient with on-canvas handles
await page.getByTestId('add-radial').click()
await settle(900)
expect((await page.getByTestId('radial-center').count()) === 1, 'radial handles shown')
const rc = await page.getByTestId('radial-center').boundingBox()
await page.mouse.move(rc.x + rc.width / 2, rc.y + rc.height / 2)
await page.mouse.down()
await page.mouse.move(rc.x - 120, rc.y + 40, { steps: 8 })
await page.mouse.up()
await setNum('local1-exposure', 0.7)
await settle(900)
await shot('09-local-radial-handles')

// ---- 8. crop mode
step('crop')
await openSection('crop')
await page.keyboard.press('r')
await settle(900)
await page.getByTestId('aspect-4:5').click()
await settle(700)
await setNum('crop-angle', 3.5)
await settle(1100)
expect((await page.getByTestId('crop-overlay').count()) === 1, 'crop overlay visible')
await shot('10-crop-mode')
await page.getByTestId('crop-done').click()
await settle(1100)
await shot('11-cropped')

// ---- 9. before / after
step('before/after')
await page.getByTestId('compare-split').click()
await settle(1100)
await shot('12-before-after-split')
const sh = await page.getByTestId('split-handle').boundingBox()
await page.mouse.move(sh.x + sh.width / 2, sh.y + sh.height / 2)
await page.mouse.down()
await page.mouse.move(sh.x - 260, sh.y + sh.height / 2, { steps: 6 })
await page.mouse.up()
await settle(300)
const sh2 = await page.getByTestId('split-handle').boundingBox()
expect(sh2.x < sh.x - 200, 'split divider is draggable')
await shot('12b-split-dragged')
await page.getByTestId('compare-split').click()
await page.keyboard.press('\\')
await settle(900)
expect((await page.getByTestId('original-chip').count()) === 1, 'backslash shows the original')
await shot('13-original-toggle')
await page.keyboard.press('\\')
await settle(500)

// ---- 10. presets + LUT
step('presets')
await openSection('presets')
await page.getByTestId('section-presets').scrollIntoViewIfNeeded()
await settle(1800)
await shot('14-presets')
await page.getByTestId('preset-film_warm').click()
await settle(1100)
await page.getByTestId('lut-path').fill('C:\\Luts\\Kodak2383.cube')
await page.getByTestId('lut-import').click()
await settle(800)
await shot('15-preset-applied-lut')

// ---- 11. history, undo / redo
step('history')
await page.getByTestId('history-toggle').click()
await settle(500)
await shot('16-history-panel')
const items = page.getByTestId('history-item')
const n = await items.count()
expect(n >= 5, `history entries (${n})`)
await items.nth(Math.min(3, n - 1)).click()
await settle(1500)
await page.getByTestId('history-toggle').click()
await page.keyboard.press('Control+Shift+z')
await settle(800)
await page.keyboard.press('Control+z')
await settle(800)

// ---- 12. copy / paste / sync + filmstrip badges
step('copy / paste / sync')
await page.keyboard.press('Control+Shift+c')
await page.waitForSelector('[data-testid="copy-dialog"]')
await settle(300)
await shot('17-copy-dialog')
await page.getByTestId('copy-confirm').click()
await settle(300)
await page.getByTestId('filmstrip').locator('button').nth(4).click()
await settle(1200)
await page.keyboard.press('Control+Shift+v')
await settle(1500)
expect((await page.getByTestId('edit-badge').count()) >= 3, `paste marks the 3 selected photos (${await page.getByTestId('edit-badge').count()})`)
// edit the current photo, then sync it to the selection ("same group" fallback)
await page.getByTestId('preset-vivid').click()
await settle(900)
await page.getByTestId('sync-group').click()
await settle(1200)
expect((await page.getByTestId('edit-badge').count()) >= 4, `sync adds the current photo's edits (${await page.getByTestId('edit-badge').count()})`)
await shot('18-filmstrip-badges')

// ---- 13. back to the grid: edited thumbs + badges, filter/cursor kept
step('back to grid')
await page.getByTestId('edit-back').click()
await page.waitForSelector('[data-id]')
await settle(1200)
expect((await page.getByTestId('edit-badge').count()) >= 1, 'grid shows edit badges')
await shot('19-grid-edited-thumbs')

await browser.close()
if (errors.length) {
  console.error('ERRORS:\n' + errors.join('\n'))
  process.exit(1)
}
console.log('OK: no console errors')
