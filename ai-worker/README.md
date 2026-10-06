# imagepicker-ai

Python AI worker of imagePicker. The Rust core spawns it and talks to it over WebSocket JSON-RPC 2.0
(protocol: `docs/05-数据与接口设计.md` §4).

## Install

```bash
cd ai-worker
uv sync --extra cuda --extra mediapipe     # NVIDIA GPU (onnxruntime-gpu + CUDA 13 libs from wheels, driver >= 580)
uv sync --extra cpu --extra mediapipe      # CPU only
# extras: cpu | cuda (mutually exclusive), mediapipe (face blendshapes/pose), torch (transformers fallback + ONNX export),
#   llm-cuda (pairs with cuda) | llm (pairs with cpu): local assistant, onnxruntime-genai
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
| `models.delete` `{id}` | remove a downloaded model's files (unloads it first) -> `{deleted:true, freed_bytes}`; unknown id -> -32602 |
| `llm.plan` | `{message, tools:[JSON schema], context?, locale?}` -> `{reply, calls:[{tool,args}]}` (assistant section below) |
| `vlm.suggest` | `{photo:{photo_id,path,orientation?}, context?:{histogram,scores,scene_type}, locale?}` -> `{problems, adjust, reason}` |
| `vlm.describe` | `{photo, locale?}` -> `{caption, keywords}` |
| `analyze.batch` | `{items:[{photo_id,path,orientation?}], profile?:"fast"\|"standard", steps?:[..], analysis_size, out_dir, allow_download?}`; `progress` `{req,kind:"analyze",done,total}` |
| `mask.generate` | `{photo:{photo_id,path,orientation?}, targets:[..], person_bbox?:[x,y,w,h], size, out_dir, allow_download?}` -> `{masks,models,skipped}`; `progress` `{req,kind:"mask",done,total}` when more than one target runs (see below) |
| `beauty.prepare` | `{photo:{photo_id,path,orientation?}, faces?:[{face_id,bbox}], size, out_dir, allow_download?}` -> `{people,models,skipped,timings}` (below) |
| `besttake.compose` | `{base:{photo_id,path,orientation?}, source:{..}, base_face:[x,y,w,h], source_face:[x,y,w,h], out_dir, allow_download?}` -> `{patch, rect, quality:{score,aligned,warnings}}` or `{patch:null, reason}` (M5 below) |
| `inpaint.run` | `{photo, mask, model?:"lama"\|"sdxl", out_dir, allow_download?}` -> `{patch, rect, model}` |
| `enhance.run` | `{photo, op:"denoise"\|"face_restore"\|"upscale", strength?, scale?:2\|4, faces?, out_dir, allow_download?}` -> `{patch,rect}` \| `{patches:[{patch,rect,face_index}]}` \| `{image}` |
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

## Generative methods (M5, contract `docs/api-contract-m5.md` section B)

All three work on the full-resolution upright photo and write RGBA PNG patches (alpha = feathered mask)
into `out_dir`; `rect` is normalised in the upright, uncropped base image. Missing models -> `-32010`
(`error.data.detail.models` lists the registry ids; `models.list` returns `besttake_models`,
`inpaint_models`, `enhance_models` for `models.ensure`). Big models run on one worker thread; ONNX models are
LRU / idle-unloaded by the `ModelManager`; on OOM the tile is halved down to 128 px, then the model is reloaded
on the CPU (per request), then `-32012`.

`besttake.compose` (doc 03 section 5): face landmarks + head pose (MediaPipe, pose matrix) of both faces;
pose delta (yaw, pitch) > 25 deg -> `reason: large_pose_change` (> 15 deg only warns). Global alignment source -> base
on a 1280 px copy with ORB (AKAZE fallback when this OpenCV build has it) + RANSAC homography (faces masked out), ECC
refinement; rejected as `camera_moved` for < 14 inliers / < 20 % inlier ratio / median residual > 3 px / shift > 12 % /
scale > 15 % / rotation > 8 deg / background NCC < 0.55. Local: robust similarity from stable landmarks (no chin arc,
eyes, mouth) inside the face, blended into the homography over 0.45 face widths, plus DIS flow (clamped to 3 % of the
face width, interpolated across eyes / brows / mouth). Region = face + hair + neck of both frames (selfie-multiclass x
BiRefNet person matte), ellipse-limited, other detected faces cut out. LAB mean/std match on the boundary band +
low-frequency residual field, Laplacian pyramid blend (2-6 levels from the face width). Quality: landmarks re-detected
on the aligned source (`seam` warning > 3 % of the face width, not composable > 10 %), gradient discontinuity and luma
misfit on the seam ring, occlusion (face-skin fraction < 0.5, oval outside the frame, another face over it) ->
`quality.score`, `warnings`. Required models: `mediapipe-face-landmarker`, `mediapipe-selfie-multiclass`, BiRefNet;
`yunet` is optional (other people).

| method | model (registry id) | notes |
|---|---|---|
| `inpaint.run` lama | `lama-big-fp32` (Carve/LaMa-ONNX, Apache-2.0, 198 MB) | 512 tile around each mask cluster with >= 96 px / 40 % context (native resolution for small holes, resized for big ones), ring colour correction, grain re-added, feathered paste |
| `inpaint.run` sdxl | `sdxl-inpaint` (OpenRAIL++, 6.7 GB fp16, exclusive group `diffusion`) | needs `uv sync --extra pro`; resident when the VRAM budget allows, else CPU offload (also after one OOM); without torch/diffusers -> `-32010` naming `sdxl-inpaint` |
| denoise | `scunet-color-real-psnr` (Apache-2.0, 73 MB) | 512 tiles, 32 px feathered overlap; `strength` blends with the input (omitted: ISO-adaptive from a noise estimate) |
| face_restore | `gfpgan-v1.4` (Apache-2.0, 325 MB) + `mediapipe-face-landmarker` | FFHQ 5-point 512 alignment, oval feathered paste, `strength` (default 0.8) blended; large faces keep the photo's own detail band, faces < 48 px are skipped (`skipped` list) |
| upscale | `realesrgan-x2[-fp16]`, `realesrgan-x4[-fp16]` (BSD-3-Clause, 66 / 34 MB) | fp16 on GPU; 512 tiles + 16 px context; output <= 400 MP; `strength` blends with bicubic |

## Local AI assistant (`llm.plan`, `vlm.suggest`, `vlm.describe`, contract `docs/api-contract-m6.md` A.3)

Runtime: **onnxruntime-genai** (`uv sync --extra cuda --extra llm-cuda`, CPU: `--extra cpu --extra llm`). It ships
wheels for Windows / Linux / macOS (CUDA: Windows / Linux), needs no compiler, no torch, and
`onnxruntime-genai-cuda` depends on `onnxruntime-gpu`, so it shares the venv with the `cuda` extra (the extras
are declared as conflicting pairs because the wheels share the `onnxruntime` package name). It also provides
llguidance-based JSON-schema constrained decoding. The `llm-cuda` wheels pick up the CUDA 13 libraries of the `cuda`
extra through `onnxruntime.preload_dlls()`.

| role | registry id | model | size | licence | notes |
|---|---|---|---|---|---|
| LLM (default, CUDA) | `phi-4-mini-genai-int4-cuda` | Phi-4-mini-instruct 3.8B int4 (Microsoft ONNX export) | 3.35 GB | MIT | fp16 CUDA build, ~250 tokens/s |
| LLM (CPU build, better Chinese) | `qwen3-4b-instruct-genai-int4` | Qwen3-4B-Instruct-2507 int4 (`robinsmits/qwen3-4b-instruct-2507-onnx-int4`) | 2.45 GB | Apache-2.0 | fp32 CPU build: ~18 tokens/s even through the CUDA EP; used for translation and as the non-CUDA LLM |
| VLM (CUDA) | `phi-3.5-vision-genai-int4-cuda` | Phi-3.5-vision-instruct 4.2B int4 (Microsoft ONNX export) | 2.45 GB | MIT | |
| VLM (CPU) | `phi-3.5-vision-genai-int4-cpu` | same, CPU build | 3.05 GB | MIT | slow, see below |

All are `optional: true`, `required_for: [assistant, ...]`, exclusive group `diffusion` (never resident together with
SDXL or each other). `models.list` returns `assistant_models: {"llm": [id], "vlm": [id]}` (what `models.ensure` has to
fetch on this machine; `[]` below the tier), `system.info` returns `llm_available` / `vlm_available` (+ `assistant`
with `runtime`, `llm_model`, `vlm_model`). Tiers: LLM T2+ (T1 only with a GPU), VLM T2+ (`IMAGEPICKER_ASSISTANT_ANY_TIER=1`
overrides). Missing extra / tier / model -> `-32010` with `error.data.detail = {models:[ids], reason:
runtime_missing | tier_too_low | not_installed}`; `allow_download: true` fetches on demand. Optional params:
`model` (force a registry id), `timeout_s` (generation deadline, default 60 s LLM / 120 s VLM, hard cap 600 / 900),
`max_tokens`. Timeout -> `-32013`, unusable output / load failure -> `-32014`.

* `llm.plan`: system prompt = rules + compact signatures of the supplied tools + context (filter, selection ids,
  current photo) + zh/en few-shot turns (only those that validate against the supplied schemas) + Chinese glossary.
  Output is grammar-constrained to `{"calls":[{"tool":<const>,"args":<tool schema>}],"reply"}` (one `anyOf` branch per
  tool), parsed with a repairing JSON extractor, validated against the tool schemas, one repair retry with the
  validation errors, optional free-text arguments the user never wrote (e.g. a made-up `dest`) are dropped.
  Unknown tools / invalid args never leave the worker. Nothing is executed. Extra result fields: `model`,
  `repaired`, `warnings`, `latency_ms`.
* `vlm.suggest`: 768 px image + measured luma / clipping / colour statistics + the caller's context. The 3-4B VLM
  fills sliders with filler values and misses colour casts, so the measurable parts are rule-based: `problems`
  about exposure / contrast / casts and the main sliders come from the measurements (`facts.baseline_adjust`), the
  VLM adds other problems (noise, blur, haze, ...), a slider only when it asks for >= 15 units, and the reason.
  `adjust` has the 12 scalar fields of `ip-render` `Adjust` (exposure -5..5 EV, temp -3000..3000 K, rest
  -100..100; clamped), `source: "ai_vlm@1"`; `curve` / `hsl` / `grading` are never suggested.
* `vlm.describe`: the VLM answers in English; for another locale the text LLM translates (Qwen3 preferred; zh output
  is grammar-forced to start with CJK). Without an LLM the English result is returned with `locale: "en"`
  (result field `locale` states the language actually returned).
* Decoding is greedy (temperature 0, top-k 1), bounded by `max_tokens` and the deadline (checked per token).

Measured on RTX 4090 / driver 616 / Windows 11 (CUDA 13, onnxruntime-genai 0.17.1): `llm.plan` Phi-4-mini avg 0.28-0.32 s
(max 0.6 s) over the 15 commands, 13/15 expected tool calls (Qwen3-4B: 14/15, avg 11.9 s); `vlm.suggest` 1.1-1.3 s,
`vlm.describe` en 0.6 s, zh 7 s (VLM -> Qwen3 translation incl. both model swaps); VRAM (nvidia-smi delta incl. CUDA
context) Phi-4-mini ~6.0 GB, Phi-3.5-vision ~8.5 GB, Qwen3 ~5.5 GB. CPU only: `vlm.suggest` / `describe` take 40-90 s
(2.6k image tokens through the CPU build), `llm.plan` with Qwen3 exceeds 60 s because of the ~1.7k-token prompt: usable
for on-demand descriptions, not for interactive planning (Core falls back to its rule engine).

## Develop

```bash
uv run pytest                # fast tests, no downloads
uv run pytest -m models      # needs model weights (downloaded into .models/ on first run) + network
uv run ruff check . && uv run ruff format .
uv run python scripts/bench_analyze.py --synth 200 [--device cpu] [--profile fast|standard] [--portrait some_face.jpg]
uv run python scripts/bench_beauty.py a.jpg b.jpg [--device cpu] [--side-by-side]   # ms per face of beauty.prepare
uv run python scripts/bench_assistant.py [--llm ID] [--vlm ID] [--device cpu]   # llm.plan x15 commands, vlm.suggest/describe
uv run python scripts/bench_m5.py [--device cpu] [--runs 3]   # ms: besttake, inpaint 512 mask, denoise, face_restore, upscale x2
uv run python scripts/export_siglip2_onnx.py --out .models/export/x   # only for models without an ONNX export
```
