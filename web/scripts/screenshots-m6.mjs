// M6 visual + behaviour smoke test against the mock backend:
//   pnpm dev:mock -- --port 5436 --host 127.0.0.1   then   BASE_URL=http://127.0.0.1:5436 node scripts/screenshots-m6.mjs
// Saves PNGs to web/screenshots/m6/ (git-ignored) and fails on unexpected console errors.
// Intentional errors (documented flows, all produced by the mock on purpose):
//   401  anonymous requests in the LAN-login scenario (before the redirect to /login)
//   400  enabling LAN access before an owner password exists (password_required)
//   429  the 6th wrong password within a minute (login rate limit, too_many_requests + retry_after countdown)
import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'

const BASE = process.env.BASE_URL ?? 'http://127.0.0.1:5436'
const OUT = new URL('../screenshots/m6/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')
mkdirSync(OUT, { recursive: true })

const errors = []
const allowed = { 401: 0, 400: 0, 429: 0 }
const browser = await chromium.launch()

const step = (s) => console.log('·', s)
const expect = (cond, msg) => {
  if (!cond) {
    errors.push(`assert: ${msg}`)
    console.log('  FAIL', msg)
  }
}

async function newPage(query = '', viewport = { width: 1440, height: 900 }) {
  const ctx = await browser.newContext({ viewport, locale: 'zh-CN' })
  const page = await ctx.newPage()
  page.on('console', (m) => {
    if (m.type() !== 'error') return
    const code = /status of (\d{3})/.exec(m.text())?.[1]
    if (code && code in allowed) return void allowed[code]++
    errors.push(`console: ${m.text()}`)
  })
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))
  const reqs = []
  page.on('request', (r) => {
    if (r.method() !== 'GET') reqs.push({ method: r.method(), url: new URL(r.url()).pathname + new URL(r.url()).search, body: r.postData() })
  })
  page.reqs = reqs
  page.shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
  page.settle = (ms = 400) => page.waitForTimeout(ms)
  page.api = (path, init) => page.evaluate(([p, i]) => fetch(p, i).then((r) => r.json()), [path, init])
  page.nav = (path) =>
    page.evaluate((p) => {
      history.pushState({}, '', p)
      dispatchEvent(new PopStateEvent('popstate'))
    }, path)
  await page.goto(BASE + query)
  return page
}

/** Clicks "execute" on the newest plan card (confirming if asked) and waits until it is done. */
const runLastPlan = async (page, { confirm = false } = {}) => {
  const done = () => page.locator('[data-testid="plan-card"][data-phase="done"]').count()
  const n = await done()
  await page.waitForSelector('[data-testid="plan-card"][data-phase="planned"]')
  await page.locator('[data-testid="plan-execute"]').last().click()
  if (confirm) await page.getByTestId('plan-confirm').click()
  await page.waitForFunction((k) => document.querySelectorAll('[data-testid="plan-card"][data-phase="done"]').length > k, n, { timeout: 20000 })
  await page.settle(500)
}

const openLibrary = async (page, title = '小明生日') => {
  await page.waitForSelector('a[href^="/s/"]')
  await page.locator('a[href^="/s/"]', { hasText: title }).click()
  await page.waitForSelector('[data-id]')
  await page.settle(900)
  return Number(/\/s\/(\d+)/.exec(page.url())[1])
}
const sessionCounts = async (page, sid) => (await page.api(`/api/sessions/${sid}`)).session

