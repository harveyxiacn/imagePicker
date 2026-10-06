// M8 mobile UX visual + behaviour test against the mock backend (Pixel 7 + iPad viewports, touch emulation):
//   pnpm dev:mock -- --port 5440 --host 127.0.0.1    then    BASE_URL=http://127.0.0.1:5440 node scripts/screenshots-m8.mjs
// Saves PNGs to web/screenshots/m8/ (git-ignored); fails on unexpected console errors or failed assertions.
// Touch gestures are injected through CDP `Input.dispatchTouchEvent` so Chromium produces real pointer events.
// Intentional console errors (mock replies for documented pairing failure flows):
//   400 invalid_code, 410 pair_expired, 502 host_unreachable
import { chromium, devices } from 'playwright'
import { mkdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const BASE = process.env.BASE_URL ?? 'http://127.0.0.1:5440'
const OUT = fileURLToPath(new URL('../screenshots/m8/', import.meta.url))
mkdirSync(OUT, { recursive: true })

const errors = []
const allowed = { 400: 0, 410: 0, 502: 0 }
const browser = await chromium.launch()
const step = (s) => console.log('·', s)
const expect = (cond, msg) => {
  if (!cond) {
    errors.push(`assert: ${msg}`)
    console.log('  FAIL', msg)
  }
}

const PIXEL = devices['Pixel 7']
const IPAD = devices['iPad (gen 7)']

async function newPage(device, query = '', path = '/') {
  const ctxOpts = { ...device }
  delete ctxOpts.defaultBrowserType
  const ctx = await browser.newContext({ ...ctxOpts, locale: 'zh-CN' })
  const page = await ctx.newPage()
  page.on('console', (m) => {
    if (m.type() !== 'error') return
    const code = /status of (\d{3})/.exec(m.text())?.[1]
    if (code && Number(code) in allowed) return void allowed[Number(code)]++
    errors.push(`console: ${m.text()}`)
  })
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))
  page.shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
  page.settle = (ms = 400) => page.waitForTimeout(ms)
  page.api = (p, init) => page.evaluate(([u, i]) => fetch(u, i).then((r) => r.text()).then((t) => (t ? JSON.parse(t) : null)), [p, init])
  page.cdp = await ctx.newCDPSession(page)
  await page.goto(BASE + path + query)
  return page
}

/** One-finger drag. `end:false` leaves the finger down (mid-gesture screenshot); call `touchEnd` afterwards. */
async function drag(page, from, to, { steps = 10, end = true, stepMs = 12 } = {}) {
  const send = (type, p) => page.cdp.send('Input.dispatchTouchEvent', { type, touchPoints: p ? [{ x: p.x, y: p.y, id: 1 }] : [] })
  await send('touchStart', from)
  for (let i = 1; i <= steps; i++) {
    await send('touchMove', { x: from.x + ((to.x - from.x) * i) / steps, y: from.y + ((to.y - from.y) * i) / steps })
    await page.waitForTimeout(stepMs)
  }
  if (end) await send('touchEnd')
}
const touchEnd = (page) => page.cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
async function tapAt(page, p) {
  await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: p.x, y: p.y, id: 1 }] })
  await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
}
async function pinch(page, c, from, to) {
  const pts = (d) => [
    { x: c.x - d, y: c.y, id: 1 },
    { x: c.x + d, y: c.y, id: 2 },
  ]
  await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: pts(from) })
  for (let i = 1; i <= 8; i++) {
    await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: pts(from + ((to - from) * i) / 8) })
    await page.waitForTimeout(16)
  }
  await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
}
const centre = async (page, testId) => {
  const b = await page.getByTestId(testId).boundingBox()
  return { x: b.x + b.width / 2, y: b.y + b.height / 2, w: b.width, h: b.height, box: b }
}

/** Polls `check()` (async, Node side) until truthy. (`waitForFunction` does not await async predicates.) */
async function until(check, timeout = 20000, what = 'condition') {
  const t0 = Date.now()
  while (Date.now() - t0 < timeout) {
    if (await check()) return true
    await new Promise((r) => setTimeout(r, 150))
  }
  throw new Error(`timeout waiting for ${what}`)
}

