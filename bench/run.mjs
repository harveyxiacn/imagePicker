// imagePicker performance benchmark driver (Node 22+).
//
//   node bench/run.mjs --label after [--only cold,import,query,patch,ws,preview,memory]
//        [--bin target/release/imagepicker] [--work <scratch dir>] [--out bench/results/<label>.json]
//
// Everything runs against a REAL `imagepicker serve` child process on a temp data dir.
// Synthetic data comes from `imagepicker bench gen|seed` (never real photos).
// Env: BENCH_QUICK=1 shrinks sample counts (smoke runs).
import { spawn, execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : 'true']);
    return acc;
  }, []),
);
const label = args.label ?? 'run';
const exe = process.platform === 'win32' ? '.exe' : '';
const BIN = resolve(args.bin ?? join(ROOT, process.env.CARGO_TARGET_DIR ?? 'target', 'release', 'imagepicker' + exe));
const WORKER = resolve(
  args.worker ?? join(dirname(BIN), 'ip-fake-worker' + exe),
);
const WORK = resolve(args.work ?? join(ROOT, 'bench', 'data'));
const OUT = resolve(args.out ?? join(ROOT, 'bench', 'results', `${label}.json`));
const only = new Set((args.only ?? 'cold,import,query,patch,ws,preview,memory').split(','));
const QUICK = !!process.env.BENCH_QUICK;
const N = QUICK ? 5 : 30;
let nextPort = 18000 + Math.floor(Math.random() * 1000);

mkdirSync(WORK, { recursive: true });
const results = { label, date: new Date().toISOString(), host: process.platform, quick: QUICK };

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const now = () => performance.now();
const stats = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  return { n: s.length, p50: +q(0.5).toFixed(2), p95: +q(0.95).toFixed(2), max: +s[s.length - 1].toFixed(2) };
};
const log = (...a) => console.error('[bench]', ...a);

function cli(...a) {
  return execFileSync(BIN, a, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 1 << 26 });
}

function ensureSeed(name, rows, sessionSize) {
  const dir = join(WORK, name);
  if (!existsSync(join(dir, 'catalog.db'))) {
    log(`seeding ${name} (${rows} rows)`);
    cli('bench', 'seed', '--data-dir', dir, '--rows', String(rows), '--session-size', String(sessionSize));
  }
  return dir;
}

function ensurePhotos(count) {
  const dir = join(WORK, `photos${count}`);
  if (!existsSync(dir) || readdirSync(dir).length < count) {
    log(`generating ${count} JPEGs`);
    cli('bench', 'gen', dir, '--count', String(count));
  }
  return dir;
}

function copyDir(src, dst) {
  rmSync(dst, { recursive: true, force: true });
  mkdirSync(dst, { recursive: true });
  for (const f of readdirSync(src)) {
    execFileSync(process.platform === 'win32' ? 'cmd' : 'cp', process.platform === 'win32' ? ['/c', 'copy', '/Y', join(src, f), join(dst, f)] : [join(src, f), join(dst, f)], { stdio: 'ignore' });
  }
}

function rssMB(pid) {
  try {
    if (process.platform === 'win32') {
      const out = execFileSync(
        'powershell',
        ['-NoProfile', '-Command', `$p=Get-Process -Id ${pid}; "$($p.WorkingSet64) $($p.PrivateMemorySize64) $($p.PeakWorkingSet64)"`],
        { encoding: 'utf8' },
      ).trim().split(' ').map(Number);
      return { ws: +(out[0] / 1048576).toFixed(0), priv: +(out[1] / 1048576).toFixed(0), peak: +(out[2] / 1048576).toFixed(0) };
    }
    const kb = Number(execFileSync('ps', ['-o', 'rss=', '-p', String(pid)], { encoding: 'utf8' }));
    return { ws: Math.round(kb / 1024), priv: Math.round(kb / 1024), peak: Math.round(kb / 1024) };
  } catch {
    return null;
  }
}