// =================================================================== A. settings (every section)
{
  step('settings sections')
  const page = await newPage()
  await page.waitForSelector('[data-testid="home-settings-link"]')
  await page.getByTestId('home-settings-link').click()
  await page.waitForSelector('[data-testid="settings-page"]')
  const section = async (id, shot) => {
    await page.getByTestId(`settings-nav-${id}`).click()
    await page.settle(500)
    if (shot) await page.shot(`settings-${id}`)
  }

  // hardware & models: worker tier, models table, download with progress, delete
  await section('hardware')
  expect((await page.getByTestId('settings-tier').textContent())?.trim() === 'T3', 'tier shown')
  expect((await page.getByTestId('model-row').count()) >= 10, 'models table rows')
  const row = page.locator('[data-testid="model-row"][data-installed="false"]', { hasText: 'sky-seg' })
  await row.getByTestId('model-download').click()
  await page.waitForSelector('[data-testid="model-progress"]', { timeout: 5000 })
  await page.shot('settings-hardware-downloading')
  await page.waitForSelector('[data-model="sky-seg"][data-installed="true"]', { timeout: 15000 })
  await page.getByTestId('setting-models-source').selectOption('hf-mirror')
  await page.getByTestId('setting-assistant-engine').selectOption('llm')
  await page.settle(500)
  await page.shot('settings-hardware')
  const s1 = await page.api('/api/settings')
  expect(s1.models.source === 'hf-mirror' && s1.assistant.engine === 'llm', 'model source / engine persisted via PATCH')
  const del = page.locator('[data-model="sky-seg"]').getByTestId('model-delete')
  await del.click()
  await page.getByTestId('model-delete-confirm').click()
  await page.waitForSelector('[data-model="sky-seg"][data-installed="false"]', { timeout: 8000 })

  // analysis
  await section('analysis')
  await page.getByTestId('setting-analysis-profile').selectOption('fast')
  await page.getByTestId('setting-analysis-auto').click()
  await page.getByTestId('setting-analysis-strictness').selectOption('strict')
  await page.settle(400)
  await page.shot('settings-analysis')
  const s2 = await page.api('/api/settings')
  expect(s2.analysis.default_profile === 'fast' && s2.analysis.auto_analyze_on_import === true && s2.analysis.group_strictness === 'strict', 'analysis settings PATCHed')

  // faces & privacy: typed confirmation
  await section('faces')
  await page.getByTestId('faces-wipe').click()
  await page.waitForSelector('[data-testid="faces-wipe-input"]')
  expect(await page.getByTestId('faces-wipe-confirm').isDisabled(), 'wipe disabled until typed')
  await page.shot('settings-faces-confirm')
  await page.getByTestId('faces-wipe-input').fill('清除')
  expect(!(await page.getByTestId('faces-wipe-confirm').isDisabled()), 'wipe enabled after typing the word')
  await page.getByTestId('faces-wipe-confirm').click()
  await page.waitForSelector('[data-testid="faces-wipe-input"]', { state: 'detached' })
  await page.getByTestId('setting-privacy-network').click()
  await page.settle(400)
  await page.shot('settings-faces')
  expect((await page.api('/api/settings')).privacy.allow_network === false, 'allow_network off PATCHed')
  await page.getByTestId('setting-privacy-network').click()

  // cache
  await section('cache')
  await page.getByTestId('cache-clear-previews').click()
  await page.waitForFunction(() => document.querySelector('[data-testid="cache-clear-previews"]')?.disabled === true && !document.querySelector('[data-testid="cache-clear-previews"] .animate-spin'))
  await page.settle(300)
  const c = await page.api('/api/cache')
  expect(c.items.previews === 0 && c.items.thumbs > 0, `previews cleared only ${JSON.stringify(c)}`)
  await page.getByTestId('setting-cache-max').fill('40')
  await page.getByTestId('setting-cache-max').press('Enter')
  await page.settle(400)
  await page.shot('settings-cache')
  expect((await page.api('/api/settings')).cache.max_gb === 40, 'cache max PATCHed')

  // render
  await section('render')
  await page.getByTestId('setting-render-backend').selectOption('cpu')
  await page.settle(300)
  await page.shot('settings-render')

  // LAN: password required first (intentional 400), then QR
  await section('lan')
  await page.getByTestId('setting-lan-enabled').click()
  await page.waitForFunction(() => document.querySelector('[data-testid="setting-lan-enabled"]')?.getAttribute('aria-checked') === 'false')
  await page.waitForSelector('text=password')
  await page.getByTestId('lan-password-input-owner').fill('hunter2hunter2')
  await page.getByTestId('lan-password-save-owner').click()
  await page.settle(500)
  await page.getByTestId('setting-lan-enabled').click()
  await page.waitForSelector('[data-testid="lan-restart"]', { timeout: 3000 })
  await page.waitForSelector('[data-testid="lan-qr"] img', { timeout: 8000 })
  await page.getByTestId('setting-lan-guest').click()
  await page.settle(600)
  await page.getByTestId('lan-access').scrollIntoViewIfNeeded()
  await page.shot('settings-lan-qr')
  const qrOk = await page.locator('[data-testid="lan-qr"] img').evaluate((img) => img.complete && img.naturalWidth > 0)
  expect(qrOk, 'QR image renders')
  expect((await page.getByTestId('lan-urls').textContent()).includes('http://192.168.'), 'LAN urls listed')

  // XMP
  await section('xmp')
  await page.getByTestId('xmp-mode-sidecar').click()
  await page.waitForFunction(() => document.querySelector('[data-testid="xmp-mode-sidecar"]')?.checked)
  await page.settle(300)
  await page.getByTestId('xmp-sync-read').click()
  await page.waitForSelector('text=已从侧车读取')
  await page.settle(300)
  await page.shot('settings-xmp')

  // shortcuts (read only, generated from the registry)
  await section('keys')
  expect((await page.locator('[data-testid="settings-keys"] kbd').count()) > 30, 'shortcut table generated from the keymap')
  expect((await page.getByTestId('settings-keys').textContent()).includes('AI 助手'), 'assistant shortcut listed')
  await page.shot('settings-keys')

  // appearance
  await section('appearance')
  await page.getByTestId('setting-theme').selectOption('light')
  await page.settle(400)
  await page.shot('settings-appearance-light')
  expect((await page.evaluate(() => document.documentElement.dataset.theme)) === 'light', 'light theme applied')
  await page.getByTestId('setting-theme').selectOption('system')
  await page.getByTestId('setting-language').selectOption('en')
  await page.settle(400)
  await page.shot('settings-appearance-en')
  expect((await page.getByTestId('settings-nav-hardware').textContent())?.includes('Hardware'), 'English UI')
  await page.getByTestId('setting-language').selectOption('zh-CN')
  await page.getByTestId('setting-theme').selectOption('dark')
  await page.settle(300)
  const s3 = await page.api('/api/settings')
  expect(s3.language === 'zh-CN' && s3.theme === 'dark', 'language / theme persisted')

  // narrow viewport
  await page.setViewportSize({ width: 390, height: 844 })
  await section('lan')
  await page.settle(300)
  await page.shot('settings-lan-mobile')
  await page.context().close()
}

