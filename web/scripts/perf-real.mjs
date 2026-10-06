// Frontend performance against a REAL `imagepicker serve` (no mock): grid scroll frame times
// and loupe next-photo latency on a 20k-photo session.
//
//   cd web && pnpm build
//   imagepicker bench gen <dir> --count 1000
//   imagepicker bench seed --data-dir <data> --rows 20000 --files-dir <dir>
//   node scripts/perf-real.mjs --bin <imagepicker> --data-dir <data> [--out result.json] [--label after]
//
// Frame times come from a requestAnimationFrame sampler inside the page while the grid is scrolled
// programmatically at a fixed speed. Headless Chromium rasterises in software, so absolute numbers
// are pessimistic compared with a GPU-accelerated desktop browser; use them for before/after only.
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
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
const DATA = resolve(args['data-dir'] ?? '');
const WEB = resolve(args['web-dir'] ?? resolve(HERE, '../dist'));
const OUT = args.out ? resolve(args.out) : null;
const PORT = Number(args.port ?? 18800 + Math.floor(Math.random() * 100));
const BASE = `http://127.0.0.1:${PORT}`;
if (!args['data-dir']) throw new Error('--data-dir is required (see the header of this file)');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.error('[perf-real]', ...a);

const server = spawn(BIN, ['serve', '--port', String(PORT), '--data-dir', DATA, '--web-dir', WEB], {
  stdio: ['ignore', 'ignore', 'inherit'],
  env: { ...process.env, RUST_LOG: 'warn' },
});
const stopServer = async () => {
  server.kill();
  await new Promise((r) => (server.exitCode !== null ? r() : server.once('exit', r)));
};
process.on('SIGINT', () => stopServer().then(() => process.exit(130)));

for (let i = 0; ; i++) {
  try {
    if ((await fetch(BASE + '/api/health')).ok) break;
  } catch {
    /* starting */
  }
  if (i > 400) throw new Error('server did not start');
  await sleep(25);
}

const { sessions } = await (await fetch(BASE + '/api/sessions')).json();
const session = sessions[0];
log(`session ${session.id}: ${session.photo_count} photos`);

// no first-run dialog in the way
await fetch(BASE + '/api/onboarding/done', { method: 'POST' }).catch(() => undefined);

const errors = [];
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
page.on('pageerror', (e) => errors.push(String(e)));

const stats = (xs) => {
  if (!xs.length) return { n: 0 };
  const s = [...xs].sort((a, b) => a - b);
  const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  const r = (x) => +x.toFixed(2);
  return { n: s.length, p50: r(q(0.5)), p95: r(q(0.95)), p99: r(q(0.99)), max: r(s[s.length - 1]) };
};

const result = { label: args.label ?? 'run', photos: session.photo_count, viewport: '1440x900' };

// ---------------------------------------------------------------- load
const t0 = Date.now();
await page.goto(`${BASE}/s/${session.id}`);
await page.waitForSelector('[data-testid="grid"] img', { timeout: 60000 });
result.first_thumb_visible_ms = Date.now() - t0;
await page.waitForFunction(
  (n) => document.querySelector('[data-testid="grid"]')?.getAttribute('aria-rowcount') > 100 || n < 100,
  session.photo_count,
  { timeout: 60000 },
);
result.grid_rows_ready_ms = Date.now() - t0;
const heap = () => page.evaluate(() => (performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : null));
result.js_heap_mb_after_load = await heap();

