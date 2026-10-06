// M5 visual + behaviour smoke test against the mock backend:
//   pnpm dev:mock -- --port 5431 --host 127.0.0.1   then   BASE_URL=http://127.0.0.1:5431 node scripts/screenshots-m5.mjs
// Saves PNGs to web/screenshots/m5/ (git-ignored) and fails on unexpected console errors.
// Intentional errors (documented flows): 409 models_missing (consent dialog) and one injected 503 (worker unavailable toast).
import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'

const BASE = process.env.BASE_URL ?? 'http://127.0.0.1:5431'
const OUT = new URL('../screenshots/m5/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')
mkdirSync(OUT, { recursive: true })

const errors = []
const browser = await chromium.launch()
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'zh-CN' })
const page = await ctx.newPage()
let seen409 = 0
let seen503 = 0
page.on('console', (m) => {
  if (m.type() !== 'error') return
  if (/status of 409/.test(m.text())) {
    seen409++
    return
  }
  if (/status of 503/.test(m.text())) {
    seen503++
    return
  }
  errors.push(`console: ${m.text()}`)
})
page.on('response', (r) => {
  if (r.status() === 404) console.log('  404:', r.url())
})
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))
const posts = []
page.on('request', (r) => {
  if (r.method() === 'POST' && /\/api\/(besttake|photos\/\d+\/(inpaint|enhance)|bursts\/\d+\/besttake)/.test(r.url())) posts.push({ url: r.url(), body: r.postData() })
})

const shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
const settle = (ms = 500) => page.waitForTimeout(ms)
const step = (s) => console.log('·', s)
const expect = (cond, msg) => {
  if (!cond) {
    errors.push(`assert: ${msg}`)
    console.log('  FAIL', msg)
  }
}
const frameSrc = () => page.getByTestId('render-frame').getAttribute('src')
const waitFrameChange = async (before, what) => {
  for (let i = 0; i < 60; i++) {
    const s = await frameSrc().catch(() => null)
    if (s && s !== before) return s
    await settle(100)
  }
  errors.push(`assert: preview frame did not change (${what})`)
  return before
}
const nav = (path) =>
  page.evaluate((p) => {
    history.pushState({}, '', p)
    dispatchEvent(new PopStateEvent('popstate'))
  }, path)
const api = (path, init) => page.evaluate(([p, i]) => fetch(p, i).then((r) => r.json()), [path, init])
const setFlag = (k, v) => page.evaluate(([key, val]) => (globalThis[key] = val), [k, v])
const openSection = async (id) => {
  const sec = page.getByTestId(`section-${id}`)
  const btn = sec.locator('button[aria-expanded]').first()
  if ((await btn.getAttribute('aria-expanded')) === 'false') await btn.click()
  await sec.scrollIntoViewIfNeeded()
  await settle(200)
}
const waitIdle = async (what) => {
  for (let i = 0; i < 100; i++) {
    if ((await page.getByTestId('gen-task').count()) === 0) return
    await settle(150)
  }
  errors.push(`assert: generative task did not finish (${what})`)
}

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

// ------------------------------------------------------------------ 1. find a burst for Best Take and a photo with bystanders
step('find data')
const groups = await api(`/api/groups?session_id=${sid}`)
let pick = null
for (const sc of groups.scenes) {
  for (const b of sc.bursts) {
    if (b.size < 4) continue
    const plan = await api(`/api/bursts/${b.id}/besttake`)
    if (plan.people.length < 2) continue
    const base = plan.base_photo_id
    // a person who can be improved AND has a non-composable candidate among the top 3
    const rich = plan.people.findIndex((p) => {
      const bestOther = p.candidates.find((c) => c.composable && c.photo_id !== base)
      const baseC = p.candidates.find((c) => c.photo_id === base)
      return bestOther && baseC && bestOther.expression_score > baseC.expression_score && p.candidates.slice(0, 3).some((c) => !c.composable)
    })
    if (rich >= 0 && !pick) pick = { burst: b, plan, rich }
    if (pick) break
  }
  if (pick) break
}
if (!pick) {
  for (const sc of groups.scenes) {
    for (const b of sc.bursts) {
      if (b.size < 3) continue
      const plan = await api(`/api/bursts/${b.id}/besttake`)
      if (plan.people.length >= 2 && !pick) pick = { burst: b, plan, rich: 0 }
    }
  }
}
expect(!!pick, 'found a burst with several people')
console.log('  burst', pick.burst.id, 'size', pick.burst.size, 'base', pick.plan.base_photo_id, 'people', pick.plan.people.length, 'rich person index', pick.rich)