class Server {
  constructor(dataDir, env = {}) {
    this.dataDir = dataDir;
    this.port = nextPort++;
    this.base = `http://127.0.0.1:${this.port}`;
    this.env = env;
  }
  async start() {
    const t0 = now();
    this.child = spawn(BIN, ['serve', '--port', String(this.port), '--data-dir', this.dataDir], {
      env: { ...process.env, IMAGEPICKER_WORKER_CMD: WORKER, RUST_LOG: 'warn', ...this.env },
      stdio: ['ignore', 'ignore', 'pipe'],
    });
    this.stderr = '';
    this.child.stderr.on('data', (d) => (this.stderr += d));
    for (;;) {
      if (this.child.exitCode !== null) throw new Error('server exited: ' + this.stderr);
      try {
        const r = await fetch(this.base + '/api/health');
        if (r.ok) break;
      } catch {
        /* not up yet */
      }
      await sleep(2);
    }
    this.startMs = now() - t0;
    return this;
  }
  async stop() {
    if (!this.child) return;
    const c = this.child;
    this.child = null;
    c.kill();
    await new Promise((r) => (c.exitCode !== null ? r() : c.once('exit', r)));
    await sleep(50);
  }
  async get(path) {
    const r = await fetch(this.base + path);
    if (!r.ok) throw new Error(`${path} -> ${r.status} ${await r.text()}`);
    return r.json();
  }
  async send(method, path, body) {
    const r = await fetch(this.base + path, {
      method,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!r.ok) throw new Error(`${method} ${path} -> ${r.status} ${await r.text()}`);
    return r.json();
  }
  async time(path) {
    const t = now();
    const r = await fetch(this.base + path);
    const buf = await r.arrayBuffer();
    if (!r.ok) throw new Error(`${path} -> ${r.status}`);
    return { ms: now() - t, bytes: buf.byteLength };
  }
}

async function repeat(n, fn) {
  const xs = [];
  for (let i = 0; i < n; i++) xs.push(await fn(i));
  return stats(xs);
}

// ------------------------------------------------------------------ cold start
async function benchCold() {
  const out = {};
  for (const [name, dir] of [
    ['empty_catalog', join(WORK, 'cold-empty')],
    ['catalog_20k', ensureSeed('seed20k', 20000, 20000)],
    ['catalog_100k', ensureSeed('seed100k', 100000, 20000)],
  ]) {
    if (name === 'empty_catalog') rmSync(dir, { recursive: true, force: true });
    const xs = [];
    for (let i = 0; i < (QUICK ? 2 : 7); i++) {
      const s = await new Server(dir).start();
      xs.push(s.startMs);
      await s.stop();
    }
    out[name] = stats(xs);
    log('cold start', name, out[name]);
  }
  results.cold_start_ms = out;
}

// ------------------------------------------------------------------ import
function openEvents(server) {
  const ws = new WebSocket(server.base.replace('http', 'ws') + '/api/events');
  const listeners = [];
  ws.onmessage = (m) => {
    const ev = JSON.parse(m.data);
    for (const l of listeners) l(ev);
  };
  const ready = new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = rej;
  });
  return { ws, ready, on: (f) => listeners.push(f) };
}

async function benchImport() {
  const count = QUICK ? 100 : 1000;
  const photos = ensurePhotos(count);
  const dir = join(WORK, 'import-data');
  rmSync(dir, { recursive: true, force: true });
  const s = await new Server(dir).start();
  const ev = openEvents(s);
  await ev.ready;
  const seen = new Set();
  const marks = {};
  let t0 = 0;
  ev.on((e) => {
    if (e.type === 'thumbs.ready') {
      for (const it of e.items) seen.add(it.id);
      if (seen.size >= 1 && !marks.first_thumb) marks.first_thumb = now() - t0;
      if (seen.size >= 100 && !marks.thumbs_100) marks.thumbs_100 = now() - t0;
      if (seen.size >= count && !marks.thumbs_all) marks.thumbs_all = now() - t0;
    }
  });
  t0 = now();
  const { session } = await s.send('POST', '/api/import', { path: photos, recursive: false });
  marks.import_call = now() - t0;
  // First screen: the first page of rows (what the grid renders) is available and has 100 rows.
  for (;;) {
    const p = await s.get(`/api/photos?session_id=${session.id}&limit=100`);
    if (p.photos.length >= Math.min(100, count)) {
      marks.first_rows_100 = now() - t0;
      break;
    }
    await sleep(5);
  }
  // First screen with metadata (sorted by capture time: all rows have taken_at + size)
  for (;;) {
    const p = await s.get(`/api/photos?session_id=${session.id}&limit=100`);
    if (p.photos.length >= Math.min(100, count) && p.photos.every((x) => x.width)) {
      marks.first_screen_meta = now() - t0;
      break;
    }
    await sleep(5);
  }
  while (!marks.thumbs_all) {
    if (now() - t0 > 180000) throw new Error('import timeout');
    await sleep(20);
  }
  for (;;) {
    const sess = (await s.get(`/api/sessions/${session.id}`)).session;
    if (sess.import_state === 'ready') {
      marks.session_ready = now() - t0;
      break;
    }
    await sleep(20);
  }
  const rss = rssMB(s.child.pid);
  ev.ws.close();
  const dataDir = dir;
  results.import = {
    photos: count,
    ms: Object.fromEntries(Object.entries(marks).map(([k, v]) => [k, +v.toFixed(0)])),
    rss_after_import_mb: rss,
  };
  log('import', results.import);
  await s.stop();
  results._importDataDir = dataDir;
}

