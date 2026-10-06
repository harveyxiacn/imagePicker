// M4 visual smoke test against the mock backend:
//   pnpm dev:mock -- --port 5411 --host 127.0.0.1   then   BASE_URL=http://127.0.0.1:5411 node scripts/screenshots-m4.mjs
// Saves PNGs to web/screenshots/m4/ (git-ignored) and fails on unexpected console errors.
// Intentional 409s: the first `beauty/prepare` answers 409 models_missing (documented consent flow).
import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'

const BASE = process.env.BASE_URL ?? 'http://127.0.0.1:5411'
const OUT = new URL('../screenshots/m4/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')
mkdirSync(OUT, { recursive: true })

const errors = []
const browser = await chromium.launch()
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'zh-CN' })
const page = await ctx.newPage()
let seen409 = 0
page.on('console', (m) => {
  if (m.type() !== 'error') return
  if (/status of 409/.test(m.text())) {
    seen409++
    return
  }
  errors.push(`console: ${m.text()}`)
})
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))

const shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
const settle = (ms = 500) => page.waitForTimeout(ms)
const step = (s) => console.log('·', s)
const expect = (cond, msg) => {
  if (!cond) errors.push(`assert: ${msg}`)
}
const frameSrc = () => page.getByTestId('render-frame').getAttribute('src')
const waitFrameChange = async (before, what) => {
  for (let i = 0; i < 50; i++) {
    const s = await frameSrc().catch(() => null)
    if (s && s !== before) return s
    await settle(100)
  }
  errors.push(`assert: preview frame did not change (${what})`)
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
  const btn = sec.locator('button[aria-expanded]').first()
  if ((await btn.getAttribute('aria-expanded')) === 'false') await btn.click()
  await sec.scrollIntoViewIfNeeded()
  await settle(200)
}
// Client-side navigation: a full reload would reset the in-memory mock backend.
const nav = (path) =>
  page.evaluate((p) => {
    history.pushState({}, '', p)
    dispatchEvent(new PopStateEvent('popstate'))
  }, path)
const api = (path, init) => page.evaluate(([p, i]) => fetch(p, i).then((r) => r.json()), [path, init])

// ------------------------------------------------------------------ 0. analyse the small session
await page.goto(BASE)
await page.waitForSelector('a[href^="/s/"]')
await page.locator('a[href^="/s/"]', { hasText: '小明生日' }).click()
await page.waitForSelector('[data-id]')
await settle(800)
const sid = Number(/\/s\/(\d+)/.exec(page.url())[1])

step('analyse')
await page.getByTestId('analyze-button').click()
await page.getByTestId('analyze-start').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="analysis-progress"]', { timeout: 20000 })
await page.waitForSelector('[data-testid="analysis-progress"]', { state: 'detached', timeout: 60000 })
await settle(1800)

// ------------------------------------------------------------------ 1. smart collections sidebar
step('collections')
await page.waitForSelector('[data-testid="collections-sidebar"]')
await page.waitForFunction(() => [...document.querySelectorAll('[data-testid="collection-count"]')].every((e) => e.textContent !== '…'))
await shot('01-collections-sidebar')
const rows = await page.getByTestId('collection-row').count()
expect(rows === 4, `4 built-in collections listed (${rows})`)
const total = Number((await page.getByTestId('showing').innerText()).match(/\d[\d,]*/g)?.[0].replace(/,/g, ''))
await page.locator('[data-collection="best_per_group"] [data-testid="collection-apply"]').click()
await settle(800)
const bestShown = Number((await page.getByTestId('showing').innerText()).match(/\d[\d,]*/g)?.[0].replace(/,/g, ''))
expect(bestShown > 0 && bestShown < total, `best-per-group applies a query (${bestShown} < ${total})`)
await page.locator('[data-collection="best_per_group"] [data-testid="collection-apply"]').first().waitFor()
expect((await page.locator('[data-collection="best_per_group"] [data-testid="collection-apply"]').getAttribute('aria-current')) === 'true', 'active collection is highlighted')
await shot('02-collection-applied')