const all = await api(`/api/photos?session_id=${sid}&limit=500`)
let bystanderPhoto = null
for (const p of all.photos) {
  if (!p.analyzed) continue
  const r = await api(`/api/photos/${p.id}/bystanders`)
  if (r.faces.length >= 1 && (!bystanderPhoto || r.faces.length > bystanderPhoto.n)) bystanderPhoto = { id: p.id, n: r.faces.length }
  if (bystanderPhoto && bystanderPhoto.n >= 2) break
}
expect(!!bystanderPhoto, 'found a photo with bystanders')
console.log('  bystander photo', bystanderPhoto?.id, 'faces', bystanderPhoto?.n)

// ------------------------------------------------------------------ 2. group view entry
step('group view')
await page.locator(`[data-id="${pick.plan.base_photo_id}"]`).first().click()
await page.keyboard.press('b')
await page.waitForSelector('[data-testid="expression-matrix"]', { timeout: 10000 })
await settle(900)
const composeBtn = page.getByTestId('matrix-compose')
expect(await composeBtn.isEnabled(), 'compose button is enabled')
await composeBtn.scrollIntoViewIfNeeded()
await shot('01-group-view-entry')

// ------------------------------------------------------------------ 3. Best Take editor
step('best take editor')
await composeBtn.click()
await page.waitForSelector('[data-testid="besttake-page"]')
await page.waitForSelector('[data-testid="bt-face"]')
await page.waitForSelector('[data-testid="render-frame"]')
await settle(900)
const faces = page.getByTestId('bt-face')
const nFaces = await faces.count()
expect(nFaces >= 2, `face boxes shown (${nFaces})`)
await shot('02-besttake-open')

// select the person with a non-composable candidate near the top
await faces.nth(pick.rich).click()
await page.waitForSelector('[data-testid="bt-panel"]')
await settle(500)
const nonComp = page.locator('[data-testid="bt-candidate"][data-composable="false"]')
expect((await nonComp.count()) >= 1, 'a non-composable candidate is listed (greyed)')
await nonComp.first().hover()
await settle(300)
await shot('03-besttake-candidates')
// clicking the greyed candidate does nothing
const f0 = await frameSrc()
await nonComp.first().click({ force: true })
await settle(500)
expect((await page.getByTestId('bt-face-progress').count()) === 0 && (await frameSrc()) === f0, 'non-composable candidate is not selectable')
expect(posts.filter((p) => /\/api\/besttake$/.test(p.url)).length === 0, 'no POST for a non-composable candidate')

// choose the best composable non-base candidate (slow steps so the progress overlay can be captured)
await setFlag('__m5StepMs', 900)
const pickable = page.locator('[data-testid="bt-candidate"][data-composable="true"]:not([data-current="true"])')
const target = pickable.filter({ hasNotText: '底片' }).first()
await target.click()
await page.waitForSelector('[data-testid="bt-face-progress"]', { timeout: 5000 })
await settle(500)
await shot('04-besttake-progress')
expect((await page.getByTestId('gen-task').count()) >= 1, 'progress shown in the status bar')
await page.waitForSelector('[data-testid="bt-face-progress"]', { state: 'detached', timeout: 20000 })
await waitIdle('besttake choice')
await settle(1200)
const replacedIcons = await page.getByTestId('bt-replaced-icon').count()
expect(replacedIcons >= 1, 'replaced face shows the undo icon')
expect((await page.locator('[data-testid="bt-candidate"][data-current="true"]').count()) === 1, 'applied candidate is highlighted')
await shot('05-besttake-result')
const choicePost = posts.filter((p) => /\/api\/besttake$/.test(p.url)).at(-1)
expect(!!choicePost && JSON.parse(choicePost.body).choices.length === 1, 'POST /api/besttake with one choice')
await setFlag('__m5StepMs', 200)

// before / after
await page.keyboard.press('\\')
await settle(900)
await shot('06-besttake-original')
await page.keyboard.press('\\')
await settle(600)

// undo / redo through the shared history
await page.getByTestId('bt-undo').click()
await settle(1100)
expect((await page.getByTestId('bt-replaced-icon').count()) === 0, 'undo removes the replaced mark')
await page.getByTestId('bt-redo').click()
await settle(1100)
expect((await page.getByTestId('bt-replaced-icon').count()) >= 1, 'redo restores it')

// blend sliders + restore this person
const amount = page.getByTestId('bt-amount').locator('input.lr-num')
await amount.click()
await amount.fill('60')
await amount.press('Enter')
await settle(700)
await page.getByTestId('bt-restore').click()
await settle(900)
expect((await page.getByTestId('bt-replaced-icon').count()) === 0, '还原此人 clears the replacement')
await page.getByTestId('bt-undo').click()
await settle(900)

