# Changelog

All notable changes to imagePicker are documented here. The project follows
[Semantic Versioning](https://semver.org/). This file is generated from
Conventional Commits by git-cliff (`cliff.toml`); do not edit by hand.

## [Unreleased]

### Features

- **imaging:** Scan, EXIF/RAW/CR3/RAF metadata, DCT-scaled and DC-only JPEG thumbnails ([b0c047f](https://github.com/harveyxiacn/imagePicker/commit/b0c047fbb8f3031dc9869ce2bce9a2e2bf5637bc))
- **imaging:** Bench_thumbs example with synthetic 24MP corpus ([573071c](https://github.com/harveyxiacn/imagePicker/commit/573071cda6c11c7ae4cff1e89371eefafcecf33f))
- **core:** Catalog, import pipeline, thumbnail scheduler, export and event bus ([0919ece](https://github.com/harveyxiacn/imagePicker/commit/0919ece024a6176fc02458cce225f815ecaf7f8c))
- **server:** Axum REST + WebSocket API implementing the M1 contract ([72a58d7](https://github.com/harveyxiacn/imagePicker/commit/72a58d7f70ff8a5a5997c305976df8e9af51371d))
- **cli:** Imagepicker serve and headless import commands ([96467f4](https://github.com/harveyxiacn/imagePicker/commit/96467f4e5d3d66cd525341f510c7f0a3a7195e75))
- **web:** M1 culling SPA with mock backend ([294808c](https://github.com/harveyxiacn/imagePicker/commit/294808ce39f5abca2f4e57aa9f2c5072634b6a50))
- **ai:** Python AI worker with JSON-RPC/WebSocket server, hardware tiers, model registry and analyze pipeline ([ff6a19f](https://github.com/harveyxiacn/imagePicker/commit/ff6a19f8e2c0659c6092e71bc26981e693aac1a4))
- **desktop:** Tauri 2 shell with embedded server, native folder picker, drag-drop, menu and NSIS bundle ([6c8acd9](https://github.com/harveyxiacn/imagePicker/commit/6c8acd961d717733342381bb14aa1da0c93b0c49))
- **ai:** M2 worker steps - identity (AuraFace), scene, aesthetic, iqa, gaze, profiles ([a38700e](https://github.com/harveyxiacn/imagePicker/commit/a38700e50e546e30d1092d06bf9266e7afb2431e))
- **web:** M2 smart analysis UI (analysis, stacks, AI ratings, group view, people, person filter) ([7b76b6b](https://github.com/harveyxiacn/imagePicker/commit/7b76b6bcaa6b101e61526d8e5d649ffc199b67df))
- **server:** M2 routes ([ca11d08](https://github.com/harveyxiacn/imagePicker/commit/ca11d08c251c971836e0bf4678145e802bde5154))
- **cli:** Analyze command ([1f4b53f](https://github.com/harveyxiacn/imagePicker/commit/1f4b53fb90db941c60837fa523951cc2364fe5c6))
- **core:** M2 analysis pipeline, worker client, routes and CLI ([f84e537](https://github.com/harveyxiacn/imagePicker/commit/f84e53774f7faac054f1bb9e524ee3266ca37335))
- **core:** M3 render service, edits/presets/LUT/mask API, edited thumbnails, sync and export ([d995c22](https://github.com/harveyxiacn/imagePicker/commit/d995c22703c8e5ed27c00ec8c88084a7642aede9))
- **ai:** Mask.generate with BiRefNet, MediaPipe multiclass and sky segmentation ([b70dcb5](https://github.com/harveyxiacn/imagePicker/commit/b70dcb54b64c3216dbc5d89efe518e86fdcd4904))
- **render:** CPU+GPU non-destructive render engine, auto adjust, presets, builtin LUTs ([eb60511](https://github.com/harveyxiacn/imagePicker/commit/eb605119a36f1c10e92a301b354ecc45d9799ccc))
- **web:** M3 edit page (retouch UI) with mock renderer, presets, masks, history and sync ([55795c8](https://github.com/harveyxiacn/imagePicker/commit/55795c81ac321a8874c551ef091f8d7f8832e038))
- GET /api/luts; web lists LUTs from the server instead of hardcoded ids ([90daeb1](https://github.com/harveyxiacn/imagePicker/commit/90daeb12b29471bf17948eca3835517b9819d223))
- M4 backend (beauty geometry, profiles, best-of-person, face search, collections, taste) ([7f181e6](https://github.com/harveyxiacn/imagePicker/commit/7f181e68d4409624c717f2e3e8878771453d045a))
- **web:** M4 UI - portrait beauty panel, profiles, people page upgrades, face search, smart collections, taste ([4e56826](https://github.com/harveyxiacn/imagePicker/commit/4e568264d85e7f20c745355821da5dd34af85e2a))
- **ai:** Beauty.prepare (face mesh, pose, person skin/body masks, blemishes) and faces.embed ([6762be6](https://github.com/harveyxiacn/imagePicker/commit/6762be6ff7958eb362db6b0be83a52d4c145a8e9))
- **render:** Portrait retouching (face/body warp field, skin beauty) with CPU/GPU parity ([42c814f](https://github.com/harveyxiacn/imagePicker/commit/42c814fe20c942092a18f5c9158c3d1b94387868))
- **render:** Patch layer compositing before crop with CPU/GPU parity ([c68ba02](https://github.com/harveyxiacn/imagePicker/commit/c68ba02d0c323c0f8518bda0edd688b072d18036))
- **web:** M5 UI - Best Take editor, repair panel (bystanders, brush erase, face restore, denoise, patch layers), export upscale, M5 mock backend ([5b3e594](https://github.com/harveyxiacn/imagePicker/commit/5b3e594eb3dcdf8e2589fd37cb5fddb82a315c33))
- M5 backend (patch assets, best take, inpaint, enhance, upscale export, singleton people, EXIF/ICC export, worker timeouts) ([de112bf](https://github.com/harveyxiacn/imagePicker/commit/de112bf68b6c199cbdc8414970f8ff26652c8992))
- **ai:** M5 best take compositing, LaMa/SDXL inpaint, denoise / face restore / upscale ([6e3db56](https://github.com/harveyxiacn/imagePicker/commit/6e3db5660524c4f6d4cb0a720df4ace11485b857))
- **security:** API auth, LAN login with guest role, path whitelist, same-origin CORS ([14abb0b](https://github.com/harveyxiacn/imagePicker/commit/14abb0b9627c05948194a8dfdf8f3eedc3834beb))
- **web:** M6 UI - AI assistant drawer, settings, LAN login and roles, onboarding, XMP conflicts ([b2e36f1](https://github.com/harveyxiacn/imagePicker/commit/b2e36f1815f11ece2635d21edf3785d79864b97f))
- **m6:** AI assistant, settings, cache, XMP interop and ask CLI ([acccd52](https://github.com/harveyxiacn/imagePicker/commit/acccd52bd4c3f02b29b6c82cec2d56e105512d6b))
- **ai:** Local LLM/VLM assistant (llm.plan, vlm.suggest, vlm.describe) on onnxruntime-genai, models.delete ([a814916](https://github.com/harveyxiacn/imagePicker/commit/a814916509f9da24ec989e129bedb93dafeea70f))

### Bug fixes

- Show capture time in the capture time zone, clean EXIF float widening ([3cb7460](https://github.com/harveyxiacn/imagePicker/commit/3cb7460630b80e0a1b27c880c1a869a60b2b6890))
- **web:** Unique keys for repeated scene headers, stable virtualizer row keys ([93a6474](https://github.com/harveyxiacn/imagePicker/commit/93a6474fe10716fd9456c6b939185b91dffaee73))
- **core:** Avoid fetch_update, deprecated on newer stable toolchains (CI) ([2dcc501](https://github.com/harveyxiacn/imagePicker/commit/2dcc5011c13f856570eb591f6bd2fa9979510d28))
- Auto-adjust suggestions serialise cleanly (round to slider steps, no f64 widening) ([1387974](https://github.com/harveyxiacn/imagePicker/commit/138797475980357e1d679fa4871fb54360435b07))
- **core:** Model preflight uses the worker's exact per-operation lists ([1e71097](https://github.com/harveyxiacn/imagePicker/commit/1e71097e1679175b52ba22fca3a23ba8f636a03a))
- **core:** Assistant asks for confirmation before batch edits on many photos ([ec0b7f7](https://github.com/harveyxiacn/imagePicker/commit/ec0b7f7f04ebe4de1c345d8ac6329c046844c58f))

### Refactor

- **core:** Delegate built-in LUTs to ip-render; enable real-renderer tests ([f5dee91](https://github.com/harveyxiacn/imagePicker/commit/f5dee917a6e283a2ea2e72c5a809a46095ef4de1))

### Documentation

- Initial design documents and project plan ([0bcd827](https://github.com/harveyxiacn/imagePicker/commit/0bcd82758429c854528646f1b2704e5608f7c4b7))
- Face-based filtering design, face-data privacy, catalog backup, decoding stack and consistency fixes ([23dc250](https://github.com/harveyxiacn/imagePicker/commit/23dc25006b4041978a0b7d45dcf74a52d5a0a84e))
- M2 API contract (analysis, groups, faces, people) ([1e7afe1](https://github.com/harveyxiacn/imagePicker/commit/1e7afe1cad7df2fb358dc2d42f27cb1938587be6))
- Record models actually adopted and NIMA training-data licence caveat ([6f2623e](https://github.com/harveyxiacn/imagePicker/commit/6f2623e180cb7709805b07ab458d8b4cc803b623))
- Record M3 segmentation models and licence notes ([e51e713](https://github.com/harveyxiacn/imagePicker/commit/e51e71346727571b322f5a839ee838aeba59a4c8))
- Update backlog after M5 backend ([36527eb](https://github.com/harveyxiacn/imagePicker/commit/36527ebe853259e210c1b08e2681569822b83cad))
- Backlog items from the M5 real-photo run ([0f5f057](https://github.com/harveyxiacn/imagePicker/commit/0f5f057e96faf57cf9a6ed2d095b752c3c942301))
- M6 contract (assistant, LAN auth, XMP, settings) ([033f4ab](https://github.com/harveyxiacn/imagePicker/commit/033f4abf92fcd81b73c37fc789ada540f79aa66f))

### Tests

- **web:** E2e smoke script against a real imagepicker server ([495e3b8](https://github.com/harveyxiacn/imagePicker/commit/495e3b817c77910de5cb48ce94c5bec5b48fcfdf))
- **core:** Use a genuinely unknown op type now that warp/beauty are known ops ([9d875d6](https://github.com/harveyxiacn/imagePicker/commit/9d875d66e78393e3d81486a7395129cb9c368cfc))
- **core:** Fix sync test expectation for the unknown-op example ([5acf55c](https://github.com/harveyxiacn/imagePicker/commit/5acf55cdb13885f076259f76c455c50a20e3b1e4))
- De-flake preview render-count test; accept beauty_models in models.list ([c9509e9](https://github.com/harveyxiacn/imagePicker/commit/c9509e9afe649bc9b8072a76b559d8618ff5d918))

### CI and build

- GitHub Actions for rust/web/ai-worker, issue templates, contributing guide ([eebcbe0](https://github.com/harveyxiacn/imagePicker/commit/eebcbe0f7ce3d09ffe324a19fb17219e904eea1c))
- Drop invalid job-level hashFiles conditions ([1ffb8f2](https://github.com/harveyxiacn/imagePicker/commit/1ffb8f25ffb9a3d654bf063a0a2df782caa06938))

### Miscellaneous

- Cargo workspace skeleton, ip-imaging contract and M1 API contract ([58b8009](https://github.com/harveyxiacn/imagePicker/commit/58b800942ae9f171beb4b265e604ba1ba3ca52fc))
- Ignore local .claude directory ([008fdd0](https://github.com/harveyxiacn/imagePicker/commit/008fdd075f0c76f63c907fc3c3c8ea2487f6403f))
- Ip-render contract (edit stack types, renderer API) and M3 API contract ([6a0437e](https://github.com/harveyxiacn/imagePicker/commit/6a0437ee77f375b559bed5eac2ad7fe70690cedf))
- M4 contract (beauty/warp ops, person geometry, beauty.prepare, M2 remainder APIs) ([08986a2](https://github.com/harveyxiacn/imagePicker/commit/08986a2b2fdbf4032ff501edd0fdbf1f9f607c57))
- Update Cargo.lock for ip-server -> ip-render dependency ([780003f](https://github.com/harveyxiacn/imagePicker/commit/780003faa5a6f02b113e6344e1ab0746581d963b))
- M5 contract (patch op, best take, inpaint, enhance) and backlog doc ([8234e26](https://github.com/harveyxiacn/imagePicker/commit/8234e2682f269a6493c6ef67b504cd64f4a67aee))