// ------------------------------------------------------------------ queries
async function benchQuery() {
  const out = {};
  for (const [name, dir, sessionSize] of [
    ['rows_20k_1session', ensureSeed('seed20k', 20000, 20000), 20000],
    ['rows_100k_5sessions', ensureSeed('seed100k', 100000, 20000), 20000],
    ['rows_100k_1session_stress', ensureSeed('seed100k1', 100000, 100000), 100000],
  ]) {
    const s = await new Server(dir).start();
    const sessions = (await s.get('/api/sessions')).sessions;
    const sid = sessions[sessions.length - 1].id; // an older, full session
    const sizeNow = sessions.find((x) => x.id === sid).photo_count;
    if (sizeNow !== sessionSize) throw new Error(`session size ${sizeNow}`);
    const q = `session_id=${sid}`;
    const cases = {
      'default (taken_at) limit=500': `${q}&limit=500`,
      'default limit=5000 (web page size)': `${q}&limit=5000`,
      'sort=-taken_at': `${q}&sort=-taken_at&limit=500`,
      'sort=name': `${q}&sort=name&limit=500`,
      'sort=rating': `${q}&sort=rating&limit=500`,
      'sort=ai': `${q}&sort=ai&limit=500`,
      'rating_gte=4': `${q}&rating_gte=4&limit=500`,
      'flag=picked': `${q}&flag=picked&limit=500`,
      'flag=rejected sort=ai': `${q}&flag=rejected&sort=ai&limit=500`,
      'issues_any=blurry': `${q}&issues_any=blurry&limit=500`,
      'issues_none=1': `${q}&issues_none=1&limit=500`,
      'burst_best_only=1': `${q}&burst_best_only=1&limit=500`,
      'scene_type=portrait': `${q}&scene_type=portrait&limit=500`,
      'persons=2': `${q}&persons=2&limit=500`,
      'persons=2,3 mode=all': `${q}&persons=2,3&person_mode=all&limit=500`,
      'persons=2 smiling': `${q}&persons=2&person_state=smiling&limit=500`,
      'exclude_persons=2': `${q}&exclude_persons=2&limit=500`,
      'person_state=eyes_open': `${q}&person_state=eyes_open&limit=500`,
      'ai_rating_gte=4 sort=ai': `${q}&ai_rating_gte=4&sort=ai&limit=500`,
      'limit=1 (collection count)': `${q}&flag=picked&limit=1`,
      'burst_id=<one>': null,
    };
    const first = await s.get(`/api/photos?${q}&burst_best_only=1&limit=500`);
    const bid = first.photos.find((p) => p.burst_id)?.burst_id;
    cases['burst_id=<one>'] = bid ? `${q}&burst_id=${bid}` : `${q}&limit=1`;
    const table = {};
    for (const [k, qs] of Object.entries(cases)) {
      await s.get(`/api/photos?${qs}`); // warm
      table[k] = await repeat(N, async () => (await s.time(`/api/photos?${qs}`)).ms);
    }
    out[name] = { sessions: sessions.length, cases: table };
    // cursor depth: time of the page at depth d (pages of 500), per sort
    const depth = {};
    for (const sort of ['taken_at', 'name', 'ai']) {
      const times = {};
      let cursor = '';
      for (let d = 0; d < Math.min(sessionSize / 500, 60); d++) {
        const url = `/api/photos?${q}&sort=${sort}&limit=500${cursor ? '&cursor=' + cursor : ''}`;
        const t = now();
        const page = await s.get(url);
        const ms = now() - t;
        if ([0, 5, 20, 39, 59].includes(d)) times[`page${d}`] = +ms.toFixed(1);
        if (!page.next_cursor) break;
        cursor = page.next_cursor;
      }
      depth[sort] = times;
    }
    out[name].cursor_depth_ms = depth;
    // whole-session pull the way the web does it (5000 per page)
    const pull = [];
    for (let i = 0; i < (QUICK ? 1 : 5); i++) {
      const t = now();
      let cursor = '';
      for (;;) {
        const page = await s.get(`/api/photos?${q}&limit=5000${cursor ? '&cursor=' + cursor : ''}`);
        if (!page.next_cursor) break;
        cursor = page.next_cursor;
      }
      pull.push(now() - t);
    }
    out[name].full_session_pull_ms = stats(pull);
    out[name].sessions_list_ms = await repeat(N, async () => (await s.time('/api/sessions')).ms);
    out[name].groups_ms = await repeat(5, async () => (await s.time(`/api/groups?session_id=${sid}`)).ms);
    out[name].people_ms = await repeat(5, async () => (await s.time(`/api/people?session_id=${sid}`)).ms);
    log('query', name, 'done');
    await s.stop();
  }
  results.query = out;
}

