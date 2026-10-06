// Chrome trace of a fast scroll through a dense, ungrouped grid (CPU throttled 4x), summarised by
// event name. Used to find where main-thread time goes; not part of the benchmark numbers.
//   node scripts/perf-trace.mjs --bin <imagepicker> --data-dir <seeded data dir> [--rate 4] [--px 6000]
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const HERE = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1]]);
    return acc;
  }, []),
);
const PORT = 19100 + Math.floor(Math.random() * 100);
const BASE = `http://127.0.0.1:${PORT}`;
const server = spawn(resolve(args.bin), ['serve', '--port', String(PORT), '--data-dir', resolve(args['data-dir']), '--web-dir', resolve(HERE, '../dist')], { stdio: 'ignore' });
for (let i = 0; i < 400; i++) {
  try {
    if ((await fetch(BASE + '/api/health')).ok) break;
  } catch {
    /* starting */
  }
  await new Promise((r) => setTimeout(r, 25));
}
await fetch(BASE + '/api/onboarding/done', { method: 'POST' });
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
await page.goto(BASE);
await page.evaluate(() => {
  localStorage.setItem('imagepicker.ui', JSON.stringify({ state: { thumbSize: 96, grouped: false }, version: 0 }));
});
await page.goto(`${BASE}/s/1`);
await page.waitForSelector('[data-testid="grid"] [data-id]');
const cdp = await page.context().newCDPSession(page);
await cdp.send('Emulation.setCPUThrottlingRate', { rate: Number(args.rate ?? 4) });
// warm: thumbnails for the scrolled range exist (server generates on demand)
const scroll = (px, ms) =>
  page.evaluate(
    ([pps, dur]) =>
      new Promise((resolve) => {
        const el = document.querySelector('[data-testid="grid"]');
        const frames = [];
        let last = performance.now();
        const start = last;
        const tick = (t) => {
          frames.push(t - last);
          el.scrollTop += pps * ((t - last) / 1000);
          last = t;
          if (t - start < dur) requestAnimationFrame(tick);
          else resolve(frames);
        };
        requestAnimationFrame(tick);
      }),
    [px, ms],
  );
await scroll(Number(args.px ?? 6000), 4000);
await page.evaluate(() => (document.querySelector('[data-testid="grid"]').scrollTop = 0));
await page.waitForTimeout(1500);
await browser.startTracing(page, { path: resolve(HERE, '../trace-tmp.json'), categories: ['devtools.timeline', 'disabled-by-default-devtools.timeline', 'v8.execute'] });
const frames = await scroll(Number(args.px ?? 6000), 4000);
await browser.stopTracing();
const s = [...frames].sort((a, b) => a - b);
console.log('frames', frames.length, 'p50', s[s.length >> 1].toFixed(1), 'p95', s[Math.floor(s.length * 0.95)].toFixed(1));
const trace = JSON.parse(readFileSync(resolve(HERE, '../trace-tmp.json'), 'utf8'));
const events = (trace.traceEvents ?? trace).filter((e) => e.ph === 'X' && e.dur);
const byName = new Map();
for (const e of events) {
  const k = e.name + (e.args?.data?.functionName ? ':' + e.args.data.functionName : '');
  const v = byName.get(k) ?? { n: 0, ms: 0 };
  v.n++;
  v.ms += e.dur / 1000;
  byName.set(k, v);
}
const top = [...byName.entries()].sort((a, b) => b[1].ms - a[1].ms).slice(0, 25);
for (const [k, v] of top) console.log(String(Math.round(v.ms)).padStart(7), 'ms', String(v.n).padStart(6), 'x', k);
await browser.close();
server.kill();
