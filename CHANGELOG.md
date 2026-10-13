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
- **runtime:** Installable AI runtime (uv-managed Python + worker), launch resolution, /api/runtime, web UI, fetch-uv script ([ce543f7](https://github.com/harveyxiacn/imagePicker/commit/ce543f72fc991ad8a08b4138e736b739f5c52fb0))
- **runtime:** Byte-based install progress, prune uv cache, server tests, CI fetches uv ([d21ccbc](https://github.com/harveyxiacn/imagePicker/commit/d21ccbcaf2f90c3deb7575afd322d8ac1ec38e90))
- **android:** Tauri 2 Android shell, imagepicker-media plugin (Rust + Kotlin), platform adapter, CI job ([4e57264](https://github.com/harveyxiacn/imagePicker/commit/4e57264a94e46add47188a530787895f391ff1e6))
- **web:** Mobile UX - bottom nav, swipe cull, quick cull, album import, edit drawer, remote AI pairing ([20eff93](https://github.com/harveyxiacn/imagePicker/commit/20eff9348e04f7512dc1f7553be5d73556b43911))
- **android:** Expose mobile platform methods (albumCoverUrl via asset protocol, flat media aliases) ([4dfe1c5](https://github.com/harveyxiacn/imagePicker/commit/4dfe1c5a5d0c670e970063939802ad153c90fe32))
- **remote:** Remote AI host endpoints, device pairing and RemoteWorker ([9c9664e](https://github.com/harveyxiacn/imagePicker/commit/9c9664ec4c224ff2562ea921720f565992e66324))
- **remote:** Align status/connect shapes and error codes with the mobile UI ([9c78c51](https://github.com/harveyxiacn/imagePicker/commit/9c78c51507ca1806ecab5193d0b7977d5a0d5651))
- **core:** Android-ready core and on-device lite analysis ([f772265](https://github.com/harveyxiacn/imagePicker/commit/f7722654e2ae4763e677533587c21b88621833d1))
- **infer:** Ip-infer crate with YuNet face detection; optional faces step for the lite profile ([296daf3](https://github.com/harveyxiacn/imagePicker/commit/296daf31fbee2af576760a45944944a682611863))
- **web:** Device filter (camera/phone), device kind in inspector, mock devices ([68fe5f4](https://github.com/harveyxiacn/imagePicker/commit/68fe5f42bdaa023e8a5c02441f90c9e8ebf06a13))
- **core,server:** Normalised camera names, device kinds, session devices endpoint and device filter ([9d46c52](https://github.com/harveyxiacn/imagePicker/commit/9d46c526168d4e99929bcc40670418d16ffcef89))
- **scoring:** Calibrate issue tags on the synthetic photo library ([c9fef3b](https://github.com/harveyxiacn/imagePicker/commit/c9fef3b1c39c759a879e0cae39d02bfc04010628))
- **grouping:** Look-back linking and gap filling for burst segmentation ([63a8ffe](https://github.com/harveyxiacn/imagePicker/commit/63a8ffe3e457846af24f55d4c3813d3e10566fb1))
- **export:** Convert wide-gamut sources to sRGB, keep XMP/IPTC and original EXIF ([fff3fe1](https://github.com/harveyxiacn/imagePicker/commit/fff3fe1160ee5e32a13b32a693ff56965f16918b))
- **imaging:** HEIC/HEIF thumbnails via embedded JPEG, OS codecs and runtime-loaded libheif ([85c53a4](https://github.com/harveyxiacn/imagePicker/commit/85c53a4602ab453e2b32769bd93d921fa7a4e2ac))
- **imaging:** Colour-manage HEIF pixels to sRGB; honour the clean aperture ([494e964](https://github.com/harveyxiacn/imagePicker/commit/494e96475cb3e568d9429a8cb0ceb4ce9a3561d8))
- **besttake:** Choose the base frame that needs the least compositing ([09b6734](https://github.com/harveyxiacn/imagePicker/commit/09b67347dfc188ee1649197d0eadb8da9cd2c2c9))
- **tasks:** Cancel generation tasks and keep a task history ([5c7f6c9](https://github.com/harveyxiacn/imagePicker/commit/5c7f6c91c76a6b3ab2b6b75566ae9ae0c6c35ae1))
- **analysis:** Horizon tilt detection for every profile ([52a4d76](https://github.com/harveyxiacn/imagePicker/commit/52a4d76d472a6700021a5c1548f8c8af5fea0fbe))
- **people:** "this is a new person" from search by face ([2d48908](https://github.com/harveyxiacn/imagePicker/commit/2d48908803889b2cc49ad537d5e2a57f209fa8ac))
- **export:** JPEG/PNG/TIFF/WebP outputs, shared presets, GPS/EXIF stripping, name tokens ([4c63e42](https://github.com/harveyxiacn/imagePicker/commit/4c63e42cd059194b9876cd2a6bdfe64fdbc65eea))
- **viewer:** Show the original's native pixels at 100% in the loupe ([eec7235](https://github.com/harveyxiacn/imagePicker/commit/eec7235b1f2573f7e683af26a5386d169e8cd4b6))
- **compare:** 2/3/4-up layouts and cells taken from the selection ([b3f3211](https://github.com/harveyxiacn/imagePicker/commit/b3f321130736fe85073ac558a4ca319ebd71512e))
- **assistant:** Justify plan targets, confirm large changes, report LLM fallbacks ([cba2ab4](https://github.com/harveyxiacn/imagePicker/commit/cba2ab489ac0a54212467966a34cb2c1a2ba68b8))
- **web:** RAW+JPEG badge, import exclusions, missing files, relocate and safe reject ([0de9798](https://github.com/harveyxiacn/imagePicker/commit/0de9798b45c7c08ed96688b4b0a7a38d84454d7e))
- **imaging:** Glob and folder excludes for scans, skip _rejected folders ([51fbdaa](https://github.com/harveyxiacn/imagePicker/commit/51fbdaa3b960212a33bff787090b8f9fc4f3576a))
- **core:** RAW+JPEG pairing, import exclusions, file watching, missing files, safe reject ([17798e4](https://github.com/harveyxiacn/imagePicker/commit/17798e444e689f3097d83d7b7ee40e90060fe0b6))
- **core:** Resumable, pausable analysis with lite fallback and per-step increments ([789ff9a](https://github.com/harveyxiacn/imagePicker/commit/789ff9a0af922a72c44a81474ee7bd6ea94a2a06))
- **server:** Analysis pause, continue and plan routes; resume open runs at startup ([79cfabe](https://github.com/harveyxiacn/imagePicker/commit/79cfabe35fb1d82369f630d2709e2ad4b3b752ca))
- **web:** Pause/continue, quick-analysis fallback and incremental-step hint ([74c56fa](https://github.com/harveyxiacn/imagePicker/commit/74c56fa98e4e50b0d1eb6b722e317caec7c572d3))

### Bug fixes

- Show capture time in the capture time zone, clean EXIF float widening ([3cb7460](https://github.com/harveyxiacn/imagePicker/commit/3cb7460630b80e0a1b27c880c1a869a60b2b6890))
- **web:** Unique keys for repeated scene headers, stable virtualizer row keys ([93a6474](https://github.com/harveyxiacn/imagePicker/commit/93a6474fe10716fd9456c6b939185b91dffaee73))
- **core:** Avoid fetch_update, deprecated on newer stable toolchains (CI) ([2dcc501](https://github.com/harveyxiacn/imagePicker/commit/2dcc5011c13f856570eb591f6bd2fa9979510d28))
- Auto-adjust suggestions serialise cleanly (round to slider steps, no f64 widening) ([1387974](https://github.com/harveyxiacn/imagePicker/commit/138797475980357e1d679fa4871fb54360435b07))
- **core:** Model preflight uses the worker's exact per-operation lists ([1e71097](https://github.com/harveyxiacn/imagePicker/commit/1e71097e1679175b52ba22fca3a23ba8f636a03a))
- **core:** Assistant asks for confirmation before batch edits on many photos ([ec0b7f7](https://github.com/harveyxiacn/imagePicker/commit/ec0b7f7f04ebe4de1c345d8ac6329c046844c58f))
- **desktop:** Asset protocol enabled with an empty scope on desktop ([2069c86](https://github.com/harveyxiacn/imagePicker/commit/2069c86e938d30a79adbaf4b9b40f04082d7a4dd))
- **android:** Pad the WebView by system bars and cutout; ci: setup-android without the retired tools package ([6f44772](https://github.com/harveyxiacn/imagePicker/commit/6f44772dea4c23cbec0a64647f49dc622f824a26))
- **core:** Wait for the GPU warm-up thread before Core shutdown ([d51693e](https://github.com/harveyxiacn/imagePicker/commit/d51693eed2b5cf361240c426fd3119f311fb4e78))
- **worker-client:** Kill and probe worker processes natively on Windows ([8871158](https://github.com/harveyxiacn/imagePicker/commit/8871158d49ebab2e4e28840454c673c7c8410cef))
- **imaging:** Use as_chunks in the macOS ImageIO pixel copy ([14f1475](https://github.com/harveyxiacn/imagePicker/commit/14f1475b81ccabe3d4e2eadb21732d1a25711153))
- **imaging:** Never hand HEVC/AV1 items without a decoder configuration to a decoder ([4175eb7](https://github.com/harveyxiacn/imagePicker/commit/4175eb775d5ee7cc0d9f1b6afe963d7e6ae49746))
- **imaging:** Treat HEIF files without a primary image as undecodable ([8f8fabe](https://github.com/harveyxiacn/imagePicker/commit/8f8fabeb049a8f02acbbff2a4104d514156e46a3))
- **data-safety:** Read-only originals, catalog backup/restore, face-recognition consent ([9bf3374](https://github.com/harveyxiacn/imagePicker/commit/9bf337468d3ef982012516e6600260675a3c75c8))
- **worker:** Start OpenCV single-threaded to stop thread-pool crashes on Windows ([33198aa](https://github.com/harveyxiacn/imagePicker/commit/33198aa0b1be65eca63b463ca89ba0e27a45cea9))
- **tasks:** Report cancelling for a cancelled task that is still writing its final state ([76fe25f](https://github.com/harveyxiacn/imagePicker/commit/76fe25f111c75386c8e7c4fdba6ab3c857acf8c2))
- **inpaint:** Fall back to the body box when a bystander's person mask is empty ([c96f716](https://github.com/harveyxiacn/imagePicker/commit/c96f7160e64ae88eae527efb66c22c95ac261c74))
- **assistant:** Recognise the worker's llm_plan / vlm_* model tasks ([ab2f2c4](https://github.com/harveyxiacn/imagePicker/commit/ab2f2c4be7192c4122b3a6c8cf5a30454256cc16))
- **scoring,besttake:** Keep group-photo members as subjects and align group photos ([1c6ed47](https://github.com/harveyxiacn/imagePicker/commit/1c6ed47ae2b4e6885f13213897947eb3e51f2912))

### Performance

- **core,server:** Two-step photo pages, cached totals, lazy GPU init, batched label writes, JSON fast path; bench harness ([fdfd2ac](https://github.com/harveyxiacn/imagePicker/commit/fdfd2ac456a7ff9742a68652f901c3f4bdcdd835))
- **web:** Instant loupe layers for preloaded photos, deeper decoded preload, i18n-free grid cells; real-server perf scripts ([53b9b2a](https://github.com/harveyxiacn/imagePicker/commit/53b9b2a90bdf91c0b96c6ea9e2bf5b4c77188f4a))
- M7 benchmark report, harness docs and results; clippy fixes in bench ([541a5f1](https://github.com/harveyxiacn/imagePicker/commit/541a5f1b73ffaf29260eaf290cdcc3577333f6d6))

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
- Git-cliff config, generated CHANGELOG and release guide ([2b2a981](https://github.com/harveyxiacn/imagePicker/commit/2b2a981f1a0a89a0cf1aa03ceb5418f5e3c49e70))
- Bilingual README, user guide (zh-CN/en), screenshots, contributing layout ([7538fb2](https://github.com/harveyxiacn/imagePicker/commit/7538fb2873d5ec09e948d972a41ec72e0691d16e))
- AI components are installed from inside the app ([d20b951](https://github.com/harveyxiacn/imagePicker/commit/d20b951f0edc85b798572c92321b993e05f360db))
- Backlog after M7 ([08aff1c](https://github.com/harveyxiacn/imagePicker/commit/08aff1cd80ac3dee165eb31906ba07e302a5f9f8))
- M8 Android contract (media plugin, lite analysis, remote AI, mobile UI) ([f78af77](https://github.com/harveyxiacn/imagePicker/commit/f78af772f5310e27d15539aac0f26fd8e9fd2632))
- First-open steps for unsigned installers (SmartScreen, Gatekeeper) ([b238671](https://github.com/harveyxiacn/imagePicker/commit/b23867198fef58dfebeb66e35210e7c0fd98908e))
- **bench:** Lite-profile calibration pass on the synthetic set; EXIF JPEG converter for the set ([a88daec](https://github.com/harveyxiacn/imagePicker/commit/a88daec3d64dda57597fad49b830fba73c7dba82))
- Completion review and finishing plan (09) ([eedbd28](https://github.com/harveyxiacn/imagePicker/commit/eedbd28bcfb377ee33ea148eba93685dac6dfbce))
- **export:** Document formats, presets API, strip options and name tokens ([bb31060](https://github.com/harveyxiacn/imagePicker/commit/bb31060298cc41c3eef8b8ffafc68b4616c0ff83))
- User guide and backlog for native-pixel zoom and the compare layouts ([636febf](https://github.com/harveyxiacn/imagePicker/commit/636febff73699ea35b9f5d9d2a0b5f6ea073b01b))
- RAW+JPEG pairs, import exclusions, file watching, relocate and safe reject ([58bc09c](https://github.com/harveyxiacn/imagePicker/commit/58bc09c2283f85e2d43773940566fe4873263a82))
- Sync changelog, completion review and backlog with merged PRs ([a623c47](https://github.com/harveyxiacn/imagePicker/commit/a623c471b459ed0b5650abec95d0641ce637f082))
- Analysis resume, pause/continue, lite fallback and incremental steps ([3087b37](https://github.com/harveyxiacn/imagePicker/commit/3087b37fd51d91fcc34cfde518e56df7f76d258d))
- Regenerate the changelog after #15; README no longer says HEIC is unavailable ([ba5b339](https://github.com/harveyxiacn/imagePicker/commit/ba5b339fc92856bb7025e086ce80faede1413ef0))

### Tests

- **web:** E2e smoke script against a real imagepicker server ([495e3b8](https://github.com/harveyxiacn/imagePicker/commit/495e3b817c77910de5cb48ce94c5bec5b48fcfdf))
- **core:** Use a genuinely unknown op type now that warp/beauty are known ops ([9d875d6](https://github.com/harveyxiacn/imagePicker/commit/9d875d66e78393e3d81486a7395129cb9c368cfc))
- **core:** Fix sync test expectation for the unknown-op example ([5acf55c](https://github.com/harveyxiacn/imagePicker/commit/5acf55cdb13885f076259f76c455c50a20e3b1e4))
- De-flake preview render-count test; accept beauty_models in models.list ([c9509e9](https://github.com/harveyxiacn/imagePicker/commit/c9509e9afe649bc9b8072a76b559d8618ff5d918))
- **bench:** Fake worker synthetic analysis (IP_FAKE_ANALYSIS), burst-aware bench gen, bench seed --files-dir ([ae35069](https://github.com/harveyxiacn/imagePicker/commit/ae35069839ed56dfdc65f2ee18b789ca782c1b2a))
- **e2e:** Real-server end-to-end test with the fake worker; analysis progress no longer shown as an export toast ([1a4cb59](https://github.com/harveyxiacn/imagePicker/commit/1a4cb59676e6b6e383ddce6840d5c9b0583ccc8c))
- **core:** Hold the fake uv sync open so the runtime install conflict check is deterministic ([ea81c1a](https://github.com/harveyxiacn/imagePicker/commit/ea81c1ad20c96afb77b74cb3a32a43b9c0e082cd))
- **ai:** Exclusive-eviction test uses an explicit memory budget ([48d6291](https://github.com/harveyxiacn/imagePicker/commit/48d62913436297f5d197c554c7076545ccedaec3))
- **server:** Wait for edits.updated before snapshotting render count ([6c11447](https://github.com/harveyxiacn/imagePicker/commit/6c11447305ec36eb564f2498d83c4511dea9b3fc))
- **server:** Give the login rate-limit window test room for slow runners ([c0ec2f4](https://github.com/harveyxiacn/imagePicker/commit/c0ec2f4009e6f863615b5afd6e6e798489d99bd3))
- **core:** Give the runtime install tests 60 s on a loaded machine ([675d96c](https://github.com/harveyxiacn/imagePicker/commit/675d96c56a09ad3bad664cb500538b199cd019d1))
- **core:** Wait for the model-download done event instead of racing it ([4497f11](https://github.com/harveyxiacn/imagePicker/commit/4497f11cc8453ffcef31c815657e98b79d22718b))
- **core:** Accept cancelled as well as cancelling right after a cancel request ([d3a98eb](https://github.com/harveyxiacn/imagePicker/commit/d3a98ebbaeb4b854757470aa014bcf64d6901dc4))
- **core:** Keep the lite analysis tests light under load ([9ac8e38](https://github.com/harveyxiacn/imagePicker/commit/9ac8e38d796ce82b227876677cb017efa4530f8c))
- **core:** Give analysis runs five minutes of wall-clock time in tests_m2 ([7fc7eb1](https://github.com/harveyxiacn/imagePicker/commit/7fc7eb14607ed64447d416a92d6101e35066b079))
- **core:** Give lite analysis runs five minutes of wall-clock time in tests_lite ([f98b9a2](https://github.com/harveyxiacn/imagePicker/commit/f98b9a2043229412ebf097bf7337ed1699df7f42))

### CI and build

- GitHub Actions for rust/web/ai-worker, issue templates, contributing guide ([eebcbe0](https://github.com/harveyxiacn/imagePicker/commit/eebcbe0f7ce3d09ffe324a19fb17219e904eea1c))
- Drop invalid job-level hashFiles conditions ([1ffb8f2](https://github.com/harveyxiacn/imagePicker/commit/1ffb8f25ffb9a3d654bf063a0a2df782caa06938))
- Release and nightly workflows, version-consistency check, bump-version script ([1be68f8](https://github.com/harveyxiacn/imagePicker/commit/1be68f8518a8303b0366b45e180d60b19892e3e4))
- AUR packages (imagepicker, imagepicker-bin) and Flatpak draft ([cf919b7](https://github.com/harveyxiacn/imagePicker/commit/cf919b79372ad7f74c1808ad2e0ff1d3591c7950))
- Rename duplicate android job (core check vs debug APK) ([0d0708d](https://github.com/harveyxiacn/imagePicker/commit/0d0708d0504ff72eb89e34c9ac1be7b58f786b51))
- **release:** Build and publish the Android APK ([9d72ee6](https://github.com/harveyxiacn/imagePicker/commit/9d72ee61d28f919596740dd632cff6f117902989))
- **release:** Setup-android without the retired tools package ([aed2418](https://github.com/harveyxiacn/imagePicker/commit/aed24185d3c8b45d3ca114c7239784ddcc544360))
- Exclude the tauri media plugin from the host rust job; fix import/settings test races ([3e0087c](https://github.com/harveyxiacn/imagePicker/commit/3e0087c83749d8cbb9092709ca723e59449c39ce))

### Miscellaneous

- Cargo workspace skeleton, ip-imaging contract and M1 API contract ([58b8009](https://github.com/harveyxiacn/imagePicker/commit/58b800942ae9f171beb4b265e604ba1ba3ca52fc))
- Ignore local .claude directory ([008fdd0](https://github.com/harveyxiacn/imagePicker/commit/008fdd075f0c76f63c907fc3c3c8ea2487f6403f))
- Ip-render contract (edit stack types, renderer API) and M3 API contract ([6a0437e](https://github.com/harveyxiacn/imagePicker/commit/6a0437ee77f375b559bed5eac2ad7fe70690cedf))
- M4 contract (beauty/warp ops, person geometry, beauty.prepare, M2 remainder APIs) ([08986a2](https://github.com/harveyxiacn/imagePicker/commit/08986a2b2fdbf4032ff501edd0fdbf1f9f607c57))
- Update Cargo.lock for ip-server -> ip-render dependency ([780003f](https://github.com/harveyxiacn/imagePicker/commit/780003faa5a6f02b113e6344e1ab0746581d963b))
- M5 contract (patch op, best take, inpaint, enhance) and backlog doc ([8234e26](https://github.com/harveyxiacn/imagePicker/commit/8234e2682f269a6493c6ef67b504cd64f4a67aee))
- Cargo.lock for desktop/android tauri features ([a25baad](https://github.com/harveyxiacn/imagePicker/commit/a25baad16557b4a9364f778612e3bf3e6a3baa5c))
- ComfyUI synthetic evaluation set generator; backlog after the M8 CI/APK/ip-infer round ([ed90a3e](https://github.com/harveyxiacn/imagePicker/commit/ed90a3e67803ce67a5e25eb4dd3a53b3f1885889))
- **bench:** V2 synthetic set generator (actor references, img2img bursts, places/things/defects) ([33c975c](https://github.com/harveyxiacn/imagePicker/commit/33c975cbca6ca90c4efd1bd7cfc856bfa2b4f7cc))
- **bench:** EXIF converter understands v2 scene/burst keys ([3cd2d7e](https://github.com/harveyxiacn/imagePicker/commit/3cd2d7e4a3d6b09f1b227c4c5db799fd57f8659e))
- **bench:** V2 generator uses face-crop identity references, scene-first prompts, denoise 0.7 bursts ([000c6e6](https://github.com/harveyxiacn/imagePicker/commit/000c6e6288c4f88e7ab4fb7afe9ebf30ccc65a0d))
- **bench:** EXIF converter rotates scenes over a camera and two phones; document-like images get no EXIF ([325e4ff](https://github.com/harveyxiacn/imagePicker/commit/325e4ff0219182b0588cde64aa3cedc2db33ecea))
- **scripts:** Add the ComfyUI synthetic photo library generator, audit and test ([c55f36d](https://github.com/harveyxiacn/imagePicker/commit/c55f36d066981e70674599b38b7437b9ef18fe12))
- Cargo fmt the subject_flags pass; ruff format gen_lite_fixtures.py; docs/09 notes the committed generator ([fd28e5d](https://github.com/harveyxiacn/imagePicker/commit/fd28e5d06ffa8d18aec82e9d2ec3fb9fab3ef1aa))

### Bench

- Standard-profile evaluation on the synthetic photo library ([51e21a5](https://github.com/harveyxiacn/imagePicker/commit/51e21a50e29545f1383d5b6f0d1897821a2f0d9a))

