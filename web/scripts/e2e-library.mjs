// Smoke the real analysed 1000-photo library in desktop and mobile layouts.
// BASE_URL and OUT_DIR should point to a disposable verification catalog/output.
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { chromium } from 'playwright';

const base = process.env.BASE_URL ?? 'http://127.0.0.1:17914';
const out = resolve(process.env.OUT_DIR ?? '../bench/data/verification-2026-10-14/browser');
mkdirSync(out, { recursive: true });
const api = async (path, body) => {
  const r = await fetch(base + '/api' + path, body === undefined ? {} : {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
  return r.status === 204 ? null : r.json();
};
await api('/onboarding/done', {});
const session = (await api('/sessions')).sessions.find((s) => s.photo_count === 1000);
if (!session) throw new Error('No 1000-photo session');
const sid = session.id;
const photos = (await api(`/photos?session_id=${sid}&limit=1000`)).photos;
const portrait = photos.find((p) => p.file_name === 'park-golden-a1.jpg') ?? photos[0];
const burst = (await api(`/groups?session_id=${sid}`)).scenes.flatMap((s) => s.bursts).find((b) => b.size > 2);
const browser = await chromium.launch();
const rows = [];
try {
  for (const [mode, viewport] of [['desktop', { width: 1440, height: 960 }], ['mobile', { width: 390, height: 844 }]]) {
    const context = await browser.newContext({ viewport, locale: 'en-US', isMobile: mode === 'mobile', hasTouch: mode === 'mobile' });
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    page.on('response', (r) => { if (r.status() >= 500) errors.push(`HTTP ${r.status()} ${r.url()}`); });
    for (const [name, path] of [
      ['home', '/'], ['library', `/s/${sid}`], ['people', `/s/${sid}/people`],
      ['edit', `/s/${sid}/edit/${portrait.id}`], ['besttake', `/s/${sid}/besttake/${burst.id}`],
      ['taste', '/taste'], ['settings', '/settings'],
    ]) {
      const previousErrors = errors.length;
      try {
        await page.goto(base + path);
        await page.waitForTimeout(900);
        if (name === 'library') {
          await page.locator('[data-id]').first().waitFor();
          await page.locator('[data-id]').first().click();
          await page.keyboard.press('ArrowRight');
          await page.keyboard.press('ArrowLeft');
          const loaded = await page.locator('img[src*="/api/thumb/"]').evaluateAll((imgs) => imgs.filter((i) => i.complete && i.naturalWidth > 0).length);
          if (!loaded) throw new Error('No decoded library thumbnails');
        }
        if (name === 'edit') await page.getByTestId('edit-canvas').locator('img').first().waitFor();
        const metrics = await page.evaluate(() => ({
          text: document.body.innerText.length,
          overflow: document.documentElement.scrollWidth > window.innerWidth + 2,
          images: [...document.images].filter((i) => i.complete && i.naturalWidth > 0).length,
        }));
        if (metrics.text < 80) throw new Error(`Empty page: ${JSON.stringify(metrics)}`);
        await page.screenshot({ path: `${out}/${mode}-${name}.png` });
        rows.push({ mode, page: name, status: errors.length === previousErrors && !metrics.overflow ? 'passed' : 'failed', metrics, errors: errors.slice(previousErrors) });
      } catch (e) {
        rows.push({ mode, page: name, status: 'failed', error: String(e) });
      }
      console.log(JSON.stringify(rows.at(-1)));
    }
    await context.close();
  }
} finally {
  await browser.close();
  writeFileSync(`${out}/results.json`, JSON.stringify(rows, null, 2));
}
if (rows.some((r) => r.status !== 'passed')) process.exitCode = 1;