// ------------------------------------------------------------------ PATCH + WS
async function benchPatchWs() {
  const dir = ensureSeed('seed20k', 20000, 20000);
  const s = await new Server(dir).start();
  const sid = (await s.get('/api/sessions')).sessions[0].id;
  const all = [];
  let cursor = '';
  for (;;) {
    const page = await s.get(`/api/photos?session_id=${sid}&limit=5000${cursor ? '&cursor=' + cursor : ''}`);
    all.push(...page.photos.map((p) => p.id));
    if (!page.next_cursor) break;
    cursor = page.next_cursor;
  }
  const out = { patch_ms: {}, ws: {} };
  let i = 0;
  const patch = async (ids, body) => {
    const t = now();
    await s.send('PATCH', '/api/photos', { ids, ...body });
    return now() - t;
  };
  out.patch_ms['1 photo rating'] = await repeat(N, async () => patch([all[i++ % all.length]], { user_rating: 1 + (i % 5) }));
  out.patch_ms['1 photo flag'] = await repeat(N, async () => patch([all[i++ % all.length]], { flag: i % 2 }));
  out.patch_ms['100 photos rating'] = await repeat(10, async () => patch(all.slice(0, 100), { user_rating: 1 + (i++ % 5) }));
  out.patch_ms['1000 photos flag'] = await repeat(5, async () => patch(all.slice(0, 1000), { flag: i++ % 2 }));
  out.patch_ms['5000 photos rating'] = await repeat(3, async () => patch(all.slice(0, 5000), { user_rating: 1 + (i++ % 5) }));
  out.patch_ms['20000 photos rating (select all)'] = await repeat(3, async () => patch(all, { user_rating: 1 + (i++ % 5) }));
  // WS: time from PATCH to receiving all photos.updated items
  const ev = openEvents(s);
  await ev.ready;
  const wsTimes = [];
  for (const n of [1, 100, 1000, 5000, 20000]) {
    const ids = all.slice(0, n);
    let got = new Set();
    let frames = 0;
    let bytes = 0;
    let resolve;
    const done = new Promise((r) => (resolve = r));
    let t = 0;
    ev.on((e) => {
      if (e.type !== 'photos.updated' || !t) return;
      frames++;
      bytes += JSON.stringify(e).length;
      for (const it of e.items) got.add(it.id);
      if (got.size >= ids.length) resolve(now() - t);
    });
    await sleep(150);
    t = now();
    await s.send('PATCH', '/api/photos', { ids, user_rating: 1 + (i++ % 5) });
    const ms = await Promise.race([done, sleep(30000).then(() => -1)]);
    t = 0;
    out.ws[`photos.updated x${n}`] = { ms: +ms.toFixed(0), frames, kb: Math.round(bytes / 1024) };
    wsTimes.push(ms);
  }
  ev.ws.close();
  results.patch_ws = out;
  log('patch/ws', JSON.stringify(out));
  await s.stop();
}

