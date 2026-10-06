// Visual smoke test against the mock backend: `pnpm dev:mock` then `pnpm screenshots`.
// Saves PNGs to web/screenshots/ (git-ignored) and fails on console errors.
import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'

const BASE = process.env.BASE_URL ?? 'http://localhost:5173'
const OUT = new URL('../screenshots/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')
mkdirSync(OUT, { recursive: true })

const errors = []
const browser = await chromium.launch()
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'zh-CN' })
const page = await ctx.newPage()
page.on('console', (m) => {
  if (m.type() === 'error') errors.push(`console: ${m.text()}`)
})
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))

const shot = (name) => page.screenshot({ path: `${OUT}${name}.png` })
const settle = (ms = 600) => page.waitForTimeout(ms)

// 1. Home
await page.goto(BASE)
await page.waitForSelector('a[href^="/s/"]')
await settle(800)
await shot('01-home')

// folder browser dialog
await page.getByRole('button', { name: '选择文件夹' }).first().click()
await page.waitForSelector('[role="dialog"] ul button')
await settle(300)
await shot('02-folder-browser')
await page.keyboard.press('Escape')

// 2. Grid (3,000 photos)
await page.locator('a[href^="/s/"]').first().click()
await page.waitForSelector('[data-id]')
await settle(1200)
await shot('03-grid')
console.log('showing:', await page.getByTestId('showing').innerText())

// rate / flag / label with the keyboard, then undo
await page.locator('[data-id]').nth(2).click()
await page.keyboard.press('4')
await page.keyboard.press('p')
await page.keyboard.press('7')
await settle(300)
await page.locator('[data-id]').nth(5).click({ modifiers: ['Shift'] })
await settle(300)
await shot('04-grid-selection')
await page.keyboard.press('Control+z')
await settle(200)

// filter: rating >= 3
await page.getByRole('button', { name: '3', exact: true }).first().click()
await settle(600)
console.log('showing (>=3):', await page.getByTestId('showing').innerText())
await shot('05-grid-filtered')
await page.getByRole('button', { name: '清除筛选' }).click()
await settle(400)

// 3. Loupe
await page.locator('[data-id]').nth(8).dblclick()
await page.waitForSelector('[data-testid="zoom-pane"]')
await settle(1200)
await shot('06-loupe')
const pane = page.getByTestId('zoom-pane')
const box = await pane.boundingBox()
await page.mouse.move(box.x + box.width * 0.6, box.y + box.height * 0.4)
for (let i = 0; i < 6; i++) await page.mouse.wheel(0, -300)
await settle(900)
await shot('07-loupe-zoomed')
await page.keyboard.press('ArrowRight')
await settle(800)

// 4. Compare
await page.keyboard.press('c')
await page.waitForSelector('[data-testid="zoom-pane"]')
await settle(1200)
await shot('08-compare-2up')
await page.getByRole('button', { name: '4-up' }).click()
await settle(1000)
await shot('09-compare-4up')

// 5. Help overlay
await page.keyboard.press('?')
await page.waitForSelector('[data-testid="help-overlay"]')
await settle(300)
await shot('10-help')
await page.keyboard.press('Escape')

// 6. Export dialog
await page.keyboard.press('Control+e')
await page.waitForSelector('#exp-dest')
await page.fill('#exp-dest', 'D:\\Export')
await settle(200)
await shot('11-export')
await page.getByRole('button', { name: /^导出 \d+ 张$/ }).click()
await settle(900)
await shot('12-export-progress')

// 6b. Import flow with live WS updates (photos.added / thumbs.ready / session.updated)
await page.goto(BASE)
await page.waitForSelector('a[href^="/s/"]')
await page.getByRole('button', { name: '选择文件夹' }).first().click()
await page.waitForSelector('[role="dialog"] ul button')
await page.getByRole('button', { name: 'Pictures' }).click()
await page.getByRole('button', { name: 'Wedding-2025' }).click()
await page.getByRole('button', { name: '导入此文件夹' }).click()
await page.waitForSelector('[data-id]')
await settle(1500)
await shot('12b-import-live')
console.log('import live showing:', await page.getByTestId('showing').innerText())

// 7. 20k stress: scroll the grid and measure frame times
await page.goto(BASE)
await page.waitForSelector('a[href^="/s/"]')
await page.locator('a[href^="/s/"]', { hasText: '压力测试' }).click()
await page.waitForSelector('[data-id]', { timeout: 20000 })
await settle(1500)
const stats = await page.evaluate(async () => {
  const el = document.querySelector('[data-testid="grid"]')
  const frames = []
  let last = performance.now()
  let running = true
  const loop = (t) => {
    frames.push(t - last)
    last = t
    if (running) requestAnimationFrame(loop)
  }
  requestAnimationFrame(loop)
  const total = el.scrollHeight - el.clientHeight
  for (let i = 0; i < 120; i++) {
    el.scrollTop = (total * i) / 120
    await new Promise((r) => requestAnimationFrame(r))
  }
  running = false
  frames.sort((a, b) => a - b)
  return {
    rows: total,
    mounted: document.querySelectorAll('[data-id]').length,
    median: frames[Math.floor(frames.length / 2)],
    p95: frames[Math.floor(frames.length * 0.95)],
    max: frames[frames.length - 1],
  }
})
console.log('20k scroll stats:', stats)
await settle(800)
await shot('13-grid-20k')

// 8. Light theme + English + tablet width
await page.setViewportSize({ width: 820, height: 1100 })
await page.getByRole('button', { name: /主题/ }).click()
await settle(500)
await shot('14-tablet-light')

await browser.close()
if (errors.length) {
  console.error('ERRORS:\n' + [...new Set(errors)].join('\n'))
  process.exit(1)
}
console.log('no console errors')