// save the current filter (best + rating>=2) as a user collection, rename, delete
await page.getByRole('button', { name: '2', exact: true }).first().click()
await settle(500)
await page.getByTestId('filter-save-collection').click()
await page.getByTestId('collection-name-input').fill('精选候选')
await shot('03-save-collection-dialog')
await page.getByTestId('collection-save-confirm').click()
await page.waitForSelector('[data-collection="c1"]')
await settle(700)
expect((await page.getByTestId('collection-row').count()) === 5, 'user collection appears')
await page.locator('[data-collection="c1"]').hover()
await page.locator('[data-collection="c1"] [data-testid="collection-rename"]').click()
await page.getByTestId('collection-rename-input').fill('精选候选 v2')
await page.keyboard.press('Enter')
await settle(600)
expect((await page.locator('[data-collection="c1"]').innerText()).includes('v2'), 'collection renamed')
await shot('04-collection-renamed')
await page.getByTestId('collection-all').click()
await settle(500)
await page.locator('[data-collection="c1"] [data-testid="collection-apply"]').click()
await settle(700)
expect((await page.getByTestId('showing').innerText()).length > 0, 'user collection applies')
await page.locator('[data-collection="c1"]').hover()
await page.locator('[data-collection="c1"] [data-testid="collection-delete"]').click()
await page.getByTestId('collection-delete-confirm').click()
await settle(700)
expect((await page.getByTestId('collection-row').count()) === 4, 'user collection deleted')
await page.getByTestId('collection-all').click()
await settle(400)

// person filter popover: save-as-collection + search-by-face entry points
await page.getByTestId('person-filter-button').click()
await page.waitForSelector('[data-testid="person-filter-popover"]')
await settle(300)
await shot('04b-person-popover')
await page.keyboard.press('Escape')
await settle(200)

// ------------------------------------------------------------------ 2. pick a photo with two people, one without pose
step('find portrait photo')
const list = await api(`/api/photos?session_id=${sid}&faces_min=2&limit=200`)
const popular = new Set((await api(`/api/people?session_id=${sid}`)).people.filter((p) => p.photo_count >= 8).map((p) => p.id))
let target = null
for (const p of list.photos) {
  const r = await api(`/api/photos/${p.id}/people`)
  const named = r.people.filter((x) => x.person_id !== null)
  if (named.length >= 2 && named.some((x) => !x.has_pose) && named.some((x) => x.has_pose && x.is_subject && popular.has(x.person_id))) {
    target = { photo: p, people: named }
    break
  }
}
if (!target) {
  // fall back: any photo with >= 2 named people
  for (const p of list.photos) {
    const r = await api(`/api/photos/${p.id}/people`)
    const named = r.people.filter((x) => x.person_id !== null)
    if (named.length >= 2) {
      target = { photo: p, people: named }
      break
    }
  }
}
expect(!!target, 'found a photo with >= 2 people')
const withPose = target.people.find((x) => x.has_pose && x.is_subject && popular.has(x.person_id)) ?? target.people.find((x) => x.has_pose && x.is_subject) ?? target.people[0]
const noPose = target.people.find((x) => !x.has_pose)
console.log('  photo', target.photo.id, 'people', target.people.map((x) => `${x.person_name ?? x.person_id}${x.has_pose ? '' : '(no pose)'}`).join(', '))

// ------------------------------------------------------------------ 3. beauty panel: not ready -> consent -> ready
step('beauty panel')
await nav(`/s/${sid}/edit/${target.photo.id}`)
await page.waitForSelector('[data-testid="render-frame"]')
await settle(900)
await openSection('portrait')
await page.waitForSelector('[data-testid="portrait-prepare"]')
await shot('05-beauty-not-ready')
await page.getByTestId('beauty-prepare').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
expect(seen409 >= 1, 'first prepare answered 409 models_missing (consent flow)')
await settle(300)
await shot('06-beauty-consent')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="portrait-ready"]', { timeout: 30000 })
await settle(600)
await openSection('portrait')
await shot('07-beauty-ready')