// =================================================================== B. assistant drawer
{
  step('assistant drawer')
  const page = await newPage()
  const sid = await openLibrary(page)
  await page.keyboard.press('Control+j')
  await page.waitForSelector('[data-testid="assistant-drawer"]')
  await page.settle(400)
  await page.shot('assistant-empty')
  expect((await page.getByTestId('engine-badge').getAttribute('data-engine')) === 'rules', 'engine badge: rules')
  expect((await page.getByTestId('engine-badge').textContent()) === '规则', 'engine badge label')

  // unsupported
  await page.getByTestId('assistant-input').fill('帮我煮杯咖啡')
  await page.getByTestId('assistant-send').click()
  await page.waitForSelector('[data-testid="plan-unsupported"]')
  await page.shot('assistant-unsupported')
  expect((await page.getByTestId('plan-unsupported').textContent()).includes('只看 4 星以上'), 'unsupported shows examples')

  // filter-only plan applies immediately (chip)
  const before = Number((await page.api(`/api/photos?session_id=${sid}&limit=1`)).total)
  await page.getByTestId('chip-picked').click()
  await page.waitForSelector('[data-testid="plan-card"][data-phase="applied"]')
  await page.settle(800)
  const shown = await page.evaluate(() => document.querySelectorAll('[data-id]').length)
  const picked = (await page.api(`/api/photos?session_id=${sid}&flag=picked&limit=1`)).total
  expect(picked < before && picked > 0, 'some photos are picked')
  expect(shown > 0 && shown <= picked, `grid shows only picked photos (${shown} <= ${picked})`)
  await page.shot('assistant-filter-applied')

  // reset the filter by hand via the assistant: "只看 4 星以上" then clear
  await page.evaluate(() => {
    const raw = localStorage.getItem('imagepicker.ui')
    void raw
  })

  // destructive plan: keep top 2 of each scene, reject the rest
  const c0 = await sessionCounts(page, sid)
  await page.getByTestId('assistant-input').fill('每个场景保留前2张，淘汰其余')
  await page.getByTestId('assistant-send').click()
  await page.waitForSelector('[data-testid="plan-card"][data-phase="planned"]')
  expect((await page.getByTestId('destructive-badge').count()) >= 1, 'destructive badge shown')
  expect((await page.getByTestId('plan-step').last().getAttribute('data-tool')) === 'scene_keep_top', 'scene_keep_top step')
  await page.shot('assistant-plan-destructive')
  await page.getByTestId('plan-preview').click()
  await page.settle(300)

  await page.evaluate(() => (globalThis.__m6StepMs = 1600))
  await page.getByTestId('plan-execute').click()
  await page.waitForSelector('[data-testid="plan-confirm"]')
  await page.shot('assistant-confirm')
  await page.getByTestId('plan-confirm').click()
  await page.waitForSelector('[data-testid="plan-progress"]')
  await page.settle(300)
  await page.shot('assistant-executing')
  await page.waitForSelector('[data-testid="plan-result"]', { timeout: 15000 })
  await page.evaluate(() => (globalThis.__m6StepMs = 100))
  await page.settle(500)
  await page.shot('assistant-done-undo')
  const c1 = await sessionCounts(page, sid)
  expect(c1.rejected_count > c0.rejected_count, `executing rejected photos (${c0.rejected_count} -> ${c1.rejected_count})`)

  // undo the whole execution as ONE step
  await page.getByTestId('plan-undo').click()
  await page.waitForSelector('[data-testid="plan-result"]:has-text("已撤销")', { timeout: 10000 })
  await page.settle(600)
  const c2 = await sessionCounts(page, sid)
  expect(c2.rejected_count === c0.rejected_count && c2.picked_count === c0.picked_count, `undo restored counts (${c2.rejected_count}/${c2.picked_count} vs ${c0.rejected_count}/${c0.picked_count})`)

  // the same via Ctrl+Z: ONE history step
  await page.getByTestId('assistant-input').fill('每个场景保留前2张，淘汰其余')
  await page.getByTestId('assistant-send').click()
  await runLastPlan(page, { confirm: true })
  const c3 = await sessionCounts(page, sid)
  expect(c3.rejected_count > c0.rejected_count, 're-executed')
  await page.getByTestId('assistant-close').click()
  await page.locator('body').click({ position: { x: 5, y: 450 } })
  await page.keyboard.press('Control+z')
  await page.settle(900)
  const c4 = await sessionCounts(page, sid)
  expect(c4.rejected_count === c0.rejected_count && c4.picked_count === c0.picked_count, `ONE Ctrl+Z reverts the whole plan (${c4.rejected_count} vs ${c0.rejected_count})`)

  // edit-writing plan: unify tone -> edit stacks; undo restores. (Reset the picked-only filter first.)
  await page.keyboard.press('Control+j')
  await page.waitForSelector('[data-testid="assistant-drawer"]')
  await page.getByTestId('assistant-input').fill('统一色调')
  await page.getByTestId('assistant-send').click()
  await runLastPlan(page)
  const edited = (await page.api(`/api/photos?session_id=${sid}&has_edits=1&limit=1`)).total
  expect(edited > 0, `tone plan wrote edits (${edited})`)
  await page.screenshot({ path: `${OUT}assistant-tone-done.png` })
  await page.locator('[data-testid="plan-undo"]:not([disabled])').last().click()
  await page.waitForTimeout(1500)
  const edited2 = (await page.api(`/api/photos?session_id=${sid}&has_edits=1&limit=1`)).total
  expect(edited2 === 0, `undo removed the edits (${edited2})`)

  await page.context().close()
}

