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
| `bench/eval-library.py`, `bench/eval-standard.py` | Score an analysed catalog of the synthetic photo library (`bench/data/qwen-photo-library`, labels in its `manifest.json`): issue tags incl. closed eyes, bursts and best frame; `eval-standard.py` adds people clustering vs the actor ids, expression signals, subject / bystander split, scenes, stars and GPU use. Results: [`standard-eval-2026-10-09.md`](standard-eval-2026-10-09.md). |
| `scripts/comfy-photo-library.py`, `scripts/comfy-photo-library-audit.py`, `scripts/test_comfy_photo_library.py` | Generator of the synthetic photo library with a local ComfyUI (Qwen-Image; plan, resumable run, EXIF, audit with pHash burst distances and a contact sheet; the test runs without ComfyUI). Plan, categories and quality limits: [`photo-library-2026-10-08.md`](photo-library-2026-10-08.md). The images stay in the git-ignored `bench/data/`. |
| `bench/eval-search.py` | Scores natural-language search (`GET /api/search`, docs/api-contract-m2.md C.7) on the same library: 20 concepts asked in English and in Chinese (40 queries), relevance from the manifest prompts and tags, precision / recall@20 per query and per language. Needs a running server, the library imported and analysed with `standard`, and the `siglip2-base-text` model (GPU PC); `--dry-run` only lists the queries and their relevant-photo counts. See "Search evaluation" below. |
| `bench/api-features.py` | Drives the AI features of a running `imagepicker serve` end to end (masks, best take, inpaint, denoise, face restore, upscale on export, portrait retouch, assistant plan / execute / undo, describe, face search, XMP round trip) and saves before / after images with numeric checks. |
| `bench/profile-analysis.py` | Stage profiler of the analysis (worker steps per photo, import -> analysis -> export), see "Analysis stage profile" below. |
| `bench/worker-rpc.py`, `bench/besttake-align.py` | Call the AI worker's JSON-RPC methods directly (`system.info`, `models.ensure`, ...); global-alignment statistics of the library's bursts as Best Take computes them (`--legacy` = the pre-2026-10-09 exclusion zone). |

## Analysis stage profile (`profile-analysis.py`): run this on your GPU PC

Where the wait after an import goes: wall time per stage (import, analysis, grouping / clustering /
scoring, export) and per analysis step and photo (decode, faces, identity, quality, phash, the
224 px preparation, NIMA aesthetic / technical, the batched SigLIP2 embedding and its zero-shot
heads), as p50 / p95 / totals and photos/s. Decode is reported on its own, both inside the
pipeline and alone on one thread. CPU-only machines run it too (`--device cpu`); the numbers of
record are in [`perf-baseline-2026-10-16.md`](perf-baseline-2026-10-16.md).

```sh
cargo build --release -p ip-cli
target/release/imagepicker bench gen bench/data/gen12 --count 200      # 12 MP JPEGs, or use your own photos
cd ai-worker && uv sync --extra cuda --extra mediapipe && cd ..       # CPU: --extra cpu
# worker steps (in-process, real models; no server, no network) + import -> analysis -> export
uv run --project ai-worker python bench/profile-analysis.py bench/data/gen12 --limit 200 \
    --profile standard --models-dir <models dir> --label gpu-standard \
    --imagepicker target/release/imagepicker --e2e-profile standard \
    --json bench/results/profile-gpu.json --md bench/results/profile-gpu.md \
    --dump-results bench/results/profile-gpu.items.json
# did an optimisation change any score, tag, face or vector? (exit status 1 if so)
python bench/profile-analysis.py --compare before.items.json after.items.json
# GPU only: does handing photos to the embedding thread in groups of 8 help? (same photos)
uv run --project ai-worker python bench/profile-analysis.py bench/data/gen12 --limit 200 \
    --profile standard --models-dir <models dir> --embed-flush 8 --label gpu-flush8 \
    --json bench/results/profile-gpu-flush8.json --dump-results bench/results/profile-gpu-flush8.items.json
python bench/profile-analysis.py --compare bench/results/profile-gpu.items.json \
    bench/results/profile-gpu-flush8.items.json --tol 1e-3
```

