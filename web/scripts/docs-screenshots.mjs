// Documentation screenshots, mock backend only (no real photos):
//   pnpm dev:mock --port 5481 --host 127.0.0.1
//   BASE_URL=http://127.0.0.1:5481 OUT_DIR=<dir> node scripts/docs-screenshots.mjs
// Writes raw PNGs to OUT_DIR (convert / optimise separately, see docs/images).
import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'

const BASE = process.env.BASE_URL ?? 'http://127.0.0.1:5481'
const OUT = (process.env.OUT_DIR ?? 'screenshots/docs') + '/'
mkdirSync(OUT, { recursive: true })

const browser = await chromium.launch()
const errors = []
async function newPage(lang = 'zh-CN', viewport = { width: 1440, height: 900 }) {
  const ctx = await browser.newContext({ viewport, locale: lang })
  const page = await ctx.newPage()
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))
  page.shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
  page.settle = (ms = 500) => page.waitForTimeout(ms)
  page.api = (path, init) => page.evaluate(([p, i]) => fetch(p, i).then((r) => r.json()), [path, init])
  page.nav = (path) =>
    page.evaluate((p) => {
      history.pushState({}, '', p)
      dispatchEvent(new PopStateEvent('popstate'))
    }, path)
  page.openSection = async (id) => {
    const sec = page.getByTestId(`section-${id}`)
    const btn = sec.locator('button[aria-expanded]').first()
    if ((await btn.getAttribute('aria-expanded')) === 'false') await btn.click()
    await sec.scrollIntoViewIfNeeded()
    await page.settle(300)
  }
  await page.goto(BASE)
  return page
}
const step = (s) => console.log('·', s)

const page = await newPage()
await page.waitForSelector('a[href^="/s/"]')
await page.settle(800)
step('home')
await page.shot('home')

await page.locator('a[href^="/s/"]', { hasText: '小明生日' }).click()
await page.waitForSelector('[data-id]')
await page.settle(900)
const sid = Number(/\/s\/(\d+)/.exec(page.url())[1])

step('analyze')
await page.getByTestId('analyze-button').click()
await page.waitForSelector('[data-testid="analyze-menu"]')
await page.settle(400)
await page.shot('analyze-menu')
await page.getByTestId('analyze-start').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await page.settle(400)
await page.shot('model-consent')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="analysis-progress"]', { timeout: 20000 })
await page.waitForSelector('[data-testid="analysis-progress"]', { state: 'detached', timeout: 60000 })
await page.settle(2000)
step('grid')
await page.shot('grid')

// a burst with a multi-person matrix
const badgeCount = await page.getByTestId('stack-badge').count()
let found = false
for (let i = 0; i < badgeCount && !found; i++) {
  await page.getByTestId('stack-badge').nth(i).locator('xpath=ancestor::div[@data-id]').click()
  await page.keyboard.press('b')
  await page.settle(1100)
  if ((await page.getByTestId('matrix-row').count()) >= 2) found = true
  else {
    await page.keyboard.press('g')
    await page.settle(300)
  }
}
if (!found) throw new Error('no burst with a multi-person matrix')
await page.settle(500)
step('group')
await page.shot('group-view')

// best take editor
await page.getByTestId('matrix-compose').click()
await page.waitForSelector('[data-testid="besttake-page"]')
await page.waitForSelector('[data-testid="bt-face"]')
await page.waitForSelector('[data-testid="render-frame"]')
await page.settle(1000)
await page.getByTestId('bt-face').nth(0).click()
await page.waitForSelector('[data-testid="bt-panel"]')
await page.settle(800)
step('besttake')
await page.shot('besttake')

// back to library: expanded stack
await page.nav(`/s/${sid}`)
await page.waitForSelector('[data-id]')
await page.settle(900)
await page.getByTestId('stack-badge').first().scrollIntoViewIfNeeded()
await page.getByTestId('stack-badge').first().click()
await page.settle(600)
step('stack')
await page.shot('stack-expanded')
await page.keyboard.press('s')

// inspector with a face-heavy photo
const cells = page.locator('[role="gridcell"]')
const n = await cells.count()
for (let i = 0; i < n; i++) {
  await cells.nth(i).click()
  await page.settle(250)
  if ((await page.getByTestId('face-chip').count()) >= 3) break
}
await page.settle(500)
step('inspector')
await page.shot('inspector')

