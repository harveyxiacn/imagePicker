# Contributing

Thanks for your interest in imagePicker! 欢迎贡献。

## Repository layout

| Path | What |
|---|---|
| `crates/ip-imaging` | Scanning, EXIF, thumbnails |
| `crates/ip-core` | Catalog (SQLite), import, tasks, events |
| `crates/ip-server` | HTTP + WebSocket API (axum) |
| `crates/ip-cli` | `imagepicker` binary |
| `web/` | React frontend (desktop + WebUI) |
| `ai-worker/` | Python AI worker (JSON-RPC over WebSocket) |
| `docs/` | Design docs; `docs/api-contract-m1.md` is the frontend/backend contract |

## Workflow

- Branch from `main`: `feat/<area>-<topic>`, `fix/<area>-<topic>`; open a PR (squash merge).
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `feat(web): ...`, `fix(core): ...`.
- Before pushing: `cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace`; in `web/`: `pnpm lint && pnpm test`; in `ai-worker/`: `uv run ruff check . && uv run pytest`.
- Never commit personal photos or model weights.

## License

By contributing you agree your contributions are licensed under Apache-2.0.