* The worker part needs the worker's environment (`uv run --project ai-worker`) and its models
  (`--models-dir`, else `IMAGEPICKER_MODELS_DIR` or the app's data dir; nothing is downloaded).
  It sends requests of 32 photos like the Core, after a warm-up batch that loads the models (its
  time is reported separately). `--device cpu` forces the CPU; `auto` uses the GPU when there is one.
* `--imagepicker BIN` adds the end-to-end run in a scratch data dir: `imagepicker analyze DIR
  --export OUT --timings-json FILE` imports, analyses and exports every photo with the default
  export settings. `--e2e-profile lite` works without Python and models; `fast` / `standard` start
  the AI worker like the app does (a loopback WebSocket, so not inside sandboxes without one).
* Step times are measured on the worker's decode threads, which run in parallel, so their sum is
  larger than the wall time; the wall time and photos/s are what a user waits for.
* `--compare` diffs the per-photo results including the SigLIP2 and AuraFace vectors (`--tol` for
  floats, `--ignore embedding,identity` against dumps written without them).
* `ai-worker/scripts/bench_analyze.py` is the older single-step benchmark: it runs each step
  alone over the whole set (images/s per step, SigLIP2 forward without decode). This profiler
  runs the real combination, as the app does, and shows how the steps share the machine.

## Analysis quality (synthetic photo library)

These score the analysis itself rather than speed; inputs are the generated photo library in
`bench/data/qwen-photo-library` (git-ignored) and catalogs written by
`imagepicker analyze <dir> --profile lite --data-dir <data dir>`.

| Piece | What it does |
|---|---|
| `bench/eval-library.py LIBRARY DATA_DIR` | Issue-tag precision / recall against the manifest tags, burst grouping, metric distributions, star histogram. |
| `bench/make-tilt-set.py [LIBRARY] [OUT]` | Rotates 247 library photos by 0, +-1, +-2, +-3, +-5, +-8 degrees (border-free centre crop) into `bench/data/tilt-set` with the true angles in its manifest (Pillow). |
| `bench/eval-time-align.py DATA_DIR [--session ID] [--trials N] [--offsets LIST]` | Multi-device time alignment (docs/03 section 2.4): opens an analysed catalog with photos of two or more devices read-only, shifts one device's capture times by known offsets (seconds to hours, in memory) and reports how often the estimator finds the shift again within 5 s, the median / p95 / max error, trials without an estimate and the real clock differences of the library. A numpy port of `analysis/time_align.rs`; run it on the GPU PC next to a real library, it is not part of CI. |
| `bench/eval-tilt.py TILT_SET DATA_DIR [--library LIBRARY DATA_DIR]` | Tilt estimate error, `tilted` flag precision / recall per angle, false positives on the 0-degree copies and the library, and a threshold sweep (docs/03 section 3.1.1). |

## Search evaluation (`eval-search.py`)

```sh
# 1. import bench/data/qwen-photo-library/jpeg as one session and run the *standard* analysis on it
#    (the app or: imagepicker serve ... then POST /api/import and /api/analysis/run), install siglip2-base-text
# 2. with the server still running:
python bench/eval-search.py bench/data/qwen-photo-library --session-id <id> --base http://127.0.0.1:7911 \
    --k 20 --json bench/results/search-eval.json
python bench/eval-search.py bench/data/qwen-photo-library --dry-run      # no server: queries + relevant counts
```

* 20 concepts (beach, snow, cat, dog, noodles, birthday cake, neon street, group photo, ...), each as an English and a Chinese query. A photo is relevant when the English prompt that generated it mentions the concept (regular expression over the scene text, style preamble cut) or its tags say so; prompts are intentions, not verified truth, so absolute recall is underestimated. The numbers compare zh with en and one model / threshold with another.
* Per query: relevant photos, hits in the top K, precision@K, recall@K (hits / relevant) and *capped* recall@K (hits / min(K, relevant); 1.0 = the best possible top K, the figure to compare with the 09 P1-4 target of 0.8). The summary averages each over the en and zh queries. `--min-score` defaults to 0 so the ranking is measured; use `0.05` for what a user sees.
* Exit status: 0 when it ran, 2 when the text model is missing (HTTP 409), 1 on other errors. Standard library only.
* Not run in the Oracle development session that added it (no models, no GPU there): the first numbers are still to be recorded here by whoever runs it on the GPU PC.

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

## Render benchmark (`bench_render`)

`cargo run --release -p ip-render --example bench_render [-- options]` renders a 24 MP synthetic source with a heavy stack (plus patch and portrait stacks) and prints best / p50 / p95 milliseconds per case for the CPU and, if an adapter exists, the GPU. Without options it only reports. Thresholds make it exit with status 1 when exceeded (it is not part of CI; a GPU runner can enforce them):

| Option | Meaning |
|---|---|
| `--runs N` | timed runs per case (default 5 / 3 / 2 depending on the case); use 20 or more for a stable p95 |
| `--max-ms MS` | p95 limit for the "drag" cases on the active backend (GPU if present, else CPU); the target is 50 on a discrete GPU, 150 on an integrated one |
| `--max-ms-portrait MS` | p95 limit for the portrait (warp + beauty) cases; the target is 200 |
| `--max-ms-all MS` | p95 limit for every case of every stack |

The limits apply to the GPU numbers when a GPU is present, otherwise to the CPU numbers, so a CPU-only machine will usually fail a GPU target; pass the thresholds only on the machine they were set for. Exceeded cases are listed on stderr under `THRESHOLD EXCEEDED`.

## Notes on the numbers

* Import marks come from the WebSocket (`thumbs.ready` events are coalesced every 100 ms), so
  they are quantised to 100 ms, which is also what a client sees.
* Memory is read with PowerShell `Get-Process` (private bytes and working set) on Windows, `ps` elsewhere.
* Headless Chromium rasterises in software; frame times are pessimistic against a GPU browser and
  meant for before / after comparison.
