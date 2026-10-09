// Real-server end-to-end test (no mock): `imagepicker serve` on a temp data dir with the fake AI
// worker, synthetic photos, driven through the built web UI with Playwright.
//
//   cd web && pnpm build
//   node scripts/e2e-real.mjs --bin <imagepicker> --worker <ip-fake-worker> [--shots <dir>] [--keep]
//
// Flow: first-run dialog -> import a folder -> grid -> rate / flag / undo -> analysis run (fake
// worker, synthetic results) -> groups and stacks -> edit (exposure slider, preset) -> before/after
// and split compare -> export -> verify the exported file on disk (JPEG, brighter than the source).
// Dev mode only: no LAN, no password. Any console error, page error or HTTP 5xx fails the run.
import { spawn, execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const HERE = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : 'true']);
    return acc;
  }, []),
);
const exe = process.platform === 'win32' ? '.exe' : '';
const targetDir = resolve(HERE, '../../', process.env.CARGO_TARGET_DIR ?? 'target', 'release');
const BIN = resolve(args.bin ?? join(targetDir, 'imagepicker' + exe));
const WORKER = resolve(args.worker ?? join(targetDir, 'ip-fake-worker' + exe));
const WEB = resolve(args['web-dir'] ?? resolve(HERE, '../dist'));
const SHOTS = args.shots ? resolve(args.shots) : null;
const PORT = Number(args.port ?? 18900 + Math.floor(Math.random() * 100));
const BASE = `http://127.0.0.1:${PORT}`;
const PHOTOS_N = Number(args.photos ?? 40);

for (const [what, p] of [['imagepicker binary', BIN], ['fake worker binary', WORKER], ['built web UI (pnpm build)', join(WEB, 'index.html')]]) {
  if (!existsSync(p)) throw new Error(`${what} not found: ${p}`);
}

const work = mkdtempSync(join(tmpdir(), 'ip-e2e-real-'));
const dataDir = join(work, 'data');
const photosDir = join(work, 'photos');
const exportDir = join(work, 'export');
if (SHOTS) mkdirSync(SHOTS, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log(...a);

// ------------------------------------------------------------------ data + server
execFileSync(BIN, ['bench', 'gen', photosDir, '--count', String(PHOTOS_N), '--megapixels', '4'], { stdio: ['ignore', 'ignore', 'inherit'] });
const server = spawn(BIN, ['serve', '--port', String(PORT), '--data-dir', dataDir, '--web-dir', WEB], {
  stdio: ['ignore', 'ignore', 'pipe'],
  env: { ...process.env, IMAGEPICKER_WORKER_CMD: WORKER, IP_FAKE_ANALYSIS: '1', IMAGEPICKER_RENDER: 'cpu', RUST_LOG: 'warn' },
});
let serverErr = '';
server.stderr.on('data', (d) => (serverErr += d));
const cleanup = async () => {
  server.kill();
  await new Promise((r) => (server.exitCode !== null ? r() : server.once('exit', r)));
  await sleep(200);
  if (!args.keep) rmSync(work, { recursive: true, force: true });
};
for (let i = 0; ; i++) {
  try {
    if ((await fetch(BASE + '/api/health')).ok) break;
  } catch {
    /* starting */
  }
  if (server.exitCode !== null) throw new Error('server exited: ' + serverErr);
  if (i > 400) throw new Error('server did not start');
  await sleep(25);
}

const api = async (path, init) => {
  const r = await fetch(BASE + path, init);
  if (!r.ok) throw new Error(`${path} -> ${r.status} ${await r.text()}`);
  return r.status === 204 ? null : r.json();
};
const until = async (what, fn, ms = 20000) => {
  const t = Date.now();
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() - t > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
};

// the server-side settings are the source of truth for the UI language
await api('/api/settings', { method: 'PATCH', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ language: 'en' }) });

