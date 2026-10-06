// End-to-end smoke test against a REAL `imagepicker serve` (not the mock).
// Usage: BASE_URL=http://127.0.0.1:7878 node scripts/e2e-smoke.mjs
// Requires at least one imported session with photos.
import { mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL ?? 'http://127.0.0.1:7878';
const OUT = fileURLToPath(new URL('../screenshots/e2e/', import.meta.url));
mkdirSync(OUT, { recursive: true });

const errors = [];
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
page.on('pageerror', (e) => errors.push(String(e)));

const api = async (path, init) => {
  const r = await fetch(BASE + path, init);
  if (!r.ok) throw new Error(`${path} -> ${r.status}`);
  return r.status === 204 ? null : r.json();
};

const { sessions } = await api('/api/sessions');
if (!sessions.length) throw new Error('no sessions: import a folder first');
const session = sessions[0];
console.log(`session ${session.id}: ${session.photo_count} photos`);

await page.goto(BASE);
await page.screenshot({ path: OUT + '01-home.png' });

await page.getByText(session.title).first().click();
const countLoaded = () =>
  [...document.images].filter((i) => i.src.includes('/api/thumb/') && i.complete && i.naturalWidth > 0).length;
const want = Math.min(session.photo_count, 12);
await page.waitForFunction(([fn, n]) => new Function(`return (${fn})()`)() >= n, [countLoaded.toString(), want], {
  timeout: 15000,
});
await page.waitForTimeout(400); // let the fade-in finish before the screenshot
const loaded = await page.evaluate(countLoaded);
console.log(`grid thumbnails rendered: ${loaded}`);
await page.screenshot({ path: OUT + '02-grid.png' });

// Rate the first photo 4 stars with the keyboard and check the server persisted it.
const { photos } = await api(`/api/photos?session_id=${session.id}&limit=1`);
const first = photos[0];
await page.locator(`img[src*="/api/thumb/${first.id}"]`).first().click();
await page.keyboard.press('4');
await page.waitForTimeout(500);
const after = (await api(`/api/photos/${first.id}`)).photo;
console.log(`photo ${first.id} user_rating after key "4": ${after.user_rating}`);

// Undo should restore the previous rating.
await page.keyboard.press('Control+z');
await page.waitForTimeout(500);
const undone = (await api(`/api/photos/${first.id}`)).photo;
console.log(`after Ctrl+Z: ${undone.user_rating}`);

await page.keyboard.press('e');
await page.waitForTimeout(1500);
await page.screenshot({ path: OUT + '03-loupe.png' });
await page.keyboard.press('c');
await page.waitForTimeout(1000);
await page.screenshot({ path: OUT + '04-compare.png' });

await browser.close();

const ok = loaded > 0 && after.user_rating === 4 && undone.user_rating === first.user_rating && errors.length === 0;
if (errors.length) console.log('console errors:\n  ' + errors.join('\n  '));
console.log(ok ? 'E2E SMOKE: PASS' : 'E2E SMOKE: FAIL');
process.exit(ok ? 0 : 1);