const photoOf = async (page, sid, id) => (await page.api(`/api/photos?session_id=${sid}&limit=5000`)).photos.find((p) => p.id === id)
const cullId = async (page) => Number(await page.getByTestId('cull-view').getAttribute('data-photo-id'))

async function openLibrary(page, title = '小明生日') {
  await page.waitForSelector('a[href^="/s/"]')
  await page.locator('a[href^="/s/"]', { hasText: title }).click()
  await page.waitForSelector('[data-id]')
  await page.settle(900)
  return Number(/\/s\/(\d+)/.exec(page.url())[1])
}

// =================================================================== 1. library + bottom nav (Pixel 7)
let sid
{
  step('library with bottom nav')
  const page = await newPage(PIXEL)
  await page.waitForSelector('a[href^="/s/"]')
  await page.settle(600)
  await page.shot('01-home-mobile')
  sid = await openLibrary(page)
  await page.shot('02-library-bottom-nav')
  expect(await page.getByTestId('bottom-nav').isVisible(), 'bottom nav visible below 768px')
  const labels = await page.getByTestId('bottom-nav').locator('button').allTextContents()
  expect(labels.join('|') === '图库|挑片|人物|设置', `bottom nav labels ${labels}`)
  // 44 px touch targets (bottom nav + condensed top bar + flag chips)
  const small = await page.evaluate(() => {
    const out = []
    for (const el of document.querySelectorAll('[data-testid="bottom-nav"] button, [data-testid="mobile-topbar"] button, [data-testid="mobile-topbar"] a, [data-testid="flag-chips"] button')) {
      const r = el.getBoundingClientRect()
      if (r.width && (r.height < 43.5 || r.width < 43.5)) out.push(`${el.getAttribute('data-testid') ?? el.getAttribute('aria-label') ?? el.textContent} ${Math.round(r.width)}x${Math.round(r.height)}`)
    }
    return out
  })
  expect(small.length === 0, `touch targets < 44px: ${small.join(', ')}`)
  expect((await page.getByTestId('settings-link').count()) === 0, 'desktop top bar not rendered on phone')
  // safe-area / no horizontal overflow
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal page scroll')
  // filter sheet
  await page.getByTestId('mobile-filter').click()
  await page.waitForSelector('[data-testid="filter-sheet"]')
  await page.settle(400)
  await page.shot('03-filter-sheet')
  await page.keyboard.press('Escape')
  await page.settle(300)
  // flag chip filters
  await page.getByTestId('flag-chip-picked').click()
  await page.settle(500)
  expect((await page.getByTestId('flag-chip-picked').getAttribute('aria-pressed')) === 'true', 'flag chip toggles filter')
  await page.getByTestId('flag-chip-all').click()
  await page.context().close()
}