// ------------------------------------------------------------------ preview / thumb latency
async function benchPreview() {
  const dir = results._importDataDir ?? join(WORK, 'import-data');
  if (!existsSync(join(dir, 'catalog.db'))) {
    await benchImport();
  }
  const s = await new Server(join(WORK, 'import-data')).start();
  const sid = (await s.get('/api/sessions')).sessions[0].id;
  const ids = (await s.get(`/api/photos?session_id=${sid}&limit=200`)).photos.map((p) => p.id);
  const out = {};
  out.thumb256_cached_ms = await repeat(N, async (k) => (await s.time(`/api/thumb/${ids[k % 100]}?s=256`)).ms);
  out.thumb512_cold_ms = stats(await Promise.all(ids.slice(0, 20).map(async (id) => (await s.time(`/api/thumb/${id}?s=512`)).ms)));
  const cold = [];
  for (const id of ids.slice(20, 20 + (QUICK ? 3 : 12))) cold.push((await s.time(`/api/preview/${id}?s=2048`)).ms);
  out.preview2048_cold_ms = stats(cold);
  out.preview2048_warm_ms = await repeat(N, async (k) => (await s.time(`/api/preview/${ids[20 + (k % 3)]}?s=2048`)).ms);
  const cold1024 = [];
  for (const id of ids.slice(60, 60 + (QUICK ? 3 : 12))) cold1024.push((await s.time(`/api/preview/${id}?s=1024`)).ms);
  out.preview1024_cold_ms = stats(cold1024);
  // loupe "next photo": preview of id[k+1] when previews were prefetched vs not
  out.preview1024_warm_ms = await repeat(N, async (k) => (await s.time(`/api/preview/${ids[60 + (k % 3)]}?s=1024`)).ms);
  results.preview = out;
  log('preview', JSON.stringify(out));
  await s.stop();
}

// ------------------------------------------------------------------ memory over a 20k session
async function benchMemory() {
  const dir = ensureSeed('seed20k', 20000, 20000);
  const s = await new Server(dir).start();
  const sid = (await s.get('/api/sessions')).sessions[0].id;
  const out = { idle: rssMB(s.child.pid) };
  const all = [];
  let cursor = '';
  for (;;) {
    const page = await s.get(`/api/photos?session_id=${sid}&limit=5000${cursor ? '&cursor=' + cursor : ''}`);
    all.push(...page.photos);
    if (!page.next_cursor) break;
    cursor = page.next_cursor;
  }
  out.after_list_20k = rssMB(s.child.pid);
  for (let r = 0; r < (QUICK ? 2 : 20); r++) {
    for (const sort of ['taken_at', '-taken_at', 'name', 'ai']) await s.get(`/api/photos?session_id=${sid}&sort=${sort}&limit=5000`);
  }
  out.after_80_full_listings = rssMB(s.child.pid);
  const ev = openEvents(s);
  await ev.ready;
  await s.send('PATCH', '/api/photos', { ids: all.map((p) => p.id), user_rating: 3 });
  await sleep(500);
  out.after_patch_20k = rssMB(s.child.pid);
  ev.ws.close();
  results.memory_mb = out;
  log('memory', JSON.stringify(out));
  await s.stop();
}

const steps = { cold: benchCold, import: benchImport, query: benchQuery, patch: benchPatchWs, ws: null, preview: benchPreview, memory: benchMemory };
for (const [k, fn] of Object.entries(steps)) {
  if (!fn || !only.has(k) && !(k === 'patch' && only.has('ws'))) continue;
  log('==', k);
  await fn();
}
delete results._importDataDir;
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, JSON.stringify(results, null, 2));
log('wrote', OUT);