// ------------------------------------------------------------------ browser
const problems = [];
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'en-US' });
await ctx.addInitScript(() => {
  try {
    localStorage.setItem('imagepicker.ui', JSON.stringify({ state: { lang: 'en' }, version: 0 }));
  } catch {
    /* ignore */
  }
});
const page = await ctx.newPage();
page.on('console', (m) => m.type() === 'error' && problems.push(`console: ${m.text()}`));
page.on('pageerror', (e) => problems.push(`pageerror: ${e}`));
page.on('response', (r) => r.status() >= 500 && problems.push(`HTTP ${r.status()} ${r.url()}`));
page.on('requestfailed', (r) => {
  const f = r.failure()?.errorText ?? '';
  if (!/ERR_ABORTED/.test(f)) problems.push(`request failed: ${r.url()} ${f}`);
});
const shot = async (name) => SHOTS && (await page.screenshot({ path: join(SHOTS, name + '.png') }));

const results = [];
const step = async (name, fn) => {
  const t = Date.now();
  try {
    await fn();
    results.push({ name, ok: true });
    log(`PASS  ${name} (${Date.now() - t} ms)`);
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.message ?? e) });
    log(`FAIL  ${name}: ${e?.message ?? e}`);
    await shot('fail-' + results.length).catch(() => undefined);
    throw e;
  }
};
const expect = (cond, msg) => {
  if (!cond) throw new Error(msg);
};
const gridImagesLoaded = () =>
  page.evaluate(() => [...document.querySelectorAll('[data-testid="grid"] img')].filter((i) => i.complete && i.naturalWidth > 0).length);

let session;
let photos = [];
let editedId;
let failed = null;

