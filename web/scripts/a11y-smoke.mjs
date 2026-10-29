// Accessibility smoke test: axe-core in Playwright.
// Works against the mock backend (`pnpm dev:mock`) or a real `imagepicker serve` with an analysed library.
//   terminal 1:  pnpm dev:mock --port 17971 --strictPort      (or: imagepicker serve --web-dir web/dist ...)
//   terminal 2:  BASE_URL=http://127.0.0.1:17971 pnpm a11y
// Prefer the real server on an analysed library: the mock has no bursts / people detail and answers /api in the page,
// so the group view, person page and login states are skipped there (listed at the end of the run).
// Walks every page and the main overlay states (grid, inspector, loupe, compare, group, popovers, palette, help,
// export, assistant, people, person, edit + panels, best-take, my style, settings, login) per viewport and appearance
// mode, saves one axe JSON per state to OUT_DIR and exits 1 when a serious/critical violation is found.
//   BASE_URL            default http://localhost:5173
//   OUT_DIR             default ./a11y-out (one <viewport>-<mode>-<state>.json per audited state + summary.json)
//   A11Y_MODES          dark,light,dark-high,light-high,dark-150   (default: dark,light)
//   A11Y_VIEWPORTS      desktop,mobile                             (default: both; mobile = 390x844 touch)
//   A11Y_FAIL_ON        impacts that fail the run                  (default: serious,critical)
//   A11Y_ONLY           substring filter on state names (debugging)
import { createRequire } from 'node:module'
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { chromium } from 'playwright'

const require = createRequire(import.meta.url)
const AXE_PATH = require.resolve('axe-core/axe.min.js')
const BASE = process.env.BASE_URL ?? 'http://localhost:5173'
const OUT = resolve(process.env.OUT_DIR ?? 'a11y-out')
const FAIL_ON = new Set((process.env.A11Y_FAIL_ON ?? 'serious,critical').split(','))
const ONLY = process.env.A11Y_ONLY
const MODES = {
  dark: { theme: 'dark', contrast: 'normal', fontScale: 100 },
  light: { theme: 'light', contrast: 'normal', fontScale: 100 },
  'dark-high': { theme: 'dark', contrast: 'high', fontScale: 100 },
  'light-high': { theme: 'light', contrast: 'high', fontScale: 100 },
  'dark-150': { theme: 'dark', contrast: 'normal', fontScale: 150 },
}
const VIEWPORTS = {
  desktop: { viewport: { width: 1440, height: 900 }, mobile: false },
  mobile: { viewport: { width: 390, height: 844 }, mobile: true },
}
const modes = (process.env.A11Y_MODES ?? 'dark,light').split(',').map((m) => m.trim())
const viewports = (process.env.A11Y_VIEWPORTS ?? 'desktop,mobile').split(',').map((m) => m.trim())
// best-practice adds the landmark/region/list structure rules on top of WCAG A/AA
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']

mkdirSync(OUT, { recursive: true })
const browser = await chromium.launch()
const failures = []
const summary = []
const runtimeErrors = []
const skipped = []

const sleep = (page, ms) => page.waitForTimeout(ms)