// ---------------------------------------------------------------- scroll
const installRunner = () =>
  page.evaluate(() => {
  window.__perf = {
    frames: [],
    long: [],
    run(el, pxPerSec, ms) {
      return new Promise((resolve) => {
        const frames = [];
        const long = [];
        const po = new PerformanceObserver((l) => l.getEntries().forEach((e) => long.push(e.duration)));
        try {
          po.observe({ type: 'longtask', buffered: false });
        } catch {
          /* unsupported */
        }
        let last = performance.now();
        const start = last;
        let dir = 1;
        let travelled = 0;
        const seen = new Set();
        const tick = (t) => {
          const dt = t - last;
          last = t;
          if (frames.length || dt < 1000) frames.push(dt);
          const max = el.scrollHeight - el.clientHeight;
          const before = el.scrollTop;
          el.scrollTop = Math.min(max, Math.max(0, el.scrollTop + dir * pxPerSec * (dt / 1000)));
          travelled += Math.abs(el.scrollTop - before);
          if (el.scrollTop >= max - 1) dir = -1;
          if (el.scrollTop <= 1) dir = 1;
          for (const c of el.querySelectorAll('[data-id]')) seen.add(c.getAttribute('data-id'));
          if (t - start < ms) requestAnimationFrame(tick);
          else {
            po.disconnect();
            resolve({ frames, long, travelled: Math.round(travelled), seen: seen.size, height: el.scrollHeight });
          }
        };
        requestAnimationFrame(tick);
      });
    },
  };
});
await installRunner();
const scrollRun = async (px, ms) => {
  const r = await page.evaluate(([s, m]) => window.__perf.run(document.querySelector('[data-testid="grid"]'), s, m), [px, ms]);
  const jank = r.frames.filter((d) => d > 33.4).length;
  return { frame_ms: stats(r.frames), frames: r.frames.length, scrolled_px: r.travelled, distinct_cells_shown: r.seen, scroll_height: r.height, jank_frames_gt33ms: jank, long_tasks: r.long.length, long_task_ms_total: +r.long.reduce((a, b) => a + b, 0).toFixed(0) };
};
const settle = async () => {
  await page.waitForFunction(
    () => {
      const imgs = [...document.querySelectorAll('[data-testid="grid"] img')];
      return imgs.length > 0 && imgs.every((i) => i.complete);
    },
    null,
    { timeout: 60000 },
  );
};

// pass 1 warms the thumbnail cache of whatever it scrolls over (the server generates on demand)
const dur = Number(args['scroll-ms'] ?? 6000);
result.scroll_cold = { '2500px/s': await scrollRun(2500, dur) };
await settle();
await page.evaluate(() => (document.querySelector('[data-testid="grid"]').scrollTop = 0));
await sleep(500);
result.scroll_warm = {
  '1500px/s': await scrollRun(1500, dur),
  '2500px/s': await scrollRun(2500, dur),
  '6000px/s': await scrollRun(6000, dur),
};
result.grid = result.scroll_warm['2500px/s'].frame_ms;
result.js_heap_mb_after_scroll = await heap();
const mounted = await page.evaluate(() => document.querySelectorAll('[data-testid="grid"] [data-id]').length);
result.mounted_cells = mounted;

// jump to random positions: time until every visible thumbnail has loaded
const jumps = [];
for (const f of [0.1, 0.55, 0.3, 0.9, 0.7, 0.2]) {
  const t = Date.now();
  await page.evaluate((fr) => {
    const el = document.querySelector('[data-testid="grid"]');
    el.scrollTop = (el.scrollHeight - el.clientHeight) * fr;
  }, f);
  await page.waitForTimeout(50);
  await settle();
  jumps.push(Date.now() - t);
}
result.jump_settle_ms = stats(jumps);

// ---------------------------------------------------------------- loupe
await page.evaluate(() => (document.querySelector('[data-testid="grid"]').scrollTop = 0));
await sleep(300);
await page.locator('[data-testid="grid"] [data-id]').first().dblclick();
await page.waitForSelector('[data-testid="zoom-pane"]');
await page.waitForFunction(() => [...document.querySelectorAll('[data-testid="zoom-pane"] img')].some((i) => i.src.includes('/preview/') && i.complete && i.naturalWidth > 0));
await sleep(800);

