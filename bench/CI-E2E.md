# Real-server end-to-end test in CI

`web/scripts/e2e-real.mjs` starts `imagepicker serve` on a temp data directory with the **fake AI
worker** (`ip-fake-worker`, no models, no Python) and synthetic photos, then drives the built web UI
with Playwright:

first-run dialog, import through the folder browser, grid with thumbnails, rate / flag / undo / redo
(keyboard, range selection), analysis run (the fake worker returns deterministic synthetic results
when `IP_FAKE_ANALYSIS=1`), groups and stacks (expand / collapse, scene headers, AI badges), edit
page (exposure slider, preset), before / after and split compare, export of the edited photo and a
check of the file on disk (complete JPEG, brighter than the source).

Dev mode only (no LAN, no password). The run fails on any console error, page error, failed request
or HTTP 5xx. It needs no GPU: it sets `IMAGEPICKER_RENDER=cpu`.

Runtime: about 5 s after the build; the Rust build dominates.

## Inputs

```text
--bin     path to the imagepicker binary      (default <target>/release/imagepicker)
--worker  path to ip-fake-worker              (default <target>/release/ip-fake-worker)
--web-dir built UI                            (default web/dist)
--shots   directory for screenshots           (optional)
--keep    keep the temp data / photos / export dir
```

Build the two binaries with `cargo build --release -p ip-cli -p ip-worker-client --bins`
(`ip-fake-worker` is a bin of `ip-worker-client`; it is test-only and must never be shipped).

## GitHub Actions job (for the release/CI agent to adopt)

```yaml
  e2e-real:
    name: e2e (real server)
    runs-on: ubuntu-latest        # also works on windows-latest / macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: pnpm/action-setup@v4
        with:
          version: 10
          package_json_file: web/package.json
      - uses: actions/setup-node@v4
        with:
          node-version: 24
          cache: pnpm
          cache-dependency-path: web/pnpm-lock.yaml
      - name: Build server, CLI and fake worker
        run: cargo build --release -p ip-cli -p ip-worker-client --bins
      - name: Build web UI
        working-directory: web
        run: pnpm install --frozen-lockfile && pnpm build
      - name: Install Chromium
        working-directory: web
        run: pnpm exec playwright install --with-deps chromium
      - name: End-to-end against the real server
        working-directory: web
        run: node scripts/e2e-real.mjs --shots ../e2e-shots
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: e2e-real-screenshots
          path: e2e-shots
          if-no-files-found: ignore
```

On Linux runners without a GPU the wgpu renderer is not used (`IMAGEPICKER_RENDER=cpu` is set by
the script). If the runner uses another target directory, pass `--bin` / `--worker` or set
`CARGO_TARGET_DIR` for the script as well.

## Optional: performance smoke (not a gate)

`bench/run.mjs` with `BENCH_QUICK=1` finishes in about a minute and prints the key latencies; it is
useful as a manual or nightly job, not as a pass / fail gate (shared runners are too noisy).

```yaml
      - run: BENCH_QUICK=1 node bench/run.mjs --label ci --only cold,query
```