// auto: best for everyone (may pick another base)
await page.getByTestId('bt-auto').click()
await page.waitForSelector('[data-testid="gen-task"]', { timeout: 5000 })
await waitIdle('besttake auto')
await settle(1500)
const replacedAfterAuto = await page.getByTestId('bt-replaced-icon').count()
expect(replacedAfterAuto >= 1, `auto best replaced faces (${replacedAfterAuto})`)
await shot('07-besttake-auto')
const warnCount = await page.getByTestId('bt-warnings').count()
console.log('  warnings visible in panel after auto:', warnCount)

// base selector
await page.getByTestId('bt-base-button').click()
await settle(500)
await shot('08-besttake-base-menu')
await page.keyboard.press('Escape') // closes nothing in the menu; the page handler goes back
await settle(600)

// ------------------------------------------------------------------ 4. bystanders (consent -> removal)
step('bystanders')
await nav(`/s/${sid}/edit/${bystanderPhoto.id}`)
await page.waitForSelector('[data-testid="render-frame"]')
await settle(1000)
await openSection('repair')
await page.waitForSelector('[data-testid="repair-bystanders"]')
await settle(500)
const bBtn = page.getByTestId('repair-bystanders')
expect((await bBtn.innerText()).includes(String(bystanderPhoto.n)), 'button shows the detected count')
await bBtn.hover()
await settle(400)
expect((await page.getByTestId('bystander-box').count()) === bystanderPhoto.n, 'bystander boxes highlighted on hover')
await shot('09-bystander-highlight')
const beforeFrame = await frameSrc()
await bBtn.click()
await page.waitForSelector('[data-testid="models-dialog"] li')
expect(seen409 >= 1, 'inpaint answered 409 models_missing (consent flow)')
await settle(300)
await shot('10-inpaint-consent')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="gen-task"]', { timeout: 30000 })
await settle(300)
await shot('11-inpaint-progress')
await waitIdle('bystander inpaint')
await settle(800)
const afterFrame = await waitFrameChange(beforeFrame, 'bystander removal')
expect(afterFrame !== beforeFrame, 'preview changed after removal')
await page.getByTestId('repair-bystanders').scrollIntoViewIfNeeded()
await settle(500)
await shot('12-bystander-removed')
expect((await page.getByTestId('patch-layer').count()) >= 1, 'inpaint layer listed')

// ------------------------------------------------------------------ 5. brush erase (with zoom + pan)
step('brush')
await page.keyboard.press('Shift+E')
await page.waitForSelector('[data-testid="brush-overlay"]')
await settle(400)
const ov = await page.getByTestId('brush-overlay').boundingBox()
const dragStroke = async (pts) => {
  await page.mouse.move(pts[0][0], pts[0][1])
  await page.mouse.down()
  for (const [x, y] of pts.slice(1)) await page.mouse.move(x, y, { steps: 6 })
  await page.mouse.up()
}
const at = (fx, fy) => [ov.x + ov.width * fx, ov.y + ov.height * fy]
await dragStroke([at(0.55, 0.62), at(0.65, 0.66), at(0.72, 0.6)])
await dragStroke([at(0.57, 0.34), at(0.66, 0.34)])
await settle(300)
expect((await page.getByTestId('brush-strokes').getAttribute('data-count')) === '2', 'two strokes painted')
await shot('13-brush-strokes')
// radius slider
const rad = page.getByTestId('brush-radius').locator('input.lr-num')
await rad.click()
await rad.fill('5')
await rad.press('Enter')
await settle(300)
await dragStroke([at(0.4, 0.3), at(0.45, 0.32)])
await shot('14-brush-radius')
await page.getByTestId('brush-undo').click()
await settle(200)
const beforeBrush = await frameSrc()
await page.getByTestId('brush-apply').click()
await page.waitForSelector('[data-testid="gen-task"]', { timeout: 5000 })
await waitIdle('brush inpaint')
await settle(800)
await waitFrameChange(beforeBrush, 'brush erase')
await shot('15-brush-result')
const strokePost = posts.filter((p) => /\/inpaint$/.test(p.url) && /strokes/.test(p.body ?? '')).at(-1)
const sbody = strokePost ? JSON.parse(strokePost.body) : null
expect(!!sbody && sbody.strokes.length === 2, 'POST strokes (undo removed the last one)')
if (sbody) {
  const [x, y] = sbody.strokes[0].points[0]
  // first stroke started at frame (0.55, 0.62) of the (uncropped) image
  expect(Math.abs(x - 0.55) < 0.03 && Math.abs(y - 0.62) < 0.03, `stroke start maps to normalised coordinates (${x}, ${y})`)
  expect(sbody.strokes.every((s) => s.points.every(([px, py]) => px >= 0 && px <= 1 && py >= 0 && py <= 1) && s.radius > 0 && s.radius <= 0.12), 'stroke values normalised')
}
expect((await page.getByTestId('brush-overlay').count()) === 0, 'brush closes after applying')