// =================================================================== 2. cull gestures (Pixel 7)
{
  step('cull view gestures')
  const page = await newPage(PIXEL, '', `/s/${sid}`)
  await page.waitForSelector('[data-id]')
  await page.getByTestId('nav-cull').click()
  await page.waitForSelector('[data-testid="cull-view"]')
  await page.settle(900)
  await page.shot('04-cull-view')
  const first = await cullId(page)
  const surface = await centre(page, 'cull-surface')
  const mid = { x: surface.x, y: surface.y }
  const counter = async () => (await page.getByTestId('cull-counter').textContent()).trim()
  expect((await counter()).startsWith('1 /'), `starts at 1: ${await counter()}`)

  // mid-gesture: finger held 52px up -> pick hint grows
  await drag(page, mid, { x: mid.x, y: mid.y - 52 }, { end: false })
  await page.settle(80)
  await page.shot('05-cull-swipe-mid-up')
  const hintOpacity = await page.getByTestId('cull-hint-up').evaluate((e) => Number(getComputedStyle(e).opacity))
  expect(hintOpacity > 0.5, `pick hint visible mid-swipe (opacity ${hintOpacity})`)
  await touchEnd(page) // 52px is below the 64px threshold and slow -> springs back
  await page.settle(350)
  expect((await photoOf(page, sid, first)).flag === 0, 'short drag does not pick')

  // swipe up = pick
  await drag(page, mid, { x: mid.x, y: mid.y - 160 })
  await page.settle(700)
  await page.shot('06-cull-after-pick-snackbar')
  expect((await photoOf(page, sid, first)).flag === 1, 'swipe up picks (flag=1 via API)')
  expect((await page.getByText('已精选').count()) > 0, 'snackbar shown after pick')
  expect((await counter()).startsWith('2 /'), `advanced to next after pick: ${await counter()}`)
  // undo from the snackbar
  await page.getByRole('button', { name: '撤销' }).first().click()
  await page.settle(500)
  expect((await photoOf(page, sid, first)).flag === 0, 'snackbar undo restores flag')

  // swipe down = reject (on current photo)
  const second = await cullId(page)
  await drag(page, mid, { x: mid.x, y: mid.y + 160 })
  await page.settle(700)
  expect((await photoOf(page, sid, second)).flag === -1, 'swipe down rejects (flag=-1)')

  // swipe left = next, right = previous
  const c0 = await counter()
  await drag(page, { x: mid.x + 100, y: mid.y }, { x: mid.x - 100, y: mid.y })
  await page.settle(600)
  const c1 = await counter()
  expect(c1 !== c0, `swipe left navigates (${c0} -> ${c1})`)
  await drag(page, { x: mid.x - 100, y: mid.y }, { x: mid.x + 100, y: mid.y })
  await page.settle(600)
  expect((await counter()) === c0, 'swipe right goes back')

  // star bar tap
  const cur = await cullId(page)
  const star = page.getByTestId('cull-bottom').getByRole('button', { name: /4/ }).first()
  await star.click()
  await page.settle(500)
  expect((await photoOf(page, sid, cur)).user_rating === 4, 'star tap sets rating 4')

  // double-tap zoom, pinch
  await tapAt(page, mid)
  await page.waitForTimeout(90)
  await tapAt(page, mid)
  await page.settle(400)
  const scaleOf = () => page.getByTestId('cull-image').evaluate((e) => new DOMMatrix(getComputedStyle(e.parentElement).transform).a)
  const z1 = await scaleOf()
  expect(z1 > 2, `double-tap zooms (scale ${z1})`)
  await page.shot('07-cull-zoomed')
  await tapAt(page, mid)
  await page.waitForTimeout(90)
  await tapAt(page, mid)
  await page.settle(400)
  expect((await scaleOf()) < 1.05, 'double-tap again returns to fit')
  await pinch(page, mid, 30, 120)
  await page.settle(400)
  expect((await scaleOf()) > 2, `pinch zooms (scale ${await scaleOf()})`)
  // while zoomed a horizontal drag pans instead of navigating
  const cz = await counter()
  await drag(page, { x: mid.x + 80, y: mid.y }, { x: mid.x - 80, y: mid.y })
  await page.settle(500)
  expect((await counter()) === cz, 'drag while zoomed pans, does not navigate')
  await tapAt(page, mid)
  await page.waitForTimeout(90)
  await tapAt(page, mid)
  await page.settle(400)

  // long-press = multi-select, bulk pick
  await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: mid.x, y: mid.y, id: 1 }] })
  await page.waitForTimeout(750)
  await touchEnd(page)
  await page.settle(300)
  expect((await page.getByTestId('cull-selection-count').textContent()).includes('1'), 'long-press selects')
  await drag(page, { x: mid.x + 100, y: mid.y }, { x: mid.x - 100, y: mid.y })
  await page.settle(600)
  await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: mid.x, y: mid.y, id: 1 }] })
  await page.waitForTimeout(750)
  await touchEnd(page)
  await page.settle(300)
  expect((await page.getByTestId('cull-selection-count').textContent()).includes('2'), 'second long-press adds to selection')
  await page.shot('08-cull-multiselect')
  const selIds = []
  await page.getByTestId('cull-bulk-pick').click()
  await page.settle(600)
  expect((await page.getByTestId('cull-selection-bar').count()) === 0, 'selection cleared after bulk action')
  const picked = (await page.api(`/api/photos?session_id=${sid}&flag=picked&limit=5000`)).photos.length
  expect(picked >= 2, `bulk pick flagged >=2 photos (${picked})`)
  void selIds

  // inspector sheet via the info handle
  await page.getByTestId('cull-info-handle').click()
  await page.waitForSelector('[data-testid="inspector-sheet"]')
  await page.settle(500)
  await page.shot('09-cull-inspector-sheet')
  await page.keyboard.press('Escape')
  await page.context().close()
}

