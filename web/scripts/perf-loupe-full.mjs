// Loupe at 100% against a REAL `imagepicker serve` (no mock), on large photos:
//  * next / previous at fit (p95 must stay < 50 ms: only the preloaded 2048 preview is used),
//  * time until a photo shows its own pixels after Space (fit -> 100%): nothing cached, cached by
//    the server only (browser cold), cached by the browser,
//  * pixel check: at 100% the screen shows the full-resolution image 1:1 (screenshot vs. image).
//
//   cd web && pnpm build
//   imagepicker bench gen <dir> --count 40 --megapixels 24
//   node scripts/perf-loupe-full.mjs --bin <imagepicker> --photos <dir> [--channel msedge] [--dpr 1.5] [--out result.json] [--shots <dir>]
import { spawn } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
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
const BIN = resolve(args.bin ?? resolve(HERE, '../../', process.env.CARGO_TARGET_DIR ?? 'target', 'release', 'imagepicker' + exe));
const PHOTOS = resolve(args.photos ?? '');
const WEB = resolve(args['web-dir'] ?? resolve(HERE, '../dist'));
const OUT = args.out ? resolve(args.out) : null;
const DPR = Number(args.dpr ?? 1);
const PORT = Number(args.port ?? 18700 + Math.floor(Math.random() * 100));
const BASE = `http://127.0.0.1:${PORT}`;
if (!args.photos) throw new Error('--photos <dir> is required (see the header of this file)');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.error('[perf-loupe-full]', ...a);
const stats = (xs) => {
  if (!xs.length) return { n: 0 };
  const s = [...xs].sort((a, b) => a - b);
  const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  const r = (x) => +x.toFixed(1);
  return { n: s.length, p50: r(q(0.5)), p95: r(q(0.95)), max: r(s[s.length - 1]) };
};

// the photos are copied into a temp folder: the server only imports from allowed roots (home, ...)
const work = mkdtempSync(join(tmpdir(), 'ip-loupe-full-'));
const photosDir = join(work, 'photos');
cpSync(PHOTOS, photosDir, { recursive: true });
const server = spawn(BIN, ['serve', '--port', String(PORT), '--data-dir', join(work, 'data'), '--web-dir', WEB], {
  stdio: ['ignore', 'ignore', 'inherit'],
  env: { ...process.env, RUST_LOG: 'warn' },
});
const stop = async () => {
  server.kill();
  await new Promise((r) => (server.exitCode !== null ? r() : server.once('exit', r)));
  await sleep(300);
  rmSync(work, { recursive: true, force: true });
};
const api = async (path, init) => {
  const r = await fetch(BASE + path, init);
  if (!r.ok) throw new Error(`${path} -> ${r.status} ${await r.text()}`);
  return r.status === 204 ? null : r.json();
};