// person filter popover
await page.getByTestId('person-filter-button').click()
await page.waitForSelector('[data-testid="person-filter-popover"]')
await page.settle(500)
step('person filter')
await page.shot('person-filter')
await page.keyboard.press('Escape')
await page.settle(300)

// assistant
await page.keyboard.press('Control+j')
await page.waitForSelector('[data-testid="assistant-drawer"]')
await page.getByTestId('assistant-input').fill('每个场景保留前2张，淘汰其余')
await page.getByTestId('assistant-send').click()
await page.waitForSelector('[data-testid="plan-card"][data-phase="planned"]')
await page.settle(500)
step('assistant')
await page.shot('assistant')
await page.getByTestId('assistant-close').click()

// export dialog
await page.keyboard.press('Control+e')
await page.waitForSelector('[data-testid="export-presets"]')
await page.settle(500)
step('export')
await page.shot('export')
await page.keyboard.press('Escape')
await page.settle(300)

// edit page on a photo with several people
const list = await page.api(`/api/photos?session_id=${sid}&limit=500`)
let target = null
for (const p of list.photos) {
  const r = await page.api(`/api/photos/${p.id}/people`)
  const named = r.people.filter((x) => x.person_id !== null)
  if (named.length >= 2 && named.some((x) => x.has_pose)) {
    target = p
    break
  }
}
if (!target) target = list.photos[0]
await page.nav(`/s/${sid}/edit/${target.id}`)
await page.waitForSelector('[data-testid="render-frame"]')
await page.settle(1000)
step('edit basic')
await page.shot('edit-basic')

await page.openSection('local')
await page.getByTestId('target-sky').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="local-item"]', { timeout: 30000 })
await page.waitForSelector('[data-testid="mask-overlay"]')
await page.settle(1500)
const inp = page.getByTestId('local0-exposure').locator('input.lr-num')
await inp.fill('-0.8')
await inp.press('Enter')
await page.settle(900)
await page.getByTestId('section-local').scrollIntoViewIfNeeded()
step('edit masks')
await page.shot('edit-masks')
await page.keyboard.press('o')

await page.openSection('portrait')
await page.waitForSelector('[data-testid="portrait-prepare"]')
await page.getByTestId('beauty-prepare').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="portrait-ready"]', { timeout: 30000 })
await page.settle(800)
await page.openSection('portrait')
const chips = page.getByTestId('chip-person')
if ((await chips.count()) > 0) await chips.nth(0).click()
for (const [id, v] of [
  ['beauty-smooth', '70'],
  ['beauty-whiten', '40'],
]) {
  const el = page.getByTestId(id).locator('input.lr-num')
  if (await el.count()) {
    await el.fill(v)
    await el.press('Enter')
    await page.settle(600)
  }
}
await page.settle(1000)
step('portrait')
await page.shot('portrait')

let byst = null
for (const p of list.photos) {
  const r = await page.api(`/api/photos/${p.id}/bystanders`)
  if (r.faces.length >= 1) {
    byst = p
    break
  }
}
if (byst) {
  await page.nav(`/s/${sid}/edit/${byst.id}`)
  await page.waitForSelector('[data-testid="render-frame"]')
  await page.settle(1000)
}
await page.openSection('repair')
await page.settle(800)
step('repair')
await page.shot('repair')

// people page
await page.nav(`/s/${sid}/people`)
await page.waitForSelector('[data-testid="person-card"]')
await page.settle(1000)
step('people')
await page.shot('people')

// settings
await page.nav('/settings')
await page.waitForSelector('[data-testid="settings-page"]')
await page.getByTestId('settings-nav-hardware').click()
await page.settle(700)
step('settings')
await page.shot('settings-hardware')
await page.getByTestId('settings-nav-lan').click()
await page.settle(500)
await page.shot('settings-lan')

// light theme grid
await page.nav('/settings')
await page.getByTestId('settings-nav-appearance').click()
await page.getByTestId('setting-theme').selectOption('light')
await page.settle(400)
await page.nav(`/s/${sid}`)
await page.waitForSelector('[data-id]')
await page.settle(1500)
step('light')
await page.shot('grid-light')

console.log(errors.length ? errors : 'no page errors')
await browser.close()