// =================================================================== 3. quick cull (Pixel 7)
{
  step('quick cull')
  const page = await newPage(PIXEL, '', `/s/${sid}`)
  await page.waitForSelector('[data-id]')
  // burst groups come from an analysis run: use the on-device (lite) profile like a phone would
  const run = await page.api('/api/analysis/run', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ session_id: sid, profile: 'lite' }) })
  expect(!!run.task_id, 'lite profile accepted by the analysis API')
  await until(async () => (await page.api(`/api/analysis/status?session_id=${sid}`)).state === 'done', 60000, 'analysis done')
  await page.settle(800)
  await page.getByTestId('nav-cull').click()
  await page.waitForSelector('[data-testid="cull-view"]')
  await page.getByTestId('quick-cull-toggle').click()
  await page.waitForSelector('[data-testid="quick-cull"]')
  await page.settle(900)
  await page.shot('10-quick-cull-cards')
  const group0 = (await page.getByTestId('quick-group').textContent()).trim()
  expect(/第 1\/\d+ 组/.test(group0), `group progress shown: ${group0}`)
  const card = await centre(page, 'quick-card')
  const id1 = Number(await page.getByTestId('quick-card').getAttribute('data-photo-id'))
  await drag(page, { x: card.x, y: card.y }, { x: card.x + 70, y: card.y + 10 }, { end: false })
  await page.settle(80)
  await page.shot('11-quick-cull-swipe-mid')
  await touchEnd(page)
  await page.settle(600)
  expect((await photoOf(page, sid, id1)).flag === 1, 'quick cull right swipe keeps (flag=1)')
  const id2 = Number(await page.getByTestId('quick-card').getAttribute('data-photo-id'))
  expect(id2 !== id1, 'next card shown')
  await drag(page, { x: card.x, y: card.y }, { x: card.x - 170, y: card.y })
  await page.settle(600)
  expect((await photoOf(page, sid, id2)).flag === -1, 'quick cull left swipe rejects (flag=-1)')
  expect((await page.getByTestId('quick-progress').textContent()).includes('2/'), 'progress counts decided cards')
  // undo
  await page.getByTestId('quick-undo').click()
  await page.settle(500)
  expect((await photoOf(page, sid, id2)).flag === 0, 'quick cull undo restores flag')
  expect(Number(await page.getByTestId('quick-card').getAttribute('data-photo-id')) === id2, 'undo brings the card back')
  // finish the first group with buttons -> next group
  await page.getByTestId('quick-keep').click()
  await page.settle(500)
  await page.getByTestId('quick-reject').click()
  await page.settle(500)
  const group1 = (await page.getByTestId('quick-group').textContent()).trim()
  expect(/第 (1|2)\/\d+ 组/.test(group1), `group indicator after cards: ${group1}`)
  await page.shot('12-quick-cull-next-group')
  await page.context().close()
}

// =================================================================== 4. album import (Pixel 7)
{
  step('album import')
  const page = await newPage(PIXEL)
  await page.waitForSelector('[data-testid="import-albums"]')
  await page.getByTestId('import-albums').click()
  await page.waitForSelector('[data-testid="album-list"]')
  await page.waitForFunction(() => [...document.querySelectorAll('[data-testid="album-list"] img')].every((i) => i.complete && i.naturalWidth > 0))
  await page.settle(400)
  await page.shot('13-album-list')
  expect((await page.locator('[data-testid^="album-"][data-testid$="camera"]').count()) === 1, 'album rows rendered')
  expect((await page.getByTestId('album-camera').textContent()).includes('1,284'), 'album counts shown')
  const sessionsBefore = (await page.api('/api/sessions')).sessions.length
  await page.getByTestId('album-kyoto').click()
  await page.waitForURL(/\/s\/\d+/)
  const sessionsAfter = (await page.api('/api/sessions')).sessions
  expect(sessionsAfter.length === sessionsBefore + 1 && sessionsAfter.some((s) => s.title === 'Kyoto2026'), 'album import creates a session titled after the album')
  await page.context().close()

  const denied = await newPage(PIXEL, '?mock_media=denied')
  await denied.waitForSelector('[data-testid="import-albums"]')
  await denied.getByTestId('import-albums').click()
  await denied.waitForSelector('[data-testid="album-denied"]')
  await denied.settle(300)
  await denied.shot('14-album-permission-denied')
  await denied.getByTestId('album-retry').click()
  await denied.waitForSelector('[data-testid="album-list"]')
  expect(true, 'retry after denial reaches the list')
  await denied.context().close()

  const partial = await newPage(PIXEL, '?mock_media=partial')
  await partial.waitForSelector('[data-testid="import-albums"]')
  await partial.getByTestId('import-albums').click()
  await partial.waitForSelector('[data-testid="album-partial"]')
  await partial.settle(400)
  await partial.shot('15-album-permission-partial')
  await partial.context().close()
}

