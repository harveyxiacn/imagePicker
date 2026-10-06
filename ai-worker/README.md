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
| `analyze.batch` | `{items:[{photo_id,path,orientation?}], profile?:"fast"\|"standard", steps?:[..], analysis_size, out_dir, allow_download?}`; `progress` `{req,kind:"analyze",done,total}` |
| `mask.generate` | `{photo:{photo_id,path,orientation?}, targets:[..], person_bbox?:[x,y,w,h], size, out_dir, allow_download?}` -> `{masks,models,skipped}`; `progress` `{req,kind:"mask",done,total}` when more than one target runs (see below) |
| `beauty.prepare` | `{photo:{photo_id,path,orientation?}, faces?:[{face_id,bbox}], size, out_dir, allow_download?}` -> `{people,models,skipped,timings}` (below) |
| `faces.embed` | `{path, orientation?, analysis_size?, allow_download?}` -> `{faces:[{bbox,det_score}], embeddings:[[512 floats]]}`: AuraFace, L2-normalised, same decode / YuNet / alignment as `identity`, rows aligned with `faces`, no files |
| `system.shutdown`, `system.ping`, `cancel {req}` | lifecycle / cancel an in-flight request |

### Steps and profiles (contract `docs/api-contract-m2.md` section A)

`profile` picks the step list: `fast` = `phash, quality, faces`; `standard` (default when neither
`profile` nor `steps` is given) = `phash, quality, faces, identity, embed, aesthetic, iqa, scene`.
An explicit `steps` array wins over `profile`. A step whose model is missing is listed in
`skipped_steps` (reason in `warnings`); only `faces`/`embed` fail the whole batch (`-32010`).
`identity` implies `faces`. `models.list` returns `required_for` (steps) per model and a `profiles`
map (`{name: {steps, models}}`: what `models.ensure` must fetch on this machine).

| step | output (per item) | model |
|---|---|---|
| `identity` | per face `identity_index`; `identity_file` = `<photo_id>.faces.npy` float16 `(n_faces,512)` L2-normalised (null / no file when no faces) | `auraface` (`fal/AuraFace-v1` `glintr100.onnx`, Apache-2.0, 249 MB): YuNet 5 landmarks -> similarity transform to the ArcFace 112x112 template |
| `scene` | `scene_type`, `scene_scores` (softmax over `portrait group landscape food architecture night pet other`) | SigLIP2 zero-shot: 5 prompts/class, mean text embedding (cached in `<models>/_cache/`), `softmax(112.67 * cos)` |
| `aesthetic` | `aesthetic` 0-1, `aesthetic_model` | `nima-aesthetic` (NIMA MobileNet on AVA, Apache-2.0, 6 MB, LiteRT) -> `sigmoid((s-5.0)/0.65)` |
| `iqa` | `iqa` 0-1, `iqa_model` | mean of `nima-technical` (TID2013, 6 MB; `sigmoid((s-5.0)/0.9)`) and a SigLIP2 zero-shot CLIP-IQA head (3 good/bad prompt pairs); NIMA only when no image embedding is computed on CPU |
| `faces` (+) | per face `gaze` 0-1 or null | MediaPipe iris landmarks vs eye corners/lids x head yaw/pitch; null without landmarks |

The SigLIP2 image embedding is computed once per photo and shared by `embed`, `scene` and the
zero-shot half of `iqa`. NIMA runs on the CPU in the decode threads (about 35 ms per image per
head per core, so it stays enabled on T0; `embed` is what is slow on CPU there).

`analyze.batch` does not download models unless `allow_download: true` (error `-32010 model_unavailable`
otherwise). Per-photo failures are reported per item (`error`), never failing the batch.
Artifacts in `out_dir`: `<photo_id>.emb.npy` (float16, L2-normalised, 768-d), `<photo_id>.faces.npy` (identity step),
`<photo_id>.analysis.json`.

## AI masks (`mask.generate`, contract `docs/api-contract-m3.md` section D)

Targets `subject sky person skin hair clothes` -> 8-bit grayscale PNG, long edge = `size`, upright (EXIF applied),
soft 0-255 alpha, written to `out_dir/<photo_id>_<target>[_<hash>].png` (`person` with `person_bbox` adds the first 8 hex
of sha1 of the bbox formatted `%.4f,%.4f,%.4f,%.4f`). `models` names the model per target, `skipped` maps a target to
`model_unavailable` (not installed, `allow_download` false), `download_failed`, `out_of_memory`, `failed` or
`unsupported_target`; the other targets are returned normally. Edges of every mask are snapped to the image with a guided
filter on the luminance at output resolution.

