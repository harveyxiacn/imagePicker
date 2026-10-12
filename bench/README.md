# Benchmarks (M7)

Everything here is synthetic: no real photos are read or written. Results and the PRD comparison
live in [`REPORT.md`](REPORT.md); raw numbers in `results/*.json`.

## Pieces

| Piece | What it does |
|---|---|
| `imagepicker bench gen <dir> --count N [--megapixels 12]` | Writes N JPEGs with EXIF: capture times in bursts (3-8 frames, 180-340 ms apart) and scenes, mixed sizes (12 MP landscape/portrait, 8% larger), 3 camera bodies. Frames of one burst share a base image, so they look alike to the grouping code. |
| `imagepicker bench seed --data-dir <dir> --rows N --session-size S` | Fills a catalog directly (no files): photos, bursts, faces, people, ratings, flags, issues, AI scores. 100k rows take about a second. `--files-dir <gen dir>` points the rows at real files (cycling through them, each row a distinct photo) so the web UI has thumbnails to load. |
| `bench/run.mjs` | Spawns real `imagepicker serve` processes and measures: cold start, import of 1000 x 12 MP JPEG, `/api/photos` latency at 20k and 100k rows (filters, sorts, cursor depth), PATCH latency, WebSocket event latency, preview latency, RSS. |
| `bench/report.mjs` | Renders `REPORT.md` from two result files (before / after) and `results/frontend.json`. |
| `web/scripts/perf-real.mjs` | Playwright against the real server and the built UI: grid scroll frame times (requestAnimationFrame sampler) and loupe next-photo latency on a 20k session. |
| `web/scripts/perf-trace.mjs` | Chrome trace of a fast scroll, summarised by event (where main-thread time goes). |
| `web/scripts/e2e-real.mjs` | Real-server end-to-end test, see [`CI-E2E.md`](CI-E2E.md). |

## Analysis quality (synthetic photo library)

These score the analysis itself rather than speed; inputs are the generated photo library in
`bench/data/qwen-photo-library` (git-ignored) and catalogs written by
`imagepicker analyze <dir> --profile lite --data-dir <data dir>`.

| Piece | What it does |
|---|---|
| `bench/eval-library.py LIBRARY DATA_DIR` | Issue-tag precision / recall against the manifest tags, burst grouping, metric distributions, star histogram. |
| `bench/make-tilt-set.py [LIBRARY] [OUT]` | Rotates 247 library photos by 0, +-1, +-2, +-3, +-5, +-8 degrees (border-free centre crop) into `bench/data/tilt-set` with the true angles in its manifest (Pillow). |
| `bench/eval-tilt.py TILT_SET DATA_DIR [--library LIBRARY DATA_DIR]` | Tilt estimate error, `tilted` flag precision / recall per angle, false positives on the 0-degree copies and the library, and a threshold sweep (docs/03 section 3.1.1). |

## Reproduce

```sh
export CARGO_TARGET_DIR=target-m7perf          # optional
cargo build --release -p ip-cli -p ip-worker-client --bins
(cd web && pnpm install && pnpm build)

# server benchmarks (data goes to bench/data, which is git-ignored)
node bench/run.mjs --label after                # or --only cold,import,query,patch,preview,memory
BENCH_QUICK=1 node bench/run.mjs --label smoke  # 5 samples instead of 30

# frontend: a 20k-row catalog whose rows point at 1000 generated JPEGs
target/release/imagepicker bench gen bench/data/photos1000 --count 1000
target/release/imagepicker bench seed --data-dir bench/data/seedweb --rows 20000 --files-dir bench/data/photos1000
(cd web && node scripts/perf-real.mjs --data-dir ../bench/data/seedweb --label after --out ../bench/results/frontend-after.json)

node bench/report.mjs bench/results/baseline.json bench/results/after.json bench/results/frontend.json > bench/REPORT.md
```

`run.mjs` options: `--bin` (default `<target>/release/imagepicker`), `--work` (scratch dir, default
`bench/data`), `--out`, `--only`. Use the same `--work` for both runs of a comparison so both see the
same data. Timings on a shared machine move by 10-30%; compare runs made back to back.

## Notes on the numbers

* Import marks come from the WebSocket (`thumbs.ready` events are coalesced every 100 ms), so
  they are quantised to 100 ms, which is also what a client sees.
* Memory is read with PowerShell `Get-Process` (private bytes and working set) on Windows, `ps` elsewhere.
* Headless Chromium rasterises in software; frame times are pessimistic against a GPU browser and
  meant for before / after comparison.