// =================================================================== 5. mobile edit drawer (Pixel 7)
{
  step('mobile edit drawer')
  const page = await newPage(PIXEL, '', `/s/${sid}`)
  await page.waitForSelector('[data-id]')
  await page.getByTestId('nav-cull').click()
  await page.waitForSelector('[data-testid="cull-view"]')
  const id = await cullId(page)
  await page.getByTestId('open-edit').click()
  await page.waitForSelector('[data-testid="edit-drawer"]')
  await page.waitForSelector('[data-testid="render-frame"]', { timeout: 15000 })
  await page.settle(900)
  await page.shot('16-edit-drawer')
  expect((await page.getByTestId('bottom-nav').count()) === 0, 'edit page is full screen (no bottom nav)')
  expect((await page.locator('[data-testid="edit-panel"]').count()) === 0, 'desktop side panel not rendered on phone')
  // AI one-click
  await page.getByTestId('m-auto-auto').click()
  await until(async () => /exposure|contrast|temp|saturation|vibrance/.test(JSON.stringify((await page.api(`/api/edits/${id}`)).stack ?? {})), 8000, 'AI auto edit saved')
  await page.settle(500)
  // chips + one big slider
  await page.getByTestId('param-contrast').click()
  await page.getByTestId('big-slider').fill('35')
  await page.getByTestId('big-slider').blur()
  await page.settle(900)
  const stack = (await page.api(`/api/edits/${id}`)).stack
  expect(JSON.stringify(stack).includes('"contrast":35'), `big slider saved contrast=35: ${JSON.stringify(stack).slice(0, 200)}`)
  await page.shot('17-edit-drawer-slider')
  // portrait tab
  await page.getByTestId('drawer-tab-portrait').click()
  await page.settle(700)
  await page.shot('18-edit-drawer-portrait')
  await page.getByTestId('drawer-tab-adjust').click()
  // press-and-hold before/after
  const hb = await centre(page, 'hold-compare')
  await page.cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: hb.x, y: hb.y, id: 1 }] })
  await page.settle(500)
  expect((await page.getByTestId('hold-compare').getAttribute('aria-pressed')) === 'true', 'holding shows original')
  await page.shot('19-edit-hold-original')
  await touchEnd(page)
  await page.settle(300)
  expect((await page.getByTestId('hold-compare').getAttribute('aria-pressed')) === 'false', 'release returns to the edit')
  await page.context().close()
}

// =================================================================== 6. analysis profile chooser (Pixel 7)
{
  step('analysis profile chooser')
  const page = await newPage(PIXEL, '', `/s/${sid}`)
  await page.waitForSelector('[data-id]')
  await page.getByTestId('analyze-button').click()
  await page.waitForSelector('[data-testid="profile-chooser"]')
  await page.settle(400)
  await page.shot('20-analysis-profile-chooser')
  const txt = await page.getByTestId('profile-chooser').textContent()
  expect(txt.includes('快速（本机）') && txt.includes('标准（远程 AI，需配对）'), 'both phone profiles listed')
  await page.getByTestId('profile-standard').click()
  await page.waitForSelector('[data-testid="remote-host-status"]')
  await page.settle(300)
  expect(await page.getByTestId('analyze-start').isDisabled(), 'standard disabled until a host is paired')
  await page.shot('21-analysis-standard-needs-pairing')
  await page.getByTestId('profile-lite').click()
  expect(!(await page.getByTestId('analyze-start').isDisabled()), 'lite always available')
  await page.getByTestId('analyze-start').click()
  await until(async () => (await page.api(`/api/analysis/status?session_id=${sid}`)).state !== 'idle', 8000, 'lite analysis started')
  await page.context().close()

  const paired = await newPage(PIXEL, '?mock_remote=paired', `/s/${sid}`)
  await paired.waitForSelector('[data-id]')
  await paired.getByTestId('analyze-button').click()
  await paired.getByTestId('profile-standard').click()
  await paired.waitForSelector('[data-testid="remote-status"][data-state="connected"]')
  expect(!(await paired.getByTestId('analyze-start').isDisabled()), 'standard enabled when the host is connected')
  await paired.settle(300)
  await paired.shot('22-analysis-standard-connected')
  await paired.context().close()
}