// ------------------------------------------------------------------ 6. denoise / face restore / layer list
step('enhance')
await page.getByTestId('repair-bystanders').scrollIntoViewIfNeeded()
const bf = await frameSrc()
await page.getByTestId('enhance-denoise').click()
await page.waitForSelector('[data-testid="models-dialog"] li') // scunet missing -> consent
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="gen-task"]', { timeout: 30000 })
await waitIdle('denoise')
await settle(600)
await waitFrameChange(bf, 'denoise')
const bf2 = await frameSrc()
await page.getByTestId('enhance-face_restore').click()
await page.waitForSelector('[data-testid="models-dialog"] li') // gfpgan missing -> consent
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="gen-task"]', { timeout: 30000 })
await waitIdle('face restore')
await settle(600)
await waitFrameChange(bf2, 'face restore')
await page.getByTestId('patch-layers').scrollIntoViewIfNeeded()
await settle(500)
const nLayers = await page.getByTestId('patch-layer').count()
expect(nLayers >= 4, `layer list shows generated layers (${nLayers})`)
await shot('16-layers')
// toggle a layer (denoise) off -> preview changes; on again; undo; delete
const dn = page.locator('[data-testid="patch-layer"][data-kind="denoise"]')
expect((await dn.count()) === 1, 'one denoise layer')
const bf3 = await frameSrc()
await dn.getByTestId('patch-toggle').click()
await waitFrameChange(bf3, 'layer toggle')
expect((await dn.getAttribute('data-enabled')) === 'false', 'layer disabled in the list')
await shot('17-layer-disabled')
await page.keyboard.press('Control+z')
await settle(900)
expect((await dn.getAttribute('data-enabled')) === 'true', 'undo re-enables the layer')
const nBefore = await page.getByTestId('patch-layer').count()
await page.locator('[data-testid="patch-layer"][data-kind="face_restore"]').first().getByTestId('patch-delete').click()
await settle(900)
expect((await page.getByTestId('patch-layer').count()) < nBefore, 'delete removes layers')
await page.keyboard.press('Control+z')
await settle(900)
expect((await page.getByTestId('patch-layer').count()) === nBefore, 'undo restores the deleted layer')

// ------------------------------------------------------------------ 7. error UX: 503 toast and failed task
step('errors')
await setFlag('__m5Fail', '503')
await page.getByTestId('enhance-denoise').click()
await settle(700)
await shot('18-error-503-toast')
expect((await page.locator('[role="status"]').innerText()).includes('AI 引擎'), '503 shows the worker-unavailable toast')
await setFlag('__m5Fail', 'task')
await page.getByTestId('enhance-denoise').click()
await settle(2200)
expect((await page.locator('[role="status"]').innerText()).includes('worker out of memory'), 'failed task shows its error as toast')
await shot('19-error-task-toast')

// ------------------------------------------------------------------ 8. export dialog: upscale option + tier hint
step('export')
await page.evaluate(() => globalThis.__mockEmit({ type: 'worker.status', state: 'ready', tier: 'T0', error: null }))
await settle(300)
await page.keyboard.press('Control+e')
await page.waitForSelector('#exp-up')
await page.selectOption('#exp-up', '4')
await settle(400)
expect((await page.getByTestId('export-upscale-hint').innerText()).includes('T0'), 'CPU tier hint shown')
await shot('20-export-upscale')
await page.locator('#exp-dest').fill('D:\\Export')
const exportBtn = page.getByRole('button', { name: /^导出 \d+ 张$/ })
await exportBtn.click()
await page.waitForSelector('[data-testid="models-dialog"] li') // realesrgan missing -> 409 -> consent
await settle(300)
await shot('21-export-upscale-consent')
await page.getByTestId('models-confirm').click()
await settle(6000)
const lastExport = await page.evaluate(() => globalThis.__lastExport)
expect(lastExport?.upscale === 4, `export request carries upscale=4 (${JSON.stringify(lastExport?.upscale)})`)

await browser.close()
console.log(`409s seen: ${seen409}, 503s seen: ${seen503}`)
if (errors.length) {
  console.error('\nFAILURES:\n' + errors.map((e) => ' - ' + e).join('\n'))
  process.exit(1)
}
console.log('\nM5 smoke test OK. Screenshots in web/screenshots/m5/')