| target | model | notes |
|---|---|---|
| `subject` | BiRefNet (MIT): GPU `birefnet-lite-fp16` (optional pack `birefnet-fp16`, used when installed), CPU T1+ `birefnet-lite`, CPU T0 `birefnet-lite-512` | full-frame matting |
| `person` | same BiRefNet | with `person_bbox` (the face box, normalised): matting on a body crop of the face, keep the connected component that covers the face, split touching people with a marker watershed using the other YuNet faces. Without bbox: union of every detected person (falls back to `subject` without faces) |
| `skin` `hair` `clothes` | `mediapipe-selfie-multiclass` (Apache-2.0, via LiteRT) | skin = body-skin + face-skin; faces narrower than 18 % of the long edge are re-run on crops and blended back |
| `sky` | `skyseg-u2net` (MIT repo, training data undisclosed) | without the model a colour/brightness/position/smoothness heuristic is used (`models.sky = "heuristic"`), so `sky` is never skipped |

`models.list` also returns `mask_models` (`{target: [model ids]}`) for `models.ensure`. YuNet is an optional helper for
`person` / `skin` / `hair` / `clothes` (other people, small-person crops).

## Portrait geometry (`beauty.prepare`, contract `docs/api-contract-m4.md` section B)

Per face (given `bbox`es, normalised upright; when `faces` is empty YuNet detects them and `face_id` is `null`;
at most 12 faces) the result carries `face_box`, `face_landmarks` (478 MediaPipe Face Mesh points on a face crop, `[]`
if unavailable), `pose` (33 MediaPipe Pose Landmarker keypoints `[x, y, visibility]` of the body that owns the
face, `[]` if none; coordinates may lie slightly outside 0..1 for cropped limbs), `skin_mask`, `body_mask`
(8-bit PNG, long edge = `size`, upright, `<out_dir>/<photo_id>_<face_id|f<index>>_skin.png|_body.png`, `null` when
the models are missing) and `blemishes` (`[x, y, r]`, r relative to the long edge, at most 24 per face).
`models` names what produced each part, `skipped` maps `face_landmarks | pose | body_mask | skin_mask | blemishes | faces`
to `model_unavailable`, `download_failed`, `out_of_memory`, `failed` or `requires_body_and_landmarks`; `timings` is a
per-stage wall-clock breakdown (seconds, summed over faces).

| part | how |
|---|---|
| landmarks | `mediapipe-face-landmarker` on a square crop (face + 35 % margin); of up to 4 faces in the crop the one nearest the box centre is taken |
| pose | `mediapipe-pose-landmarker-full` (Apache-2.0, 9.4 MB) on a body crop (3.2 face widths either side, 1.2 above, 8 below), up to 4 poses; the pose whose visible head keypoints (nose, eyes, ears, mouth) are nearest the face-box centre (< 0.6 face sizes, nose inside the box +35 %) wins |
| body | BiRefNet person selection of `mask.generate` (`select_person`), plus pose skeletons (shoulder-hip-limb polylines) as extra watershed seeds, so touching people keep their own arms; pixels still claimed by two people go to the nearest seed |
| skin | selfie-multiclass (face-skin + body-skin) x the person's matte, guided filter, minus the eye / eyebrow / outer-lip polygons below (dilated by 1.2 % / 1.0 % / 1.0 % of the face width, feathered) |
| blemishes | DoG blob detector on masked Lab L and a* (see `beauty/blemish.py`): radii 1.2-5 % of the face width, robust z >= 4.5 plus >= 2.5 Lab units contrast, 3x3x3 NMS, rejected on edge / line structure (Hessian eigenvalue ratio), eyes / brows / lips / nostrils (+ margins), hair, non-skin, outside the face oval |

Landmark index sets (`imagepicker_ai/beauty/landmarks.py`, MediaPipe canonical topology, subject's left/right):
`RIGHT_EYE` 33 7 163 144 145 153 154 155 133 173 157 158 159 160 161 246; `LEFT_EYE` 362 382 381 380 374 373 390 249 263 466 388 387 386 385 384 398;
`RIGHT_BROW` 46 53 52 65 55 107 66 105 63 70; `LEFT_BROW` 276 283 282 295 285 336 296 334 293 300;
`LIPS_OUTER` 61 146 91 181 84 17 314 405 321 375 291 409 270 269 267 0 37 39 40 185 (removed from the skin, includes mouth opening / teeth);
`LIPS_INNER` 78 95 88 178 87 14 317 402 318 324 308 415 310 311 312 13 82 81 80 191; `FACE_OVAL` 10 338 297 332 284 251 389 356 454 323 361 288 397 365 379 378 400 377 152 148 176 149 150 136 172 58 132 93 234 127 162 21 54 103 67 109;
`NOSTRILS` (blemish rejection hull) 48 64 98 97 2 326 327 294 278 331 279 360 438 457 237 218 129 358; iris points 468-477.

## Develop

```bash
uv run pytest                # fast tests, no downloads
uv run pytest -m models      # needs model weights (downloaded into .models/ on first run) + network
uv run ruff check . && uv run ruff format .
uv run python scripts/bench_analyze.py --synth 200 [--device cpu] [--profile fast|standard] [--portrait some_face.jpg]
uv run python scripts/bench_beauty.py a.jpg b.jpg [--device cpu] [--side-by-side]   # ms per face of beauty.prepare
uv run python scripts/export_siglip2_onnx.py --out .models/export/x   # only for models without an ONNX export
```