// =================================================================== C. edit page suggestions
{
  step('edit suggestions')
  const page = await newPage()
  const sid = await openLibrary(page)
  const first = await page.locator('[data-id]').first().getAttribute('data-id')
  await page.nav(`/s/${sid}/edit/${first}`)
  await page.waitForSelector('[data-testid="edit-page"]')
  await page.waitForSelector('[data-testid="suggest-edits"]')
  await page.getByTestId('suggest-edits').click()
  await page.waitForSelector('[data-testid="suggestions"]', { timeout: 15000 })
  await page.settle(400)
  await page.shot('edit-suggestions')
  const stackBefore = await page.api(`/api/edits/${first}`)
  await page.getByTestId('suggest-apply').click()
  await page.waitForTimeout(1200)
  const stackAfter = await page.api(`/api/edits/${first}`)
  expect(stackAfter.stack.ops.length > 0 && stackBefore.stack.ops.length === 0, 'suggestion saved as an edit')
  await page.shot('edit-suggestions-applied')
  await page.getByTestId('edit-undo').click()
  await page.waitForTimeout(1200)
  const stackUndone = await page.api(`/api/edits/${first}`)
  expect(stackUndone.stack.ops.length === 0, 'suggestion is one undoable change')
  // assistant drawer inside the edit page
  await page.keyboard.press('Control+j')
  await page.waitForSelector('[data-testid="assistant-drawer"]')
  await page.shot('edit-assistant')
  await page.context().close()
}

