// UX audit capture: walk every module and its main states on an analysed library.
// BASE_URL and OUT_DIR point to a disposable verification catalog/output. Each step is
// independent: a failure is recorded and the walk continues.
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { chromium } from 'playwright';

const base = process.env.BASE_URL ?? 'http://127.0.0.1:17915';
const out = resolve(process.env.OUT_DIR ?? '../bench/data/verification-2026-10-29/ux');
const locale = process.env.UX_LOCALE ?? 'zh-CN';
mkdirSync(out, { recursive: true });
const api = async (path, body) => {
  const r = await fetch(base + '/api' + path, body === undefined ? {} : {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
  return r.status === 204 ? null : r.json();
};
await api('/onboarding/done', {}).catch(() => {});
const sessions = (await api('/sessions')).sessions;
const session = sessions.sort((a, b) => b.photo_count - a.photo_count)[0];
const sid = session.id;
const photos = (await api(`/photos?session_id=${sid}&limit=1000`)).photos;
const portrait = photos.find((p) => p.file_name === 'park-golden-a1.jpg') ?? photos[0];
const groups = await api(`/groups?session_id=${sid}`);
const burst = groups.scenes.flatMap((s) => s.bursts).find((b) => b.size > 2);

const browser = await chromium.launch();
const rows = [];
const timings = [];
async function step(page, name, fn, errors) {
  const before = errors.length;
  const t0 = Date.now();
  try {
    await fn();
    await page.waitForTimeout(500);
    const m = await page.evaluate(() => ({
      overflowX: document.documentElement.scrollWidth > window.innerWidth + 2,
      text: document.body.innerText.length,
      smallTargets: [...document.querySelectorAll('button,a,[role=button],input,select')]
        .filter((e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && (r.width < 24 || r.height < 24); }).length,
      unlabeled: [...document.querySelectorAll('button,[role=button]')]
        .filter((e) => e.getBoundingClientRect().width > 0 && !(e.innerText.trim() || e.getAttribute('aria-label') || e.getAttribute('title'))).length,
    }));
    await page.screenshot({ path: `${out}/${name}.png` });
    rows.push({ name, ok: errors.length === before, ms: Date.now() - t0, ...m, errors: errors.slice(before) });
  } catch (e) {
    await page.screenshot({ path: `${out}/${name}-FAILED.png` }).catch(() => {});
    rows.push({ name, ok: false, ms: Date.now() - t0, error: String(e).slice(0, 400), errors: errors.slice(before) });
  }
  console.log(JSON.stringify(rows.at(-1)));
}
const setTheme = (page, t) => page.evaluate((t) => { document.documentElement.dataset.theme = t; }, t);

try {
  for (const [mode, viewport] of [['desktop', { width: 1440, height: 900 }], ['laptop', { width: 1280, height: 720 }], ['mobile', { width: 390, height: 844 }]]) {
    const context = await browser.newContext({ viewport, locale, isMobile: mode === 'mobile', hasTouch: mode === 'mobile' });
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', (e) => errors.push(String(e).slice(0, 300)));
    page.on('console', (m) => { if (m.type() === 'error') errors.push('console: ' + m.text().slice(0, 300)); });
    page.on('response', (r) => { if (r.status() >= 500) errors.push(`HTTP ${r.status()} ${r.url()}`); });
    const go = async (p) => { await page.goto(base + p); await page.waitForLoadState('networkidle').catch(() => {}); };
    const s = (n) => `${mode}-${n}`;

    await step(page, s('01-home'), () => go('/'), errors);
    await step(page, s('02-library'), async () => {
      const t0 = Date.now();
      await go(`/s/${sid}`);
      await page.locator('[data-id]').first().waitFor({ timeout: 20000 });
      timings.push({ mode, what: 'library-first-tile-ms', ms: Date.now() - t0 });
    }, errors);
    if (mode === 'desktop') {
      // keyboard: Tab until the grid holds focus, then ArrowDown twice; the active cell should show a focus ring
      await step(page, s('02b-keyboard'), async () => {
        // Tab on <body> is the inspector shortcut (keymap.tabIsShortcut): start traversal from a toolbar control
        await page.getByTestId('grouped-toggle').focus()
        for (let i = 0; i < 60; i++) {
          await page.keyboard.press('Tab');
          if (await page.evaluate(() => document.activeElement?.getAttribute('data-testid') === 'grid')) break;
        }
        const focused = await page.evaluate(() => document.activeElement?.getAttribute('data-testid'));
        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('ArrowDown');
        const st = await page.evaluate(() => {
          const g = document.querySelector('[data-testid=grid]');
          const id = g?.getAttribute('aria-activedescendant');
          const el = id ? document.getElementById(id) : null;
          return { focused: document.activeElement === g, activedescendant: id, resolves: !!el, outline: el ? getComputedStyle(el).outlineStyle : null };
        });
        console.log(JSON.stringify({ keyboard: { tabbedTo: focused, ...st } }));
        if (!st.focused || !st.resolves) throw new Error('grid keyboard focus check failed: ' + JSON.stringify(st));
      }, errors);
      await page.locator('[data-id]').first().click();
    }
    if (mode !== 'mobile') {
      await step(page, s('03-library-light'), () => setTheme(page, 'light'), errors);
      await setTheme(page, 'dark');
      await step(page, s('04-selected-inspector'), async () => { await page.locator('[data-id]').nth(2).click(); }, errors);
      await step(page, s('05-loupe'), async () => { await page.keyboard.press('e'); await page.waitForTimeout(800); }, errors);
      await step(page, s('06-loupe-faces'), async () => { await page.keyboard.press('Shift+F'); }, errors);
      await page.keyboard.press('Shift+F');
      await step(page, s('07-group'), async () => { await page.keyboard.press('b'); await page.waitForTimeout(800); }, errors);
      await step(page, s('08-compare'), async () => { await page.keyboard.press('c'); await page.waitForTimeout(800); }, errors);
      await step(page, s('09-grid-back'), async () => { await page.keyboard.press('g'); }, errors);
      await step(page, s('10-palette'), async () => { await page.keyboard.press('Control+k'); await page.getByTestId('command-palette').waitFor(); }, errors);
      await page.keyboard.press('Escape');
      await step(page, s('11-help'), async () => { await page.keyboard.press('Shift+Slash'); await page.getByTestId('help-overlay').waitFor({ timeout: 3000 }); }, errors);
      await page.keyboard.press('Escape');
      await step(page, s('12-issues-filter'), async () => { await page.getByTestId('issues-filter').click(); }, errors);
      await page.keyboard.press('Escape');
      await step(page, s('13-person-filter'), async () => { await page.getByTestId('person-filter-button').click(); }, errors);
      await page.keyboard.press('Escape');
      await step(page, s('14-timeline'), async () => { await page.getByTestId('timeline-toggle').click(); await page.waitForTimeout(800); }, errors);
      await page.getByTestId('timeline-close').click().catch(() => {});
      await step(page, s('15-grouped'), async () => { await page.getByTestId('grouped-toggle').click(); await page.waitForTimeout(800); }, errors);
      await page.getByTestId('grouped-toggle').click().catch(() => {});
      await step(page, s('16-photo-menu'), async () => { await page.locator('[data-id]').nth(1).click({ button: 'right' }); await page.getByTestId('photo-menu').waitFor({ timeout: 3000 }); }, errors);
      await page.keyboard.press('Escape');
      await step(page, s('17-export'), async () => { await page.locator('[data-id]').nth(1).click(); await page.keyboard.press('Control+e'); await page.waitForTimeout(800); }, errors);
      await page.keyboard.press('Escape');
      await step(page, s('18-assistant'), async () => { await page.keyboard.press('Control+j'); await page.waitForTimeout(800); }, errors);
      await page.keyboard.press('Control+j');
      await step(page, s('19-search'), async () => {
        await page.getByTestId('search-input').fill('一只猫');
        await page.keyboard.press('Enter');
        await page.waitForTimeout(2500);
      }, errors);
      await page.getByTestId('search-clear').click().catch(() => {});
      await step(page, s('19b-search-absent'), async () => {
        await page.getByTestId('search-input').fill('长颈鹿');
        await page.keyboard.press('Enter');
        await page.waitForTimeout(2500);
      }, errors);
      await page.getByTestId('search-clear').click().catch(() => {});
    } else {
      await step(page, s('03-cull'), async () => { await page.locator('[data-id]').first().click(); await page.waitForTimeout(900); }, errors);
      await step(page, s('04-cull-swipe'), async () => {
        const vp = page.viewportSize();
        await page.touchscreen.tap(vp.width / 2, vp.height / 2);
      }, errors);
      await page.goBack().catch(() => {});
    }
    await step(page, s('20-people'), async () => { await go(`/s/${sid}/people`); await page.waitForTimeout(800); }, errors);
    await step(page, s('21-person'), async () => { await page.getByTestId('person-card').first().click(); await page.waitForTimeout(900); }, errors);
    await step(page, s('22-edit'), async () => {
      const t0 = Date.now();
      await go(`/s/${sid}/edit/${portrait.id}`);
      await page.getByTestId('edit-canvas').locator('img').first().waitFor({ timeout: 30000 });
      timings.push({ mode, what: 'edit-first-image-ms', ms: Date.now() - t0 });
    }, errors);
    if (mode !== 'mobile') {
      for (const [i, label] of [[23, 'scroll-mid'], [24, 'scroll-end']].entries()) {
        await step(page, s(`${label[0]}-edit-${label[1]}`), async () => {
          await page.getByTestId('edit-page').locator('aside, [class*=panel]').last().evaluate((el, i) => { el.scrollTop = i === 0 ? el.scrollHeight / 2 : el.scrollHeight; }, i);
        }, errors);
      }
      await step(page, s('25-edit-light'), () => setTheme(page, 'light'), errors);
      await setTheme(page, 'dark');
      await step(page, s('26-edit-crop'), async () => { await page.keyboard.press('r'); }, errors);
      await page.keyboard.press('r');
    }
    if (burst) await step(page, s('27-besttake'), async () => { await go(`/s/${sid}/besttake/${burst.id}`); await page.waitForTimeout(1500); }, errors);
    await step(page, s('28-taste'), () => go('/taste'), errors);
    await step(page, s('29-settings'), () => go('/settings'), errors);
    await step(page, s('30-settings-end'), () => page.evaluate(() => window.scrollTo(0, document.body.scrollHeight)), errors);
    await context.close();
  }
} finally {
  await browser.close();
  writeFileSync(`${out}/results.json`, JSON.stringify({ sid, photos: photos.length, rows, timings }, null, 2));
}