try {
  await step('first-run dialog: choose basic mode', async () => {
    await page.goto(BASE);
    await page.getByTestId('onboarding-basic').click({ timeout: 15000 });
    await page.getByTestId('onboarding-basic').waitFor({ state: 'detached' });
  });

  await step('import a folder through the folder browser', async () => {
    await page.getByRole('button', { name: 'Choose folder' }).click();
    const path = page.getByLabel('Path', { exact: true });
    await path.fill(photosDir);
    await page.getByRole('button', { name: 'Go', exact: true }).click();
    await page.getByRole('button', { name: 'Import this folder' }).click();
    await page.waitForURL(/\/s\/\d+/, { timeout: 15000 });
    session = Number(page.url().match(/\/s\/(\d+)/)[1]);
    await until('session ready', async () => {
      const s = (await api(`/api/sessions/${session}`)).session;
      return s.import_state === 'ready' && s.photo_count === PHOTOS_N;
    });
  });

  await step('grid shows every photo with thumbnails', async () => {
    // the coach marks of the first session would cover the grid
    const skip = page.getByTestId('coach-skip');
    if (await skip.isVisible().catch(() => false)) await skip.click();
    await page.getByTestId('grid').waitFor();
    await until('thumbnails painted', async () => (await gridImagesLoaded()) >= 8);
    const total = (await api(`/api/photos?session_id=${session}&limit=500`)).total;
    expect(total === PHOTOS_N, `total ${total} != ${PHOTOS_N}`);
    const showing = await page.getByTestId('showing').innerText();
    expect(showing.includes(String(PHOTOS_N)), `status line "${showing}"`);
    photos = (await api(`/api/photos?session_id=${session}&limit=500`)).photos;
    expect(photos.every((p) => p.width === 2310 || p.width > 0), 'photos have sizes');
    await shot('01-grid');
  });

  const rating = async (id) => (await api(`/api/photos/${id}`)).photo.user_rating;
  const flag = async (id) => (await api(`/api/photos/${id}`)).photo.flag;
  await step('rate, flag and undo with the keyboard', async () => {
    const first = photos[0];
    await page.locator(`[data-id="${first.id}"]`).click();
    await page.keyboard.press('4');
    await until('rating 4', async () => (await rating(first.id)) === 4);
    await page.keyboard.press('p');
    await until('picked', async () => (await flag(first.id)) === 1);
    await page.keyboard.press('x');
    await until('rejected', async () => (await flag(first.id)) === -1);
    await page.keyboard.press('Control+z');
    await until('undo reject -> picked', async () => (await flag(first.id)) === 1);
    await page.keyboard.press('Control+z');
    await until('undo pick -> unflagged', async () => (await flag(first.id)) === 0);
    await page.keyboard.press('Control+z');
    await until('undo rating -> none', async () => (await rating(first.id)) === null);
    await page.keyboard.press('Control+Shift+z');
    await until('redo rating', async () => (await rating(first.id)) === 4);
    // multi-select: shift-click a range and rate it
    await page.locator(`[data-id="${photos[3].id}"]`).click({ modifiers: ['Shift'] });
    await page.keyboard.press('2');
    await until('range rated', async () => (await rating(photos[2].id)) === 2 && (await rating(photos[3].id)) === 2);
    await shot('02-rated');
  });

  await step('analysis run with the fake worker', async () => {
    await page.getByTestId('analyze-button').click();
    await page.getByRole('radio', { name: /Fast/ }).check();
    expect((await api('/api/settings')).faces.consented === false, 'face recognition needs an explicit consent');
    await page.getByTestId('analyze-start').click();
    // the first analysis asks about face recognition first (docs/02 §8); agreeing starts it
    await page.getByTestId('face-consent-agree').click();
    await until('analysis done', async () => {
      const s = await api(`/api/analysis/status?session_id=${session}`);
      expect(s.state !== 'failed', `analysis failed: ${s.error}`);
      return s.state === 'done';
    }, 60000);
    await until('AI ratings in the catalog', async () => {
      const p = (await api(`/api/photos?session_id=${session}&limit=500`)).photos;
      return p.every((x) => x.analyzed && x.ai_rating !== null);
    });
    // regression: analysis progress used to surface as an "Export finished" toast
    expect((await page.getByText(/Export (finished|running)/).count()) === 0, 'analysis must not show an export toast');
    await until('AI badges in the grid', async () => (await page.getByTestId('ai-badge').count()) > 0);
    await shot('03-analysed');
  });

  await step('groups and stacks', async () => {
    const g = await api(`/api/groups?session_id=${session}`);
    const bursts = g.scenes.flatMap((s) => s.bursts);
    const multi = bursts.filter((b) => b.size >= 2);
    expect(multi.length > 0, `no multi-photo bursts among ${bursts.length}`);
    expect(multi.every((b) => b.best_photo_id !== null), 'every burst has a best shot');
    await until('stack badges', async () => (await page.getByTestId('stack-badge').count()) > 0);
    const badge = page.getByTestId('stack-badge').first();
    const before = await page.locator('[data-testid="grid"] [data-id]').count();
    await badge.click();
    await until('stack expanded', async () => (await page.getByTestId('stack-badge').first().getAttribute('aria-expanded')) === 'true');
    const after = await page.locator('[data-testid="grid"] [data-id]').count();
    expect(after > before, `expanding a stack should show more cells (${before} -> ${after})`);
    await page.getByTestId('stack-badge').first().click();
    await until('stack collapsed', async () => (await page.getByTestId('stack-badge').first().getAttribute('aria-expanded')) === 'false');
    expect((await page.getByTestId('scene-header').count()) > 0, 'scene headers are shown');
    await shot('04-stacks');
  });

  await step('open the edit page and move the exposure slider', async () => {
    photos = (await api(`/api/photos?session_id=${session}&limit=500`)).photos;
    // a visible, non-stacked cell near the top of the grid
    const cell = page.locator('[data-testid="grid"] [role="gridcell"]:not(:has([data-testid="stack-badge"]))').first();
    await cell.waitFor();
    editedId = Number(await cell.getAttribute('data-id'));
    await page.locator(`[data-id="${editedId}"]`).click();
    await page.keyboard.press('d');
    await page.getByTestId('edit-page').waitFor();
    await until('edit canvas', async () => (await page.getByTestId('edit-canvas').locator('img').count()) > 0);
    const input = page.getByTestId('adj-exposure').locator('input.lr-num');
    await input.click();
    await input.fill('1');
    await input.press('Enter');
    await until('stack saved with exposure', async () => {
      const e = await api(`/api/edits/${editedId}`);
      const json = JSON.stringify(e);
      return /"exposure":\s*1(\.0)?[,}]/.test(json);
    });
    await shot('05-edit-exposure');
  });

  await step('apply a preset', async () => {
    const presets = (await api('/api/presets')).presets;
    expect(presets.length > 0, 'built-in presets exist');
    const before = JSON.stringify(await api(`/api/edits/${editedId}`));
    await page.getByTestId(`preset-${presets[0].id}`).click();
    await until('stack changed by preset', async () => JSON.stringify(await api(`/api/edits/${editedId}`)) !== before);
    // exposure is kept or replaced; either way the edit still renders
    await until('canvas image', async () => (await page.getByTestId('edit-canvas').locator('img').count()) > 0);
    await shot('06-edit-preset');
    // put the exposure back to +1 so the export check below has a clear brightness difference
    const input = page.getByTestId('adj-exposure').locator('input.lr-num');
    await input.click();
    await input.fill('1.5');
    await input.press('Enter');
    await until('exposure 1.5 saved', async () => /"exposure":\s*1\.5[,}]/.test(JSON.stringify(await api(`/api/edits/${editedId}`))));
  });

  await step('before / after and split compare', async () => {
    await page.getByTestId('compare-toggle').click();
    await page.getByTestId('original-chip').waitFor();
    await shot('07-before');
    await page.getByTestId('compare-toggle').click();
    await page.getByTestId('original-chip').waitFor({ state: 'detached' });
    await page.getByTestId('compare-split').click();
    await page.getByTestId('split-overlay').waitFor();
    await shot('08-split');
    await page.getByTestId('compare-split').click();
    await page.getByTestId('split-overlay').waitFor({ state: 'detached' });
    await until('save state settled', async () => !/saving/i.test((await page.getByTestId('save-state').innerText().catch(() => '')) || ''));
  });

  await step('back to the grid shows the edit badge', async () => {
    await page.getByTestId('edit-back').click();
    await page.getByTestId('grid').waitFor();
    await until('edit badge', async () => (await page.getByTestId('edit-badge').count()) > 0);
    const p = (await api(`/api/photos/${editedId}`)).photo;
    expect(p.has_edits === true, 'photo has_edits');
  });

  await step('export the edited photo and verify the file', async () => {
    mkdirSync(exportDir, { recursive: true });
    await page.locator(`[data-id="${editedId}"]`).waitFor();
    await page.locator(`[data-id="${editedId}"]`).click();
    await page.keyboard.press('Control+e');
    await page.getByLabel('Destination folder').fill(exportDir);
    await page.getByRole('radio', { name: /^Selected/ }).check();
    await page.getByRole('button', { name: /^Export 1/ }).click();
    const name = photos.find((p) => p.id === editedId).file_name.replace(/\.[^.]+$/, '');
    const file = await until('exported file on disk', async () => readdirSync(exportDir).find((f) => f.startsWith(name) && f.toLowerCase().endsWith('.jpg')), 60000);
    const out = join(exportDir, file);
    await until('export finished writing', async () => statSync(out).size > 10_000);
    const bytes = readFileSync(out);
    expect(bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[bytes.length - 2] === 0xff && bytes[bytes.length - 1] === 0xd9, 'exported file is a complete JPEG');
    // brightness: +1.5 EV must be clearly brighter than the untouched original
    const src = readFileSync(join(photosDir, photos.find((p) => p.id === editedId).file_name));
    const meanLuma = (buf) =>
      page.evaluate(async (b64) => {
        const img = new Image();
        img.src = 'data:image/jpeg;base64,' + b64;
        await img.decode();
        const c = document.createElement('canvas');
        c.width = 64;
        c.height = 64;
        const g = c.getContext('2d');
        g.drawImage(img, 0, 0, 64, 64);
        const d = g.getImageData(0, 0, 64, 64).data;
        let s = 0;
        for (let i = 0; i < d.length; i += 4) s += 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2];
        return s / (d.length / 4);
      }, buf.toString('base64'));
    const [lo, hi] = [await meanLuma(src), await meanLuma(bytes)];
    log(`      mean luma original ${lo.toFixed(1)} -> exported ${hi.toFixed(1)}`);
    expect(hi > lo * 1.15 || (lo > 200 && hi >= lo), `exported image is not brighter (${lo.toFixed(1)} -> ${hi.toFixed(1)})`);
    expect(file !== photos.find((p) => p.id === editedId).file_name || bytes.length !== src.length, 'export differs from the original file');
    await shot('09-exported');
  });
} catch (e) {
  failed = e;
}

await sleep(300);
await browser.close();
await cleanup();
if (!failed && problems.length === 0) {
  log(`\nE2E REAL: PASS (${results.length} steps, 0 console errors)`);
  process.exit(0);
}
if (problems.length) log('\nproblems:\n  ' + problems.join('\n  '));
if (serverErr.trim()) log('\nserver stderr (tail):\n' + serverErr.split('\n').slice(-15).join('\n'));
log('\nE2E REAL: FAIL');
process.exit(1);