async function audit(page, tag, state) {
  const name = `${tag}-${state}`
  if (ONLY && !name.includes(ONLY)) return
  await page.addScriptTag({ path: AXE_PATH })
  const res = await page.evaluate(
    async (tags) => await globalThis.axe.run(document, { runOnly: { type: 'tag', values: tags }, resultTypes: ['violations'] }),
    TAGS,
  )
  const slim = res.violations.map((v) => ({
    id: v.id,
    impact: v.impact,
    help: v.help,
    nodes: v.nodes.map((n) => ({
      target: n.target,
      html: n.html.slice(0, 240),
      summary: n.failureSummary?.split('\n').slice(1).join(' ').trim(),
      // what each failing check measured (contrast ratio, name vs visible text, ...)
      data: [...n.any, ...n.all, ...n.none].map((c) => c.data).filter(Boolean),
    })),
  }))
  // rendered text of the first nodes of each rule: tells a name/label mismatch apart from a hidden-text artefact
  for (const v of slim)
    for (const n of v.nodes.slice(0, 3)) {
      n.text = await page
        .evaluate((sel) => {
          const el = document.querySelector(sel)
          return el ? { inner: el.innerText?.slice(0, 160), label: el.getAttribute('aria-label')?.slice(0, 160), outer: el.outerHTML.slice(0, 600) } : null
        }, n.target[0])
        .catch(() => null)
    }
  writeFileSync(`${OUT}/${name}.json`, JSON.stringify({ url: page.url(), state, tag, violations: slim }, null, 2))
  const bad = slim.filter((v) => FAIL_ON.has(v.impact))
  summary.push({ name, violations: slim.map((v) => ({ id: v.id, impact: v.impact, nodes: v.nodes.length })) })
  const minor = slim.length - bad.length
  console.log(`${bad.length ? 'FAIL' : 'ok  '} ${name} ${new URL(page.url()).pathname.replace(/\d+/g, 'N')}${minor ? `  (+${minor} moderate/minor)` : ''}`)
  if (bad.length) console.log(`       ${bad.map((v) => `${v.id}(${v.nodes.length})`).join(' ')}`)
  for (const v of bad) failures.push({ name, id: v.id, impact: v.impact, nodes: v.nodes.length })
}