// =================================================================== D. onboarding
{
  step('onboarding')
  const page = await newPage('/?mock_onboarding=1')
  await page.waitForSelector('[data-testid="onboarding-card"]')
  await page.settle(500)
  await page.shot('onboarding-card')
  expect((await page.getByTestId('onboarding-tier').textContent()) === 'T3', 'onboarding shows tier')
  await page.getByTestId('onboarding-basic').click()
  await page.waitForSelector('[data-testid="onboarding-card"]', { state: 'detached' })
  const sid = await openLibrary(page)
  void sid
  const marks = []
  for (let i = 0; i < 3; i++) {
    await page.waitForSelector('[data-testid="coach-bubble"]')
    await page.settle(450)
    marks.push(await page.getByTestId('coach-bubble').getAttribute('data-mark'))
    await page.shot(`onboarding-coach-${i + 1}`)
    await page.getByTestId('coach-next').click()
  }
  expect(marks.join() === 'analyze,stacks,rating', `coach marks in order (${marks})`)
  await page.settle(400)
  expect((await page.getByTestId('coach-bubble').count()) === 0, 'coach marks gone')
  expect(page.reqs.some((r) => r.url === '/api/onboarding/done'), 'POST /api/onboarding/done sent')
  expect((await page.api('/api/onboarding')).first_run === false, 'first_run false afterwards')
  await page.context().close()

  // "download AI components" path
  const p2 = await newPage('/?mock_onboarding=1')
  await p2.waitForSelector('[data-testid="onboarding-card"]')
  await p2.getByTestId('onboarding-download').click()
  await p2.waitForSelector('[data-testid="onboarding-card"]', { state: 'detached' })
  expect(p2.reqs.some((r) => r.url === '/api/models/ensure'), 'download triggers /api/models/ensure')
  await p2.settle(600)
  await p2.shot('onboarding-downloading')
  await p2.context().close()
}