// chips: select the person with pose, apply beauty + face sliders
step('sliders')
const chips = page.getByTestId('chip-person')
expect((await chips.count()) >= 2, 'person chips shown')
const poseIdx = target.people.findIndex((x) => x.person_id === withPose.person_id)
await chips.nth(poseIdx).click()
const f0 = await frameSrc()
await setNum('beauty-smooth', 80)
let f1 = await waitFrameChange(f0, 'smooth')
await setNum('beauty-whiten', 60)
f1 = await waitFrameChange(f1, 'whiten')
await setNum('face-slim', 70)
f1 = await waitFrameChange(f1, 'slim')
await setNum('face-eyes', 50)
await waitFrameChange(f1, 'eyes')
await settle(900)
await page.getByTestId('compare-split').click()
await settle(900)
await shot('08-beauty-applied-split')
await page.getByTestId('compare-split').click()
await page.getByTestId('portrait-ready').scrollIntoViewIfNeeded()
await shot('09-beauty-sliders')
// zoomed crop of the face region for a close look
const frame = page.getByTestId('render-frame')
await frame.screenshot({ path: `${OUT}09b-face-after.png` })
await page.getByTestId('compare-toggle').click()
await settle(900)
await frame.screenshot({ path: `${OUT}09c-face-before.png` })
await page.getByTestId('compare-toggle').click()
await settle(500)

// level + naturalness warning
step('level / warning')
expect((await page.getByTestId('unnatural-warning').count()) === 0, 'no warning at Standard')
await page.getByTestId('level-refined').click()
await settle(300)
await setNum('beauty-smooth', 90)
await settle(500)
expect((await page.getByTestId('unnatural-warning').count()) === 1, 'Refined + high values shows the naturalness warning')
await page.getByTestId('unnatural-warning').scrollIntoViewIfNeeded()
await shot('10-unnatural-warning')
await page.getByTestId('level-natural').click()
await settle(300)
expect((await page.getByTestId('unnatural-warning').count()) === 0, 'warning gone at Natural')

// body sliders enabled for a person with pose; disabled for one without
step('body sliders')
await setNum('body-waist', 40)
await settle(700)
expect((await page.getByTestId('no-pose-hint').count()) === 0, 'body enabled with pose')
if (noPose) {
  const idx = target.people.findIndex((x) => x.person_id === noPose.person_id)
  await chips.nth(idx).click()
  await settle(300)
  expect((await page.getByTestId('no-pose-hint').count()) === 1, 'no-pose hint shown')
  expect(await page.getByTestId('body-legs').locator('input[type=range]').isDisabled(), 'body sliders disabled without pose')
  await page.getByTestId('body-sliders').scrollIntoViewIfNeeded()
  await shot('11-body-disabled')
  await chips.nth(poseIdx).click()
  await settle(300)
} else {
  errors.push('assert: no person without pose in the chosen photo (cannot show disabled state)')
}

// undo is one step per slider commit; per-person stack ops
step('undo')
const stackBefore = await api(`/api/edits/${target.photo.id}`)
const nOps = stackBefore.stack.ops.length
expect(nOps >= 3, `per-person ops saved (beauty + warp face + warp body) (${nOps})`)
await settle(500)

// save profile -> chip star -> apply
step('profile')
await page.getByTestId('profile-save').click()
await page.waitForSelector('[data-testid="chip-person"] svg.lucide-star', { timeout: 5000 }).catch(() => errors.push('assert: profile star on chip'))
await settle(500)
await shot('12-profile-saved')