for (const vpName of viewports) {
  const vp = VIEWPORTS[vpName]
  if (!vp) throw new Error(`unknown viewport ${vpName}; use ${Object.keys(VIEWPORTS).join(', ')}`)
  for (const mode of modes) {
    const ui = MODES[mode]
    if (!ui) throw new Error(`unknown mode ${mode}; use ${Object.keys(MODES).join(', ')}`)
    const tag = `${vpName}-${mode}`
    const ctx = await browser.newContext({ viewport: vp.viewport, locale: 'en-US', isMobile: vp.mobile, hasTouch: vp.mobile })
    // the same persisted store the app writes (zustand persist, key imagepicker.ui)
    await ctx.addInitScript((state) => {
      localStorage.setItem('imagepicker.ui', JSON.stringify({ state: { lang: 'en', ...state }, version: 0 }))
    }, ui)
    const page = await ctx.newPage()
    page.setDefaultTimeout(8000)
    page.on('pageerror', (e) => runtimeErrors.push(`[${tag}] pageerror: ${e.message}`))
    // a real server pushes its own theme setting over the persisted one: force the mode under test
    const applyMode = () =>
      page.evaluate((u) => {
        document.documentElement.dataset.theme = u.theme
        document.documentElement.dataset.contrast = u.contrast
      }, ui)
    const go = async (path) => {
      await page.goto(`${BASE}${path}`)
      await page.waitForLoadState('networkidle').catch(() => {})
      await applyMode()
    }
    // each state is independent: a failed setup is recorded and the walk continues
    const state = async (name, fn) => {
      if (ONLY && !`${tag}-${name}`.includes(ONLY)) return
      try {
        await fn()
        await applyMode()
        await sleep(page, 400)
        await audit(page, tag, name)
      } catch (e) {
        // what was on screen helps to tell a wrong selector from a page that never rendered
        const seen = await page
          .evaluate(() => [...document.querySelectorAll('[data-testid]')].slice(0, 25).map((x) => x.getAttribute('data-testid')).join(','))
          .catch(() => '?')
        skipped.push(`${tag}-${name}: ${String(e).split('\n')[0].slice(0, 160)} @${new URL(page.url()).pathname} [${seen}]`)
        console.log(`skip ${tag}-${name}: ${String(e).split('\n')[0].slice(0, 120)}`)
      }
      // Escape closes overlays, but on the edit page it leaves the page, which the next edit states still need
      if (!page.url().includes('/edit/')) await page.keyboard.press('Escape').catch(() => {})
    }
    const dismissOnboarding = async () => {
      for (const id of ['coach-skip', 'onboarding-basic']) {
        const el = page.getByTestId(id)
        if (await el.first().isVisible().catch(() => false)) await el.first().click().catch(() => {})
      }
    }

    // discover a session / photo / burst through the app's own API (mock or real)
    await go('/')
    await dismissOnboarding()
    const ids = await page.evaluate(async () => {
      const j = async (u) => (await fetch(u)).json()
      const sessions = (await j('/api/sessions')).sessions.sort((a, b) => b.photo_count - a.photo_count)
      const sid = sessions[0].id
      const photos = (await j(`/api/photos?session_id=${sid}&limit=1000`)).photos
      const groups = await j(`/api/groups?session_id=${sid}`).catch(() => ({ scenes: [] }))
      const burst = groups.scenes.flatMap((s) => s.bursts).find((b) => b.size > 2)
      return { sid, photo: photos[0]?.id, burst: burst?.id }
    })
    const lib = async () => {
      await go(`/s/${ids.sid}`)
      await page.locator('[data-id]').first().waitFor({ timeout: 20000 })
      await dismissOnboarding()
      await sleep(page, 600)
    }

    await state('home', async () => {
      await go('/')
      await page.waitForSelector('a[href^="/s/"]')
    })
    await state('library', lib)

    if (!vp.mobile) {
      await state('inspector', async () => {
        await lib()
        await page.locator('[data-id]').nth(2).click()
        await page.getByTestId('inspector-toggle').click().catch(() => {})
        await sleep(page, 500)
      })
      await state('loupe', async () => {
        await lib()
        await page.locator('[data-id]').nth(3).dblclick()
        await page.waitForSelector('[data-testid="zoom-pane"]')
        await sleep(page, 1000)
      })
      await state('loupe-faces', async () => {
        await page.keyboard.press('Shift+F')
        await sleep(page, 600)
      })
      await state('compare', async () => {
        await lib()
        await page.locator('[data-id]').nth(3).click()
        await page.locator('[data-id]').nth(4).click({ modifiers: ['Shift'] })
        await page.keyboard.press('c')
        await page.waitForSelector('[data-testid="zoom-pane"]')
        await sleep(page, 900)
      })
      await state('group-view', async () => {
        await lib()
        await page.locator('[data-id]:has([data-testid="stack-badge"])').first().click()
        await page.keyboard.press('b')
        await page.getByTestId('group-view').waitFor({ timeout: 5000 })
        await sleep(page, 600)
      })
      await state('grouped-grid', async () => {
        await lib()
        await page.getByTestId('grouped-toggle').click()
        await sleep(page, 800)
        await page.getByTestId('grouped-toggle').click()
      })
      await state('issues-filter', async () => {
        await lib()
        await page.getByTestId('issues-filter').click()
        await sleep(page, 300)
      })
      await state('person-filter', async () => {
        await lib()
        await page.getByTestId('person-filter-button').click()
        await sleep(page, 300)
      })
      await state('device-filter', async () => {
        await lib()
        await page.getByTestId('device-filter-button').click()
        await sleep(page, 300)
      })
      await state('timeline', async () => {
        await lib()
        await page.getByTestId('timeline-toggle').click()
        await sleep(page, 800)
      })
      await state('photo-menu', async () => {
        await lib()
        await page.locator('[data-id]').nth(1).click({ button: 'right' })
        await page.getByTestId('photo-menu').waitFor({ timeout: 3000 })
      })
      await state('export-dialog', async () => {
        await lib()
        await page.locator('[data-id]').nth(2).click()
        await page.keyboard.press('Control+e')
        await page.waitForSelector('[role="dialog"]')
      })
      await state('command-palette', async () => {
        await lib()
        await page.keyboard.press('Control+k')
        await page.waitForSelector('[data-testid="command-palette"]')
        await page.keyboard.type('undo')
      })
      await state('help-overlay', async () => {
        await lib()
        await page.keyboard.press('?')
        await page.waitForSelector('[data-testid="help-overlay"]')
      })
      await state('assistant-drawer', async () => {
        await lib()
        await page.keyboard.press('Control+j')
        await page.getByTestId('assistant-drawer').waitFor({ timeout: 4000 })
        await sleep(page, 600)
      })
      await state('search-results', async () => {
        await lib()
        await page.getByTestId('search-input').fill('cat')
        await page.keyboard.press('Enter')
        await sleep(page, 2000)
      })
    } else {
      await state('cull-view', async () => {
        await lib()
        await page.locator('[data-id]').first().dblclick()
        await page.getByTestId('cull-view').waitFor({ timeout: 5000 })
        await sleep(page, 700)
      })
      await state('quick-cull', async () => {
        // the previous state's Escape left the cull view
        await lib()
        await page.locator('[data-id]').first().dblclick()
        await page.getByTestId('cull-view').waitFor({ timeout: 5000 })
        await page.getByTestId('quick-cull-toggle').dispatchEvent('click')
        await page.getByTestId('quick-cull').waitFor({ timeout: 5000 })
        await sleep(page, 500)
      })
      await state('mobile-more', async () => {
        await lib()
        await page.getByTestId('mobile-more').click()
        await sleep(page, 500)
      })
      await state('mobile-filter', async () => {
        await lib()
        await page.getByTestId('mobile-filter').click()
        await sleep(page, 500)
      })
      await state('analyze-sheet', async () => {
        await lib()
        await page.getByTestId('analyze-button').click()
        await sleep(page, 500)
      })
    }

    await state('people', async () => {
      await go(`/s/${ids.sid}/people`)
      await page.waitForSelector('[data-testid="people-page"]')
      await sleep(page, 800)
    })
    await state('person-library', async () => {
      await page.getByTestId('person-card').first().getByRole('button').first().click()
      await page.locator('[data-id]').first().waitFor()
      await sleep(page, 900)
    })
    await state('edit', async () => {
      await go(`/s/${ids.sid}/edit/${ids.photo}`)
      await page.getByTestId('edit-canvas').locator('img').first().waitFor({ timeout: 30000 })
      await sleep(page, 600)
    })
    if (!vp.mobile) {
      await state('edit-panels-scrolled', async () => {
        await page.locator('[data-testid="edit-page"] aside').evaluate((el) => { el.scrollTop = el.scrollHeight })
      })
      await state('edit-sections-open', async () => {
        // expand every collapsed panel section so their controls are audited too
        const closed = page.locator('[data-testid="edit-page"] aside [aria-expanded="false"]')
        for (let i = 0; i < 12 && (await closed.count()) > 0; i++) await closed.first().click({ force: true })
        await sleep(page, 500)
      })
      await state('edit-crop', async () => {
        await page.keyboard.press('r')
        await sleep(page, 500)
      })
      await state('edit-history', async () => {
        await page.getByTestId('history-toggle').click({ force: true })
        await sleep(page, 400)
      })
    } else {
      await state('edit-drawer', async () => {
        await page.getByTestId('drawer-toggle').click({ force: true })
        await sleep(page, 600)
      })
    }
    if (ids.burst) {
      await state('best-take', async () => {
        await go(`/s/${ids.sid}/besttake/${ids.burst}`)
        await page.getByTestId('besttake-page').waitFor({ timeout: 10000 })
        await sleep(page, 1500)
      })
    }
    await state('my-style', async () => {
      await go('/taste')
      await page.getByTestId('taste-page').waitFor({ timeout: 10000 })
    })
    await state('settings', async () => {
      await go('/settings')
      await page.getByTestId('settings-page').waitFor({ timeout: 10000 })
      await sleep(page, 600)
    })
    // every settings section (the nav lists them; owner-only ones are simply absent for other roles)
    const sections = await page.evaluate(() => [...document.querySelectorAll('[data-testid^="settings-nav-"]')].map((x) => x.getAttribute('data-testid')))
    for (const id of sections ?? []) {
      await state(id.replace('settings-nav-', 'settings-'), async () => {
        await page.getByTestId(id).click()
        await sleep(page, 500)
      })
    }
    await state('login', async () => {
      // /login redirects away unless the server reports LAN auth; stub the probe so the form renders
      await page.route('**/api/auth/me', (r) => r.fulfill({ json: { lan: true, role: null } }))
      await go('/login')
      await page.getByTestId('login-page').waitFor({ timeout: 10000 })
    })
    await ctx.close()
  }
}