// =================================================================== E. LAN login + roles
{
  step('login / guest / logout')
  const page = await newPage('/s/2?mock_auth=locked')
  await page.waitForSelector('[data-testid="login-page"]')
  expect(page.url().includes('/login?next='), `redirected to login with next (${page.url()})`)
  await page.settle(400)
  await page.shot('login')
  // wrong password
  await page.getByTestId('login-password').fill('nope')
  await page.getByTestId('login-submit').click()
  await page.waitForSelector('[data-testid="login-error"][data-kind="wrong"]')
  await page.shot('login-wrong')
  // rate limit after 5 failures
  for (let i = 0; i < 4; i++) {
    await page.getByTestId('login-password').fill(`bad${i}`)
    await page.getByTestId('login-submit').click()
    await page.waitForSelector('[data-testid="login-error"]')
    await page.settle(120)
  }
  // age the failures so the countdown is short (real window: 60 s)
  await page.evaluate(() => globalThis.__mockM6.auth.failures.fill(Date.now() - 57000))
  await page.getByTestId('login-password').fill('bad-again')
  await page.getByTestId('login-submit').click()
  await page.waitForSelector('[data-testid="login-error"][data-kind="rateLimited"]')
  expect(/\d+ 秒/.test(await page.getByTestId('login-error').textContent()), 'rate limit countdown shown')
  await page.shot('login-rate-limited')
  // still on /login (no redirect loop); the mock's in-memory counter is reset by hand below
  expect(new URL(page.url()).pathname === '/login', 'still on the login page')
  await page.waitForFunction(() => !document.querySelector('[data-testid="login-submit"]').disabled || !document.querySelector('[data-testid="login-password"]').value, null, { timeout: 8000 })
  // guest login returns to the previous route
  await page.evaluate(() => globalThis.__mockM6.auth.failures.splice(0))
  await page.getByTestId('login-password').fill('guest123')
  await page.waitForFunction(() => !document.querySelector('[data-testid="login-submit"]').disabled, null, { timeout: 8000 })
  await page.getByTestId('login-submit').click()
  await page.waitForSelector('[data-id]', { timeout: 15000 })
  expect(/\/s\/2$/.test(new URL(page.url()).pathname), `back on the previous route (${page.url()})`)
  await page.settle(800)
  await page.shot('guest-library')
  expect((await page.getByTestId('guest-badge').count()) === 1, 'guest badge');
  for (const id of ['people-link', 'settings-link', 'assistant-toggle', 'analyze-button']) expect((await page.getByTestId(id).count()) === 0, `guest hides ${id}`)
  expect((await page.getByRole('button', { name: /导出/ }).count()) === 0, 'guest hides export')
  const reqsBefore = page.reqs.length
  await page.locator('[data-id]').first().click()
  await page.keyboard.press('3')
  await page.settle(500)
  expect(!page.reqs.slice(reqsBefore).some((r) => r.method === 'PATCH'), 'guest rating shortcut sends no PATCH')
  expect((await page.locator('text=只读访客不能修改').count()) >= 1, 'read-only toast shown')
  await page.keyboard.press('Control+j')
  await page.settle(300)
  expect((await page.getByTestId('assistant-drawer').count()) === 0, 'guest cannot open the assistant')
  await page.nav('/settings')
  await page.waitForSelector('a[href^="/s/"]')
  expect(new URL(page.url()).pathname === '/', 'guest bounced from /settings')
  expect((await page.getByTestId('home-settings-link').count()) === 0, 'no settings link on the guest home')
  await page.shot('guest-home')
  await page.nav('/s/2/people')
  await page.waitForSelector('[data-id]')
  expect(new URL(page.url()).pathname === '/s/2', 'guest bounced from the people page')

  // logout from the top bar menu, then log in as owner
  await page.getByTestId('user-menu').click()
  await page.shot('user-menu')
  await page.getByTestId('logout').click()
  await page.waitForSelector('[data-testid="login-page"]')
  await page.getByTestId('login-password').fill('owner123')
  await page.getByTestId('login-submit').click()
  await page.waitForSelector('a[href^="/s/"]', { timeout: 10000 })
  expect(new URL(page.url()).pathname === '/', 'after logout + login the previous route (home) is restored')
  expect((await page.getByTestId('home-settings-link').count()) === 1, 'owner sees settings')
  await page.shot('owner-lan-home')
  await page.context().close()
}

// =================================================================== F. XMP conflict toast
{
  step('xmp conflict')
  const page = await newPage()
  const sid = await openLibrary(page)
  await page.api('/api/settings', { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ xmp_mode: 'sidecar' }) })
  const id = Number(await page.locator('[data-id]').first().getAttribute('data-id'))
  await page.evaluate((photo_id) => globalThis.__mockEmit({ type: 'xmp.conflict', photo_id, sidecar: { rating: 2 }, catalog: { rating: 4 } }), id)
  await page.waitForSelector('text=以目录库为准')
  await page.settle(300)
  await page.shot('xmp-conflict-toast')
  await page.getByRole('button', { name: '以目录库为准' }).click()
  await page.waitForSelector('text=侧车已更新')
  const sync = page.reqs.find((r) => r.url === '/api/xmp/sync')
  expect(sync && JSON.parse(sync.body).direction === 'write' && JSON.parse(sync.body).session_id === sid, 'catalog wins -> xmp/sync write for the session')
  await page.evaluate((photo_id) => globalThis.__mockEmit({ type: 'xmp.conflict', photo_id, sidecar: { rating: -1 }, catalog: { rating: 0 } }), id)
  await page.waitForSelector('text=以侧车为准')
  await page.getByRole('button', { name: '以侧车为准' }).click()
  await page.waitForSelector('text=已以侧车为准')
  expect(page.reqs.filter((r) => r.url === '/api/xmp/sync').some((r) => JSON.parse(r.body).direction === 'read'), 'sidecar wins -> xmp/sync read')
  await page.context().close()
}

await browser.close()
console.log(`intentional errors seen: ${JSON.stringify(allowed)}`)
if (errors.length) {
  console.log('\nFAILURES:')
  for (const e of errors) console.log(' -', e)
  process.exit(1)
}
console.log('\nM6 screenshots OK ->', OUT)