await page.evaluate(() => {
  const probe = {
    t0: 0,
    results: [],
    arm() {
      this.t0 = performance.now();
      this.first = null;
      this.settled = null;
      const startSrc = new Set([...document.querySelectorAll('[data-testid="zoom-pane"] img')].map((i) => i.src));
      const poll = () => {
        const imgs = [...document.querySelectorAll('[data-testid="zoom-pane"] img')];
        // images that belong to the NEW photo (their src was not on screen before the key press)
        const fresh = imgs.filter((i) => !startSrc.has(i.src));
        const visible = (i) => i.complete && i.naturalWidth > 0 && parseFloat(getComputedStyle(i).opacity) > 0.05;
        const full = (i) => i.complete && i.naturalWidth > 0 && parseFloat(getComputedStyle(i).opacity) > 0.99;
        const now = performance.now();
        if (this.first === null && fresh.some(visible)) this.first = now - this.t0;
        if (this.settled === null && fresh.some((i) => i.src.includes('/preview/') && full(i))) {
          this.settled = now - this.t0;
          this.results.push({ first: this.first, settled: this.settled });
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
const steps = Number(args['loupe-steps'] ?? 40);
for (let i = 0; i < steps; i++) {
  await page.keyboard.press('ArrowRight');
  await sleep(i < steps / 2 ? 400 : 200); // relaxed browsing, then quicker flips
}
await sleep(800);
const lr = await page.evaluate(() => window.__probe.results);
const pick = (k, from, to) => lr.slice(from, to).map((r) => r[k]).filter((x) => x !== null);
result.loupe = {
  first_pixels_ms: stats(pick('first', 0, lr.length)),
  settled_ms: stats(pick('settled', 0, lr.length)),
  relaxed_settled_ms: stats(pick('settled', 0, steps / 2)),
  quick_settled_ms: stats(pick('settled', steps / 2, lr.length)),
  not_settled: lr.filter((r) => r.settled === null).length,
};
// rapid repeat (held key)
await page.evaluate(() => (window.__probe.results.length = 0));
for (let i = 0; i < 25; i++) {
  await page.keyboard.press('ArrowRight');
  await sleep(35);
}
await sleep(2500);
result.loupe_held_key = await page.evaluate(() => window.__probe.results.map((r) => r.settled).filter((x) => x !== null).slice(-1)[0] ?? null);
result.js_heap_mb_end = await heap();

// ---------------------------------------------------------------- dense grid, throttled CPU
// Smallest thumbnails, no grouping (most mounted cells) with the CPU slowed down 4x: a mid-range laptop.
await page.keyboard.press('Escape');
await page.addInitScript(() => {
  const k = 'imagepicker.ui';
  const v = JSON.parse(localStorage.getItem(k) || '{"state":{},"version":0}');
  v.state.thumbSize = 96;
  v.state.grouped = false; // plain rows: the worst case for the number of mounted cells
  localStorage.setItem(k, JSON.stringify(v));
});
await page.goto(`${BASE}/s/${session.id}`);
await page.waitForSelector('[data-testid="grid"] [data-id]', { timeout: 60000 });
await page.evaluate(() => {
  window.__perf = window.__perf ?? {};
});
const cdp = await page.context().newCDPSession(page);
await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
result.dense_throttled_4x = {};
await page.evaluate(() => (document.querySelector('[data-testid="grid"]').scrollTop = 0));
for (const [name, px] of [['2500px/s', 2500], ['6000px/s', 6000]]) {
  await installRunner();
  await settle();
  result.dense_throttled_4x[name] = await scrollRun(px, dur);
}
result.dense_mounted_cells = await page.evaluate(() => document.querySelectorAll('[data-testid="grid"] [data-id]').length);
await cdp.send('Emulation.setCPUThrottlingRate', { rate: 1 });
result.console_errors = errors;

await browser.close();
await stopServer();
console.log(JSON.stringify(result, null, 2));
if (OUT) {
  mkdirSync(dirname(OUT), { recursive: true });
  writeFileSync(OUT, JSON.stringify(result, null, 2));
}
process.exit(errors.length ? 1 : 0);