await browser.close()
writeFileSync(`${OUT}/summary.json`, JSON.stringify({ summary, skipped, runtimeErrors }, null, 2))
console.log(`\n${summary.length} states audited (${viewports.join(', ')} x ${modes.join(', ')}), JSON in ${OUT}.`)
// rollup by rule: states hit, nodes, and a few sample selectors (all impacts, not only the failing ones)
const byRule = new Map()
for (const row of summary) {
  for (const v of row.violations) {
    const r = byRule.get(v.id) ?? { impact: v.impact, states: 0, nodes: 0 }
    r.states++
    r.nodes += v.nodes
    byRule.set(v.id, r)
  }
}
if (byRule.size) {
  console.log('\nby rule (states / nodes):')
  for (const [id, r] of [...byRule].sort((a, b) => a[1].impact.localeCompare(b[1].impact) || b[1].nodes - a[1].nodes))
    console.log(`  ${r.impact.padEnd(9)} ${id.padEnd(32)} ${String(r.states).padStart(3)} / ${r.nodes}`)
}
if (process.env.A11Y_DETAIL !== '0') {
  // sample selectors per rule so a failing run is actionable without opening the JSON files
  const seen = new Map()
  for (const f of readdirSync(OUT).filter((n) => n.endsWith('.json') && n !== 'summary.json')) {
    const doc = JSON.parse(readFileSync(`${OUT}/${f}`, 'utf8'))
    for (const v of doc.violations)
      for (const n of v.nodes) {
        const key = `${v.id}\u0000${n.target.join(' ').replace(/:nth-child\(\d+\)/g, '').slice(0, 150)}`
        const e = seen.get(key) ?? { id: v.id, impact: v.impact, sel: n.target.join(' ').replace(/:nth-child\(\d+\)/g, '').slice(0, 150), count: 0, states: new Set(), why: n.summary?.slice(0, 110) }
        e.count++
        e.states.add(doc.state)
        seen.set(key, e)
      }
  }
  const perRule = new Map()
  for (const e of [...seen.values()].sort((a, b) => b.states.size - a.states.size)) {
    const arr = perRule.get(e.id) ?? []
    if (arr.length < 8) arr.push(e)
    perRule.set(e.id, arr)
  }
  console.log('\ndetail (distinct selector: nodes, states):')
  for (const [id, arr] of perRule) {
    console.log(`  ${arr[0].impact} ${id}`)
    for (const e of arr) console.log(`    ${e.sel}  [${e.count}, ${[...e.states].slice(0, 4).join('/')}${e.states.size > 4 ? '/..' : ''}] ${e.why ?? ''}`)
  }
}
if (skipped.length) console.log(`skipped states:\n  ${skipped.join('\n  ')}`)
if (runtimeErrors.length) console.log(`page errors:\n  ${runtimeErrors.join('\n  ')}`)
if (failures.length) {
  const rules = [...new Set(failures.map((f) => f.id))]
  console.log(`A11Y SMOKE: FAIL (${failures.length} violation groups: ${rules.join(', ')})`)
  process.exit(1)
}
console.log('A11Y SMOKE: PASS')
