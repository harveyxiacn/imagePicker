// Functional smoke test of the M2 flows against the mock backend (accept AI + undo, accept all, cancel, people -> library).
//   BASE_URL=http://127.0.0.1:5247 node scripts/e2e-m2.mjs
import { chromium } from 'playwright'

const BASE = process.env.BASE_URL ?? 'http://localhost:5173'
const errors = []
const browser = await chromium.launch()
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'zh-CN' })).newPage()
page.on('console', (m) => {
  // The 409 models_missing response is the documented first-run flow, not a bug.
  if (m.type() === 'error' && !/status of 409/.test(m.text())) errors.push(`console: ${m.text()}`)
})
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))
const settle = (ms = 400) => page.waitForTimeout(ms)
const ok = (cond, msg) => {
  if (!cond) {
    errors.push(`assert: ${msg}`)
    console.log('✗', msg)
  } else console.log('✓', msg)
}

async function open(title) {
  await page.goto(BASE)
  await page.waitForSelector('a[href^="/s/"]')
  await page.locator('a[href^="/s/"]', { hasText: title }).click()
  await page.waitForSelector('[data-id]')
}

// ---- cancel a long run (3,000 photos)
await open('京都')
await page.getByTestId('analyze-button').click()
await page.getByRole('radio').first().check() // fast profile
await page.getByTestId('analyze-start').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="analysis-progress"]', { timeout: 20000 })
await settle(600)
await page.getByTestId('analysis-cancel').click()
await page.waitForSelector('[data-testid="analysis-progress"]', { state: 'detached', timeout: 10000 })
ok(true, 'cancel removes the progress bar')

// ---- accept AI / undo on a fresh session (standard profile; models are installed now only for the fast set)
await open('小明生日')
await page.getByTestId('analyze-button').click()
await page.getByRole('radio').nth(1).check()
await page.getByTestId('analyze-start').click()
await page.waitForSelector('[data-testid="models-dialog"] li')
await page.getByTestId('models-confirm').click()
await page.waitForSelector('[data-testid="analysis-progress"]', { timeout: 20000 })
await page.waitForSelector('[data-testid="analysis-progress"]', { state: 'detached', timeout: 60000 })
await settle(2000)

const cell = page.locator('[role="gridcell"]:has([data-testid="ai-badge"])').first()
await cell.click()
const id = await cell.getAttribute('data-id')
const text = async () => (await page.locator(`[data-id="${id}"]`).innerText()).replace(/\s+/g, '')
const before = await text()
const ai = Number(/☆(\d(?:\.\d)?)/.exec(before)?.[1])
ok(Number.isFinite(ai), `cell shows AI rating (☆${ai})`)
await page.keyboard.press('a')
await settle(600)
const stars = '★'.repeat(Math.round(ai))
ok((await text()).includes(stars) && !(await text()).includes('☆'), `A accepts AI rating -> ${stars}`)
await page.keyboard.press('Control+z')
await settle(600)
ok((await text()).includes(`☆${ai.toFixed(1)}`), 'Ctrl+Z restores the AI-only state')

await page.keyboard.press('Control+Shift+A')
await page.waitForSelector('[data-testid="accept-all-confirm"]')
await page.getByTestId('accept-all-confirm').click()
await settle(900)
ok((await text()).includes(stars), 'Ctrl+Shift+A accepts all (after confirmation)')
await page.keyboard.press('Control+z')
await settle(900)
ok((await text()).includes(`☆${ai.toFixed(1)}`), 'one Ctrl+Z undoes the whole batch')

// ---- stack toggle / group navigation keys
const badge = page.getByTestId('stack-badge').first()
await badge.scrollIntoViewIfNeeded()
const countBefore = await page.locator('[role="gridcell"]').count()
await badge.locator('xpath=ancestor::div[@data-id]').click()
await page.keyboard.press('s')
await settle(300)
ok((await page.locator('[role="gridcell"]').count()) > countBefore, 'S expands the stack under the cursor')
await page.keyboard.press('s')
await settle(300)
ok((await page.locator('[role="gridcell"]').count()) === countBefore, 'S collapses it again')
await page.keyboard.press('Shift+S')
await settle(300)
ok((await page.locator('[role="gridcell"]').count()) > countBefore, 'Shift+S expands all')
await page.keyboard.press('Shift+S')
await settle(300)

// ---- flat / grouped toggle
await page.getByTestId('grouped-toggle').click()
await settle(300)
ok((await page.getByTestId('stack-badge').count()) === 0 && (await page.getByTestId('scene-header').count()) === 0, 'flat mode shows no stacks or headers')
await page.getByTestId('grouped-toggle').click()

// ---- issue + ai filters
await page.getByTestId('issues-filter').selectOption('closed_eyes')
await settle(700)
ok((await page.getByTestId('filter-pills').count()) === 1, 'issue filter shows a pill')
await page.getByTestId('issues-filter').selectOption('all')
await page.getByTestId('ai-rating-filter').selectOption('4')
await settle(700)
const shown = await page.getByTestId('showing').innerText()
console.log('  AI >= 4:', shown)
await page.getByTestId('ai-rating-filter').selectOption('0')

// ---- people page -> library filtered by person
await page.getByTestId('people-link').click()
await page.waitForSelector('[data-testid="person-card"]')
const nameBtn = page.getByTestId('person-name').first()
await nameBtn.click()
await page.getByTestId('person-name-input').fill('测试人物')
await page.keyboard.press('Enter')
await settle(700)
ok((await page.getByTestId('person-name').first().innerText()).includes('测试人物'), 'inline rename')
await page.getByTestId('person-select').nth(0).click()
await page.getByTestId('person-select').nth(1).click()
await page.getByTestId('people-merge').click()
await page.getByTestId('people-merge-confirm').click()
await settle(900)
ok((await page.getByTestId('person-card').count()) === 8, 'merge two people -> 8 cards')
await page.getByTestId('person-card').first().locator('button').first().click()
await page.waitForSelector('[data-id]')
await settle(900)
ok((await page.getByTestId('filter-pill').count()) === 1, 'person card opens the library with a person pill')
console.log('  person filter result:', await page.getByTestId('showing').innerText())

await browser.close()
if (errors.length) {
  console.error('FAILURES:\n' + errors.join('\n'))
  process.exit(1)
}
console.log('OK')