// ------------------------------------------------------------------ 4. apply profiles to a selection from the library (undoable)
step('apply profiles to selection')
const others = await api(`/api/photos?session_id=${sid}&persons=${withPose.person_id}&include_background=1&limit=60`)
const ids = others.photos.map((p) => p.id).filter((id) => id !== target.photo.id).slice(0, 4)
expect(ids.length >= 2, `other photos with the person (${ids.length})`)
await nav(`/s/${sid}`)
await page.waitForSelector('[data-id]')
await settle(900)
// narrow the library to this person so the cells are easy to pick
await page.evaluate(() => 0)
for (const [i, id] of ids.entries()) {
  const cell = page.locator(`[data-id="${id}"]`)
  if ((await cell.count()) === 0) await page.getByTestId('grouped-toggle').click().catch(() => {})
  await page.locator(`[data-id="${id}"]`).first().scrollIntoViewIfNeeded()
  await page.locator(`[data-id="${id}"]`).first().click({ modifiers: i === 0 ? [] : ['Control'] })
}
await page.locator(`[data-id="${ids[0]}"]`).first().click({ button: 'right' })
await page.waitForSelector('[data-testid="photo-menu"]')
await shot('13-photo-context-menu')
await page.getByTestId('menu-apply-profiles').click()
await settle(1500)
const after = await api(`/api/edits/${ids[0]}`)
expect(after.stack.ops.some((o) => o.type === 'beauty' && o.person_id === withPose.person_id), 'profile applied to the first selected photo')
await shot('14-profiles-applied')
await page.keyboard.press('Control+z')
await settle(1500)
const undone = await api(`/api/edits/${ids[0]}`)
expect(!undone.stack.ops.some((o) => o.type === 'beauty'), 'one Ctrl+Z reverts the whole batch')
const undone2 = await api(`/api/edits/${ids[1]}`)
expect(!undone2.stack.ops.some((o) => o.type === 'beauty'), 'batch undo covers every photo')

// ------------------------------------------------------------------ 5. people page
step('people page')
await page.getByTestId('people-link').click()
await page.waitForSelector('[data-testid="person-card"]')
await settle(900)
await page.getByTestId('person-select').nth(0).click()
await page.getByTestId('person-select').nth(1).click()
await settle(300)
await shot('15-people-multiselect')
expect(await page.getByTestId('people-together').isEnabled(), 'together enabled with 2 selected')
await page.getByTestId('people-together').click()
await page.waitForSelector('[data-id]')
await settle(900)
const pills = await page.getByTestId('filter-pills').innerText()
expect(pills.includes('+'), `library filtered with persons AND (${pills.trim()})`)
await shot('16-library-together')
await page.goBack()
await page.waitForSelector('[data-testid="person-card"]')
await settle(500)

step('best N')
await page.getByTestId('person-select').nth(0).click()
await page.getByTestId('person-select').nth(1).click()
await page.getByTestId('people-view-best').click()
await page.waitForSelector('[data-testid="best-section"]', { timeout: 10000 })
await settle(900)
const n3 = await page.getByTestId('best-photo').count()
await shot('17-best-n3')
await page.getByTestId('best-n').selectOption('1')
await settle(900)
const n1 = await page.getByTestId('best-photo').count()
expect(n1 > 0 && n1 < n3, `N selector changes the number of photos (${n3} -> ${n1})`)
await page.getByTestId('best-n').selectOption('5')
await settle(900)
await shot('18-best-n5')

step('export by person')
await page.getByTestId('people-export').click()
await page.waitForSelector('[data-testid="export-folders"]')
await shot('19-export-by-person')
await page.locator('#exp-dest').fill('D:\\Export\\people')
await page.getByRole('button', { name: /导出 \d+ 张/ }).click()
await settle(500)
const last = await page.evaluate(() => globalThis.__lastExport)
expect(last && last.folders && Object.keys(last.folders).length >= 1 && !last.ids, 'export body carries `folders` and no ids')
console.log('  export folders:', JSON.stringify(last?.folders))
await settle(2500)