const result = { dpr: DPR, viewport: '1440x900' };
let browser;
try {
  for (let i = 0; ; i++) {
    try {
      if ((await fetch(BASE + '/api/health')).ok) break;
    } catch {
      /* starting */
    }
    if (server.exitCode !== null) throw new Error('server exited');
    if (i > 400) throw new Error('server did not start');
    await sleep(25);
  }
  await fetch(BASE + '/api/onboarding/done', { method: 'POST' }).catch(() => undefined);
  await api('/api/settings', { method: 'PATCH', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ language: 'en' }) });
  const { session } = await api('/api/import', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ path: photosDir, recursive: false }) });
  for (let i = 0; ; i++) {
    const s = (await api(`/api/sessions/${session.id}`)).session;
    if (s.import_state === 'ready') break;
    if (i > 1200) throw new Error('import did not finish');
    await sleep(100);
  }
  const { photos } = await api(`/api/photos?session_id=${session.id}&limit=500&sort=name`);
  result.photos = photos.length;
  result.sizes = [...new Set(photos.map((p) => `${p.width}x${p.height}`))];
  log(`session ${session.id}: ${photos.length} photos ${result.sizes.join(', ')}`);

  // ------------------------------------------------------------ server only: GET /api/full
  const timed = async (url) => {
    const t = performance.now();
    const r = await fetch(BASE + url);
    const b = await r.arrayBuffer();
    if (!r.ok) throw new Error(`${url} -> ${r.status}`);
    return { ms: performance.now() - t, bytes: b.byteLength };
  };
  // the first photos: the UI flips past them at fit and later uses them as "server-cached"
  const tail = photos.slice(0, 6);
  const cold = [];
  const warm = [];
  const p4096 = [];
  for (const p of tail) {
    p4096.push((await timed(`/api/preview/${p.id}?s=4096`)).ms);
    const c = await timed(`/api/full/${p.id}`);
    cold.push(c.ms);
    warm.push((await timed(`/api/full/${p.id}`)).ms);
    result.full_bytes = c.bytes;
  }
  result.server = { preview_4096_cold_ms: stats(p4096), full_cold_ms: stats(cold), full_cached_ms: stats(warm) };
  log('server', JSON.stringify(result.server));

  // ------------------------------------------------------------ browser
  browser = await chromium.launch(args.channel ? { channel: args.channel } : {});
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: DPR, locale: 'en-US' });
  await ctx.addInitScript(() => {
    try {
      localStorage.setItem('imagepicker.ui', JSON.stringify({ state: { lang: 'en' }, version: 0 }));
    } catch {
      /* ignore */
    }
  });
  const page = await ctx.newPage();
  const errors = [];
  let fullRequests = 0;
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('request', (r) => r.url().includes('/api/full/') && fullRequests++);
  await page.goto(`${BASE}/s/${session.id}`);
  const skip = page.getByTestId('coach-skip');
  await page.waitForSelector('[data-testid="grid"] img', { timeout: 60000 });
  if (await skip.isVisible().catch(() => false)) await skip.click();
  await page.locator('[data-testid="grid"] [data-id]').first().dblclick();
  await page.waitForSelector('[data-testid="zoom-pane"]');
  await page.waitForFunction(() => [...document.querySelectorAll('[data-testid="zoom-pane"] img')].some((i) => i.src.includes('/preview/') && i.complete && i.naturalWidth > 0), null, { timeout: 30000 });
  await sleep(1500);

  // next photo at fit (same probe as perf-real.mjs)
  await page.evaluate(() => {
    const probe = {
      results: [],
      arm() {
        this.t0 = performance.now();
        this.first = null;
        const startSrc = new Set([...document.querySelectorAll('[data-testid="zoom-pane"] img')].map((i) => i.src));
        const poll = () => {
          const imgs = [...document.querySelectorAll('[data-testid="zoom-pane"] img')];
          const fresh = imgs.filter((i) => !startSrc.has(i.src));
          const op = (i) => parseFloat(getComputedStyle(i).opacity);
          const now = performance.now();
          if (this.first === null && fresh.some((i) => i.complete && i.naturalWidth > 0 && op(i) > 0.05)) this.first = now - this.t0;
          if (fresh.some((i) => i.src.includes('/preview/') && i.complete && i.naturalWidth > 0 && op(i) > 0.99)) {
            this.results.push({ first: this.first, settled: now - this.t0 });
            return;
          }
          if (now - this.t0 > 5000) {
            this.results.push({ first: this.first, settled: null });
            return;
          }
          requestAnimationFrame(poll);
        };
        requestAnimationFrame(poll);
      },
    };
    window.__probe = probe;
    window.addEventListener('keydown', (e) => e.key === 'ArrowRight' && probe.arm(), true);
  });
  const steps = Math.min(Number(args['loupe-steps'] ?? 24), photos.length - 8);
  for (let i = 0; i < steps; i++) {
    await page.keyboard.press('ArrowRight');
    await sleep(i < steps / 2 ? 400 : 200);
  }
  await sleep(800);
  const lr = await page.evaluate(() => window.__probe.results);
  result.next_photo = {
    first_pixels_ms: stats(lr.map((r) => r.first).filter((x) => x !== null)),
    settled_ms: stats(lr.map((r) => r.settled).filter((x) => x !== null)),
    not_settled: lr.filter((r) => r.settled === null).length,
    full_requests_while_flipping: fullRequests,
  };
  log('next photo', JSON.stringify(result.next_photo));

  // fit -> 100% (Space): time until the full-resolution layer is shown, from the key press
  await page.evaluate(() => window.addEventListener('keydown', (e) => e.key === ' ' && (window.__spaceT0 = performance.now()), true));
  const toFull = async () => {
    await page.evaluate(() => (window.__spaceT0 = null));
    const done = page.evaluate(
      () =>
        new Promise((resolve) => {
          let t0 = null;
          let first = null;
          let hi = null;
          const op = (i) => parseFloat(getComputedStyle(i).opacity);
          const poll = () => {
            if (t0 === null) {
              t0 = window.__spaceT0;
              requestAnimationFrame(poll);
              return;
            }
            const pane = document.querySelector('[data-testid="zoom-pane"]');
            const imgs = [...pane.querySelectorAll('img')];
            const full = imgs.find((i) => i.src.includes('/full/'));
            const h = imgs.find((i) => i.src.includes('s=4096'));
            const now = performance.now() - t0;
            if (hi === null && h && h.complete && h.naturalWidth > 0 && op(h) > 0.05) hi = now;
            if (first === null && full && full.complete && full.naturalWidth > 0 && op(full) > 0.05) first = now;
            if (full && full.complete && full.naturalWidth > 0 && op(full) > 0.99) {
              // drawn: decoded, and two frames later
              const done = () =>
                requestAnimationFrame(() =>
                  requestAnimationFrame(() =>
                    resolve({ preview4096: hi, first, opaque: now, shown: performance.now() - t0, natural: [full.naturalWidth, full.naturalHeight], badge: pane.querySelector('.tnum')?.textContent }),
                  ),
                );
              full.decode().then(done, done);
              return;
            }
            if (now > 15000) {
              resolve({ preview4096: hi, first, shown: null });
              return;
            }
            requestAnimationFrame(poll);
          };
          requestAnimationFrame(poll);
        }),
    );
    await page.keyboard.press('Space');
    return done;
  };
  const coldFull = [];
  const browserCached = [];
  const runs = { cold: [], server_cached: [] };
  const tries = Math.min(Number(args['full-tries'] ?? 6), photos.length - steps - 2);
  // photos after the flipped ones: nothing cached yet
  for (let i = 0; i < tries; i++) {
    await page.keyboard.press('ArrowRight');
    await sleep(900);
    const cold = await toFull();
    runs.cold.push(cold);
    if (cold.shown !== null) coldFull.push(cold.shown);
    await page.keyboard.press('Space'); // back to fit
    await sleep(400);
    const again = await toFull();
    if (again.shown !== null) browserCached.push(again.shown);
    await page.keyboard.press('Space');
    await sleep(300);
  }
  // the first photos: generated by the server above, never shown in the browser at 100%
  const serverCached = [];
  await page.keyboard.press('Home');
  await sleep(900);
  for (let i = 0; i < Math.min(5, tail.length); i++) {
    if (i > 0) {
      await page.keyboard.press('ArrowRight');
      await sleep(900);
    }
    const r = await toFull();
    runs.server_cached.push(r);
    if (r.shown !== null) serverCached.push(r.shown);
    if (i < Math.min(5, tail.length) - 1) {
      await page.keyboard.press('Space');
      await sleep(300);
    }
  }
  result.fit_to_100 = {
    note: 'ms from the Space key press until the full-resolution layer is decoded, fully opaque (after its 160 ms fade-in) and painted; first = first frame it is partly visible',
    cold_ms: stats(coldFull),
    cold_first_visible_ms: stats(runs.cold.map((r) => r.first).filter((x) => x !== null)),
    server_cached_ms: stats(serverCached),
    browser_cached_ms: stats(browserCached),
    runs,
  };
  log('fit -> 100%', JSON.stringify({ ...result.fit_to_100, runs: undefined }));

  // pixel check at 100%: the screen (device pixels) at the pane centre against the full image itself
  const geo = await page.evaluate(() => {
    const pane = document.querySelector('[data-testid="zoom-pane"]').getBoundingClientRect();
    const full = [...document.querySelectorAll('[data-testid="zoom-pane"] img')].find((i) => i.src.includes('/full/'));
    const r = full.getBoundingClientRect();
    return { pane: { x: pane.x, y: pane.y, w: pane.width, h: pane.height }, img: { x: r.x, y: r.y, w: r.width, h: r.height }, natural: [full.naturalWidth, full.naturalHeight], dpr: devicePixelRatio };
  });
  const png = await page.screenshot(); // device pixels
  if (args.shots) {
    mkdirSync(resolve(args.shots), { recursive: true });
    writeFileSync(join(resolve(args.shots), `pixel-check-dpr${DPR}.png`), png);
  }
  result.pixel_check = await page.evaluate(
    async ({ b64, geo }) => {
      const shot = await new Promise((res, rej) => {
        const im = new Image();
        im.onload = () => res(im);
        im.onerror = rej;
        im.src = 'data:image/png;base64,' + b64;
      });
      const N = 240;
      const dpr = geo.dpr;
      // block of N x N device pixels at the pane centre
      const sx = Math.round((geo.pane.x + geo.pane.w / 2) * dpr) - N / 2;
      const sy = Math.round((geo.pane.y + geo.pane.h / 2) * dpr) - N / 2;
      const read = (src, x, y) => {
        const c = document.createElement('canvas');
        c.width = N;
        c.height = N;
        const g = c.getContext('2d');
        g.imageSmoothingEnabled = false;
        g.drawImage(src, x, y, N, N, 0, 0, N, N);
        return g.getImageData(0, 0, N, N).data;
      };
      const a = read(shot, sx, sy);
      const full = [...document.querySelectorAll('[data-testid="zoom-pane"] img')].find((i) => i.src.includes('/full/'));
      const left = geo.img.x * dpr;
      const top = geo.img.y * dpr;
      // image pixel under the block's top-left device pixel
      const ox = Math.round(sx - left);
      const oy = Math.round(sy - top);
      const diff = (dx, dy) => {
        const b = read(full, ox + dx, oy + dy);
        let sum = 0;
        let max = 0;
        let exact = 0;
        for (let i = 0; i < a.length; i += 4) {
          const d = Math.max(Math.abs(a[i] - b[i]), Math.abs(a[i + 1] - b[i + 1]), Math.abs(a[i + 2] - b[i + 2]));
          sum += d;
          max = Math.max(max, d);
          if (d === 0) exact++;
        }
        return { mean: +(sum / (a.length / 4)).toFixed(3), max, exact_share: +(exact / (a.length / 4)).toFixed(4) };
      };
      // rows of 24 px: mean difference per band (where does it differ?)
      const bands = [];
      {
        const b = read(full, ox, oy);
        for (let y0 = 0; y0 < N; y0 += 24) {
          let sum = 0;
          for (let y = y0; y < y0 + 24; y++)
            for (let x = 0; x < N; x++) {
              const i = (y * N + x) * 4;
              sum += Math.max(Math.abs(a[i] - b[i]), Math.abs(a[i + 1] - b[i + 1]), Math.abs(a[i + 2] - b[i + 2]));
            }
          bands.push(+(sum / (24 * N)).toFixed(2));
        }
      }
      // screen | image side by side, for a look
      const sbs = document.createElement('canvas');
      sbs.width = 2 * N;
      sbs.height = N;
      const g2 = sbs.getContext('2d');
      g2.imageSmoothingEnabled = false;
      g2.drawImage(shot, sx, sy, N, N, 0, 0, N, N);
      g2.drawImage(full, ox, oy, N, N, N, 0, N, N);
      let best = null;
      for (let dy = -2; dy <= 2; dy++)
        for (let dx = -2; dx <= 2; dx++) {
          const d = diff(dx, dy);
          if (!best || d.mean < best.mean) best = { dx, dy, ...d };
        }
      return {
        block_device_px: N,
        image_left_top_device_px: [+left.toFixed(3), +top.toFixed(3)],
        image_scale_device_px_per_image_px: +((geo.img.w * dpr) / geo.natural[0]).toFixed(5),
        aligned: diff(0, 0),
        shifted_by_one_pixel: diff(1, 0),
        best_offset: best,
        bands_aligned: bands,
        side_by_side: sbs.toDataURL('image/png'),
      };
    },
    { b64: png.toString('base64'), geo },
  );
  const sideBySide = result.pixel_check.side_by_side;
  delete result.pixel_check.side_by_side;
  if (args.shots) writeFileSync(join(resolve(args.shots), `side-by-side-dpr${DPR}.txt`), sideBySide);
  log('pixel check', JSON.stringify(result.pixel_check));
  result.console_errors = errors;
} finally {
  await browser?.close();
  await stop();
}
console.log(JSON.stringify(result, null, 2));
if (OUT) {
  mkdirSync(dirname(OUT), { recursive: true });
  writeFileSync(OUT, JSON.stringify(result, null, 2));
}
