# Contributing

Thanks for your interest in imagePicker! 欢迎贡献。

User-facing docs: [docs/user-guide](docs/user-guide/README.md). Design docs: [docs/README.md](docs/README.md). Known issues and open work: [docs/backlog.md](docs/backlog.md).

## Repository layout

| Path | What |
|---|---|
| `crates/ip-imaging` | Scanning, EXIF, thumbnails, RAW embedded previews |
| `crates/ip-render` | Non-destructive render engine: edit stack (JSON) to pixels, GPU (wgpu) with CPU fallback; LUTs, masks, warp, patches |
| `crates/ip-worker-client` | Spawns and talks to the Python AI worker (JSON-RPC over WebSocket): restarts, timeouts, progress |
| `crates/ip-core` | Catalog (SQLite), import, analysis orchestration, edits, export, assistant, settings, auth, XMP, tasks, events |
| `crates/ip-server` | HTTP + WebSocket API (axum), serves the built web UI |
| `crates/ip-cli` | `imagepicker` binary (`serve`, `import`, `analyze`, `render`, `besttake`, `inpaint`, `ask`) |
| `apps/desktop` | Tauri 2 desktop shell (`src-tauri` is a Cargo workspace member); see [apps/desktop/README.md](apps/desktop/README.md) |
| `web/` | React frontend, used by the desktop app and the LAN WebUI; includes a mock backend (`pnpm dev:mock`) |
| `ai-worker/` | Python AI worker (JSON-RPC 2.0 over WebSocket); see [ai-worker/README.md](ai-worker/README.md) |
| `docs/` | Design docs, user guide, screenshots (`docs/images`), and the per-milestone API contracts `docs/api-contract-m1.md` … `m6.md` (frontend / backend contract) |

## Development setup

Prerequisites: Rust (stable; MSVC toolchain on Windows), Node 24, pnpm 10, and for the AI worker [uv](https://docs.astral.sh/uv/) with Python 3.12. Linux desktop builds also need `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libgtk-3-dev patchelf`.

### Rust crates (core, server, CLI, render, imaging)

```sh
cargo build --workspace
cargo run -p ip-cli -- serve --web-dir web/dist     # API + built web UI on http://127.0.0.1:7878
cargo run -p ip-cli -- --help                        # import / analyze / render / ask ...
```

`serve` accepts `--data-dir` (or `IMAGEPICKER_DATA_DIR`), `--lan` and `--password` / `IMAGEPICKER_PASSWORD`. Useful environment variables: `IMAGEPICKER_WORKER_DIR` / `IMAGEPICKER_WORKER_CMD` (where / how to start the AI worker), `IMAGEPICKER_MODELS_DIR`, `IMAGEPICKER_RENDER=cpu`, `IMAGEPICKER_DEV_CORS=1` (allow the Vite dev origins).

### Web frontend (`web/`)

```sh
cd web
pnpm install
pnpm dev:mock          # SPA against an in-browser mock backend, no Rust or Python needed
pnpm dev               # SPA against a real server (Vite proxies /api to 127.0.0.1:7878)
pnpm build             # type-check + production bundle (web/dist)
pnpm lint && pnpm test
```

Screenshot / smoke scripts use Playwright against the mock backend: `web/scripts/screenshots-m*.mjs`, and `web/scripts/docs-screenshots.mjs` for the images in `docs/images` (run `pnpm dev:mock --port 5481 --host 127.0.0.1` first, then `BASE_URL=http://127.0.0.1:5481 OUT_DIR=<dir> node scripts/docs-screenshots.mjs`). Never use real or personal photos for screenshots.

### Desktop shell (`apps/desktop`)

```sh
cd web && pnpm install && cd ../apps/desktop && pnpm install
pnpm tauri dev                       # Vite dev server + shell with HMR
pnpm tauri build --no-bundle         # compile only (what CI does)
pnpm tauri build                     # installers for the host OS
```

### AI worker (`ai-worker/`)

```sh
cd ai-worker
uv sync --extra cuda --extra mediapipe        # NVIDIA GPU (or: --extra cpu --extra mediapipe)
uv run imagepicker-ai serve --host 127.0.0.1 --port 0 --token T
uv run pytest                                 # fast tests, no downloads
uv run pytest -m models                       # needs model weights + network
uv run ruff check . && uv run ruff format .
```

## Workflow

- Branch from `main`: `feat/<area>-<topic>`, `fix/<area>-<topic>`; open a PR (squash merge).
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `feat(web): ...`, `fix(core): ...`, `docs: ...`.
- Before pushing: `cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace`; in `web/`: `pnpm lint && pnpm test`; in `ai-worker/`: `uv run ruff check . && uv run pytest`.
- User-visible strings live in `web/src/i18n/zh-CN.ts` and `en.ts`; keep both in sync. Keyboard shortcuts are defined once in `web/src/lib/keymap.ts` (the help overlay and settings table are generated from it); update the [user guide](docs/user-guide/README.md) when you change them.
- Never commit personal photos or model weights.

## License

By contributing you agree your contributions are licensed under Apache-2.0.
