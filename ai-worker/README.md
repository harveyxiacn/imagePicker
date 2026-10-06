# imagepicker-ai

Python AI worker of imagePicker. The Rust core spawns it and talks to it over WebSocket JSON-RPC 2.0
(protocol: `docs/05-数据与接口设计.md` §4).

## Install

```bash
cd ai-worker
uv sync --extra cuda --extra mediapipe     # NVIDIA GPU (onnxruntime-gpu + CUDA 13 libs from wheels, driver >= 580)
uv sync --extra cpu --extra mediapipe      # CPU only
# extras: cpu | cuda (mutually exclusive), mediapipe (face blendshapes/pose), torch (transformers fallback + ONNX export)
```

## Run

```bash
uv run imagepicker-ai serve --host 127.0.0.1 --port 0 --token T [--models-dir DIR] [--device auto|cpu]
# stdout, single line:  {"event":"ready","port":54321}      (logs go to stderr)
```

Env fallbacks: `IP_WORKER_ADDR` (`host:port`), `IP_WORKER_TOKEN`, `IMAGEPICKER_MODELS_DIR`.
Authenticate with `Authorization: Bearer <token>` or `ws://host:port/?token=<token>`.
`--parent-pid N` makes the worker exit when the parent dies. Default models dir: `<platform data dir>/imagePicker/models`.

## Methods

| method | notes |
|---|---|
| `system.info` | hardware, tier (T0-T3), providers, loaded models, VRAM budget/usage |
| `models.list` | registry + installed/loaded/recommended flags |
| `models.ensure` `{ids:[..]}` | downloads (HF official -> `HF_ENDPOINT` -> hf-mirror), streams `progress` `{req,kind:"model.download",model,file,phase,done,total}` |
| `models.unload` `{id?}` | free VRAM |
| `analyze.batch` | `{items:[{photo_id,path,orientation?}], steps:[phash,quality,faces,embed], analysis_size, out_dir, allow_download?}`; `progress` `{req,kind:"analyze",done,total}` |
| `system.shutdown`, `system.ping`, `cancel {req}` | lifecycle / cancel an in-flight request |

`analyze.batch` does not download models unless `allow_download: true` (error `-32010 model_unavailable`
otherwise). Per-photo failures are reported per item (`error`), never failing the batch.
Artifacts in `out_dir`: `<photo_id>.emb.npy` (float16, L2-normalised, 768-d), `<photo_id>.analysis.json`.

## Develop

```bash
uv run pytest                # fast tests, no downloads
uv run pytest -m models      # needs model weights (downloaded into .models/ on first run) + network
uv run ruff check . && uv run ruff format .
uv run python scripts/bench_analyze.py --synth 200 [--device cpu] [--portrait some_face.jpg]
uv run python scripts/export_siglip2_onnx.py --out .models/export/x   # only for models without an ONNX export
```
