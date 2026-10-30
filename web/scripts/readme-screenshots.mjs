// Capture the real WebUI against a disposable, analysed Qwen 1000-image catalog.
// BASE_URL=http://127.0.0.1:17830 node scripts/readme-screenshots.mjs
// See docs/images/qwen/README.md for catalog preparation and provenance.
import { chromium } from 'playwright'
import { mkdirSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const base = process.env.BASE_URL ?? 'http://127.0.0.1:17830'
const out = resolve(process.env.OUT_DIR ?? '../docs/images/qwen')
mkdirSync(out, { recursive: true })
const api = async (path) => {
  const r = await fetch(`${base}/api${path}`)
  if (!r.ok) throw new Error(`${path}: HTTP ${r.status}`)
  return r.json()
}
const sessions = (await api('/sessions')).sessions
const session = sessions.find(s => s.photo_count === 1000)
if (!session) throw new Error('An analysed 1000-image session is required')
const sid = session.id
const photos = (await api(`/photos?session_id=${sid}&limit=1000`)).photos
const portrait = photos.find(p => p.file_name === 'park-golden-a1.jpg')
if (!portrait) throw new Error('Expected Qwen portrait is missing')
const family = photos.find(p => p.file_name === 'bg01-family-dinner-00.jpg')
if (!family?.burst_id) throw new Error('Expected analysed Qwen family burst is missing')
const browser = await chromium.launch()
const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, locale: 'zh-CN', colorScheme: 'dark' })
const page = await context.newPage()
const errors = []
const shots = []
page.on('pageerror', e => errors.push(e.message))
page.on('response', r => { if (r.status() >= 500) errors.push(`${r.status()} ${r.url()}`) })
const go = async path => { await page.goto(base + path); await page.waitForLoadState('networkidle') }
const showGrid = async () => {
  await go(`/s/${sid}`)
  await page.getByTestId('grid').waitFor()
  if (await page.getByTestId('grouped-toggle').getAttribute('aria-pressed') === 'true') await page.getByTestId('grouped-toggle').click()
  await page.getByTestId('grid').evaluate(el => { el.scrollTop = 540 })
  await page.waitForTimeout(500)
  const id = await page.locator('[role="gridcell"][data-id]').evaluateAll(cells => cells.find(el => {
    const r = el.getBoundingClientRect()
    return r.top > 250 && r.bottom < 700
  })?.getAttribute('data-id'))
  if (id) await page.locator(`[role="gridcell"][data-id="${id}"]`).click()
}
const shot = async name => {
  await page.evaluate(() => document.fonts.ready)
  await page.waitForFunction(() => [...document.images].filter(i => {
    const r = i.getBoundingClientRect()
    return r.width && r.height && r.top < innerHeight && r.bottom > 0
  }).every(i => i.complete && i.naturalWidth > 0))
  await page.waitForTimeout(700)
  await page.mouse.move(1590, 990)
  await page.screenshot({ path: `${out}/${name}.png` })
  shots.push(name)
  console.log(`Captured ${name}`)
}
try {
  await showGrid()
  await shot('grid')
  await go(`/s/${sid}/besttake/${family.burst_id}`)
  await page.getByTestId('besttake-page').waitFor()
  await page.getByTestId('bt-face').first().click()
  await page.getByTestId('bt-panel').waitFor()
  await shot('besttake')
  await page.getByTestId('bt-back').click()
  await page.getByTestId('grid').waitFor()
  if (await page.getByTestId('grouped-toggle').getAttribute('aria-pressed') === 'true') await page.getByTestId('grouped-toggle').click()
  await page.waitForTimeout(1000)
  await page.keyboard.press('b')
  await page.getByTestId('group-view').waitFor()
  await page.getByTestId('matrix-row').first().waitFor()
  await shot('group-view')
  await go('/settings')
  await page.getByTestId('settings-nav-appearance').click()
  await page.getByTestId('setting-theme').selectOption('light')
  await showGrid()
  await shot('grid-light')
  await go('/settings')
  await page.getByTestId('settings-nav-appearance').click()
  await page.getByTestId('setting-theme').selectOption('dark')
  await go(`/s/${sid}/people`)
  await page.getByTestId('person-card').first().waitFor()
  await shot('people')
  await go(`/s/${sid}/edit/${portrait.id}`)
  await page.getByTestId('edit-canvas').locator('img').first().waitFor()
  await shot('edit-basic')
  await go(`/s/${sid}`)
  await page.locator('[data-id]').first().click()
  await page.keyboard.press('Control+e')
  await page.getByTestId('export-presets').waitFor()
  await shot('export')
  await page.keyboard.press('Escape')
  await page.getByTestId('search-input').fill('一只猫')
  await page.keyboard.press('Enter')
  await page.waitForTimeout(10000)
  await shot('search')
} finally {
  if (shots.length < 8) {
    await page.screenshot({ path: `${out}/debug.png` })
    console.log((await page.locator('body').innerText()).slice(0, 2000))
  }
  writeFileSync(`${out}/capture.json`, JSON.stringify({ base, sessionId: sid, photoCount: session.photo_count, viewport: { width: 1600, height: 1000 }, shots, errors }, null, 2) + '\n')
  await browser.close()
}
if (errors.length) throw new Error(errors.join('\n'))