step('face search')
await page.getByTestId('people-face-search').click()
await page.waitForSelector('[data-testid="face-search-drop"]')
await shot('20-face-search-empty')
const png = await page.evaluate(() => {
  const c = document.createElement('canvas')
  c.width = 480
  c.height = 320
  const g = c.getContext('2d')
  const gr = g.createLinearGradient(0, 0, 480, 320)
  gr.addColorStop(0, '#3a5a8c')
  gr.addColorStop(1, '#d9a066')
  g.fillStyle = gr
  g.fillRect(0, 0, 480, 320)
  for (const [x, y] of [[150, 150], [330, 160]]) {
    g.fillStyle = '#f1c9a5'
    g.beginPath()
    g.ellipse(x, y, 48, 62, 0, 0, Math.PI * 2)
    g.fill()
  }
  return c.toDataURL('image/png').split(',')[1]
})
await page.getByTestId('face-search-input').setInputFiles({ name: 'friend-1.png', mimeType: 'image/png', buffer: Buffer.from(png, 'base64') })
await page.waitForSelector('[data-testid="face-search-results"]', { timeout: 10000 })
await settle(500)
await shot('21-face-search-results')
const nFaces = await page.getByTestId('detected-face').count()
if (nFaces > 1) {
  await page.getByTestId('detected-face').nth(1).click()
  await page.waitForSelector('[data-testid="face-search-results"]')
  await settle(500)
  await shot('22-face-search-second-face')
}
expect((await page.getByTestId('face-candidate').count()) >= 1, 'candidates listed')
expect(/\d+%/.test(await page.getByTestId('candidate-similarity').first().innerText()), 'similarity shown')
await page.getByTestId('face-candidate').first().click()
await page.waitForSelector('[data-id]')
await settle(900)
expect((await page.getByTestId('filter-pills').count()) === 1, 'candidate click filters the library by that person')
await shot('23-face-search-filtered')

// ------------------------------------------------------------------ 6. taste
step('taste')
await nav(`/taste`)
await page.waitForSelector('[data-testid="taste-page"]')
await page.waitForSelector('[data-testid="taste-labels"]')
await settle(500)
const labels0 = Number((await page.getByTestId('taste-labels').innerText()).replace(/\D/g, '').slice(-3))
await shot('24-taste-inactive')
expect((await page.getByTestId('taste-chip').count()) === 0, 'no taste chip while inactive')
await nav(`/s/${sid}`)
await page.waitForSelector('[data-id]')
await settle(700)
await page.locator('[data-id]').first().click()
await page.keyboard.press('Control+a')
await page.keyboard.press('4')
await page.waitForSelector('[data-testid="taste-chip"]', { timeout: 10000 }).catch(() => errors.push('assert: taste chip did not appear after labels grew'))
await settle(600)
await shot('25-taste-chip')
await page.getByTestId('taste-chip').click()
await page.waitForSelector('[data-testid="taste-page"]')
await settle(700)
const labels1 = Number((await page.getByTestId('taste-labels').innerText()).replace(/\D/g, ''))
expect(labels1 > labels0, `label count grew (${labels0} -> ${labels1})`)
expect((await page.getByTestId('taste-trait').count()) >= 3, 'traits listed as sentences')
expect((await page.getByTestId('taste-state-text').innerText()).includes('已启用'), 'active state shown')
await shot('26-taste-active')
await page.getByTestId('taste-reset').click()
await shot('27-taste-reset-confirm')
await page.getByTestId('taste-reset-confirm').click()
await settle(900)
expect((await page.getByTestId('taste-no-traits').count()) === 1, 'reset clears traits')
await shot('28-taste-after-reset')

await browser.close()
if (errors.length) {
  console.error('\nFAILED:\n' + errors.map((e) => ' - ' + e).join('\n'))
  process.exit(1)
}
console.log(`\nOK (intentional 409s seen: ${seen409})`)