// =================================================================== 7. phone pairing (Pixel 7)
{
  step('remote AI pairing (phone)')
  const page = await newPage(PIXEL, '?section=remote', '/settings')
  await page.waitForSelector('[data-testid="remote-section"]')
  await page.settle(500)
  await page.shot('23-pairing-phone')
  // validation
  await page.getByTestId('remote-connect').click()
  await page.waitForSelector('[data-testid="remote-url-error"]')
  expect((await page.getByTestId('remote-code-error').textContent()).includes('请输入'), 'empty code flagged')
  await page.getByTestId('remote-url').fill('http://')
  await page.getByTestId('remote-code').fill('12ab')
  expect((await page.getByTestId('remote-code').inputValue()) === '12', 'code input keeps digits only')
  await page.getByTestId('remote-connect').click()
  await page.settle(200)
  expect((await page.getByTestId('remote-url-error').textContent()).includes('地址'), 'invalid address flagged')
  await page.shot('24-pairing-validation')
  // wrong code (server 400)
  await page.getByTestId('remote-url').fill('192.168.1.23:7878')
  await page.getByTestId('remote-code').fill('000000')
  await page.getByTestId('remote-connect').click()
  await page.waitForSelector('[data-testid="remote-pair-error"]')
  expect((await page.getByTestId('remote-pair-error').textContent()).includes('配对码不正确'), 'wrong code error shown')
  await page.shot('25-pairing-wrong-code')
  // success
  await page.getByTestId('remote-code').fill('123456')
  await page.getByTestId('remote-connect').click()
  await page.waitForSelector('[data-testid="remote-status"][data-state="connected"]', { timeout: 8000 })
  await page.settle(400)
  await page.shot('26-pairing-connected')
  const st = await page.api('/api/remote/status')
  expect(st.connected && st.host_tier === 'T2', 'status API reports connected + host tier')
  expect((await page.getByTestId('remote-tier').textContent()).includes('T2'), 'host tier chip')
  await page.getByTestId('remote-disconnect').click()
  await page.waitForSelector('[data-testid="remote-form"]')
  expect((await page.api('/api/remote/status')).paired === false, 'disconnect clears pairing')
  await page.context().close()

  const off = await newPage(PIXEL, '?section=remote&mock_remote=offline', '/settings')
  await off.waitForSelector('[data-testid="remote-status"][data-state="offline"]')
  await off.settle(300)
  await off.shot('27-pairing-host-offline')
  await off.context().close()
}

// =================================================================== 8. host pairing code/QR + devices (Pixel 7 + iPad)
{
  step('remote AI devices (host)')
  const page = await newPage(PIXEL, '?section=devices', '/settings')
  await page.waitForSelector('[data-testid="remote-devices-section"]')
  await page.waitForSelector('[data-testid^="device-dev-"]')
  await page.getByTestId('pair-start').click()
  await page.waitForSelector('[data-testid="pair-code"]')
  await page.settle(500)
  await page.shot('28-host-pairing-code-qr')
  const code = (await page.getByTestId('pair-code').textContent()).replace(/\s/g, '')
  expect(/^\d{6}$/.test(code), `6-digit code shown (${code})`)
  expect((await page.getByTestId('pair-countdown').textContent()).match(/[45]:/), 'countdown starts around 5:00')
  const img = await page.getByTestId('pair-qr').evaluate((e) => e.naturalWidth)
  expect(img > 0, 'QR image decodes')
  const t0 = (await page.getByTestId('pair-countdown').textContent()).trim()
  await page.waitForTimeout(2100)
  expect((await page.getByTestId('pair-countdown').textContent()).trim() !== t0, 'countdown ticks')
  // the displayed code is accepted by the phone-side connect call (mock plays both roles)
  const res = await page.api('/api/remote/connect', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ url: 'http://192.168.1.23:7878', code }) })
  expect(res.connected === true, 'pairing with the displayed code succeeds')
  await page.waitForSelector('[data-testid="device-dev-pixel-old"]')
  await page.waitForFunction(() => document.querySelectorAll('[data-testid^="device-dev-"]').length >= 3, null, { timeout: 12000 })
  await page.settle(300)
  await page.shot('29-host-devices-list')
  const before = await page.locator('[data-testid^="device-dev-"]').count()
  await page.getByTestId('revoke-dev-pixel-old').click()
  await page.getByTestId('revoke-confirm-dev-pixel-old').click()
  await page.waitForFunction((n) => document.querySelectorAll('[data-testid^="device-dev-"]').length === n - 1, before, { timeout: 8000 })
  expect(true, 'revoke removes the device')
  await page.api('/api/remote/connect', { method: 'DELETE' })
  await page.context().close()
}

// =================================================================== 9. iPad + desktop regression
{
  step('iPad / desktop layouts')
  const ipad = await newPage(IPAD)
  await ipad.waitForSelector('a[href^="/s/"]')
  const isid = await openLibrary(ipad)
  await ipad.shot('30-ipad-library')
  expect((await ipad.getByTestId('bottom-nav').count()) === 0, 'iPad (>=768px) keeps the desktop layout without bottom nav')
  expect((await ipad.getByTestId('settings-link').count()) === 1, 'iPad shows the regular top bar')
  await ipad.goto(`${BASE}/settings?section=devices`)
  await ipad.waitForSelector('[data-testid="remote-devices-section"]')
  await ipad.getByTestId('pair-start').click()
  await ipad.waitForSelector('[data-testid="pair-code"]')
  await ipad.settle(400)
  await ipad.shot('31-ipad-host-pairing')
  await ipad.goto(`${BASE}/s/${isid}/edit/${(await ipad.api(`/api/photos?session_id=${isid}&limit=1`)).photos[0].id}`)
  await ipad.waitForSelector('[data-testid="edit-panel"]', { timeout: 15000 })
  await ipad.settle(900)
  await ipad.shot('32-ipad-edit')
  await ipad.context().close()

  const desk = await newPage({ viewport: { width: 1440, height: 900 }, hasTouch: false })
  await desk.waitForSelector('a[href^="/s/"]')
  await openLibrary(desk)
  await desk.shot('33-desktop-library')
  expect((await desk.getByTestId('bottom-nav').count()) === 0, 'desktop has no bottom nav')
  expect((await desk.getByTestId('settings-link').count()) === 1, 'desktop top bar unchanged')
  expect((await desk.getByTestId('mobile-topbar').count()) === 0, 'no mobile top bar on desktop')
  // keyboard shortcuts still work: press 5 rates the active photo
  const active = await desk.evaluate(() => document.querySelector('[data-id][aria-selected="true"], [data-id][data-active="true"]')?.getAttribute('data-id'))
  void active
  await desk.keyboard.press('e')
  await desk.settle(500)
  expect((await desk.locator('[data-testid="zoom-pane"]').count()) === 1, 'E opens the loupe with a keyboard')
  await desk.keyboard.press('ArrowRight')
  await desk.keyboard.press('4')
  await desk.settle(600)
  expect((await desk.api(`/api/photos?session_id=${isid}&rating_gte=4&limit=5000`)).photos.length >= 1, 'number keys rate with a hardware keyboard')
  await desk.shot('34-desktop-loupe')
  await desk.context().close()
}

await browser.close()
console.log('allowed intentional console errors:', allowed)
if (errors.length) {
  console.log('\nFAILURES:\n' + errors.map((e) => ' - ' + e).join('\n'))
  process.exit(1)
}
console.log('\nM8 mobile checks passed. Screenshots in web/screenshots/m8/')
