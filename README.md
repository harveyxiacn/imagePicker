<div align="center">

# imagePicker

**本地优先、隐私安全、跨平台的 AI 照片挑选与修图工作台**
**A local-first, privacy-first, cross-platform AI photo culling & retouching workstation**

[中文](#中文) · [English](#english) · [用户指南 / User Guide](docs/user-guide/README.md) · [文档 / Docs](docs/README.md)

![imagePicker grid with stacks and AI stars](docs/images/grid.png)

</div>

---

## 中文

旅行、聚会、婚礼拍了上千张？拖进来，一键分析：连拍与相似照片自动成组、0–5 星智能评分、闭眼/模糊自动标记、合影「全员最佳表情」合成、自动光影色彩与人像美化。**所有处理都在你自己的电脑上完成，照片不会上传。**

### 状态

**预发布（v0.1，开发中）**：核心功能（导入、分析、分组、评分、人物、最佳表情、编辑、人像美化、修复、导出、AI 助手、局域网 WebUI、XMP 互通、设置）已实现并通过 mock 与合成数据验证。已知限制见 [待办与已知问题](docs/backlog.md)：评分阈值尚未在大规模真实照片上校准；HEIC/HEIF 缩略图、搜索框（自然语言搜索）、Android 版尚未提供。安装包与 AI 运行时的打包仍在进行中，目前请参考下文「从源码运行」。

### 功能亮点

| | |
|---|---|
| ⚡ 极速载入 | Rust 原生缩略图流水线（优先提取内嵌预览），万张级虚拟化网格 |
| 🧠 一键智能分析 | 连拍/场景分组、可解释的多维评分映射为 0–5 星，闭眼、模糊、过曝等问题标签 |
| 📚 堆栈与组视图 | 连拍折叠成堆栈；组视图 A/B 对比、同步缩放、一键「保留最佳，淘汰其余」 |
| 👥 人物 | 本地人脸聚类，按人物筛选（包含/排除、睁眼、笑容、看镜头）、每人最佳、按人物导出、以脸搜脸 |
| 😀 最佳表情 | 人物 × 帧的表情矩阵；每个人选最佳表情，合成到同一张底片 |
| 🎨 非破坏性编辑 | 基础/曲线/HSL/色彩分级、AI 蒙版（主体/天空/人物/皮肤/头发/衣服）、渐变、裁剪、预设、LUT |
| 💄 人像美化 | 磨皮、美白、去瑕疵、亮眼、瘦脸、瘦手臂/腿等；按人物保存档案，自然度提醒 |
| 🧽 修复 | 消除路人、画笔消除、降噪、人脸修复、导出时超分 |
| 🤖 AI 助手 | 中英文自然语言指令 → 先出计划，确认后执行，可一步撤销 |
| 🌐 局域网 WebUI | 手机/平板用浏览器访问；所有者与只读访客两种角色，扫码访问 |
| 🔁 XMP 互通 | 与 Lightroom / darktable 共享星级、旗标、色标、关键词 |
| 🎛️ 硬件自适应 | 纯 CPU 到 RTX 4090 四档（T0–T3）；模型按需下载，可选镜像 / ModelScope |

### 截图

| 一键分析后的网格（堆栈 + AI 星级） | 组视图与表情矩阵 |
|---|---|
| ![grid](docs/images/grid.png) | ![group view](docs/images/group-view.png) |
| **最佳表情编辑器** | **编辑：局部蒙版** |
| ![best take](docs/images/besttake.png) | ![masks](docs/images/edit-masks.png) |
| **人像美化** | **AI 助手** |
| ![portrait](docs/images/portrait.png) | ![assistant](docs/images/assistant.png) |
| **人物页** | **浅色主题** |
| ![people](docs/images/people.png) | ![light theme](docs/images/grid-light.png) |

> 截图由 mock 后端和程序生成的示例图（渐变风景 + 卡通脸）截取，不含任何真实照片。

### 硬件档位

首次启动时自动检测，可在「设置 → 硬件与模型」查看。档位只决定默认启用哪些模型，核心功能在 CPU 上都能工作，只是更慢；本地 AI 助手模型与扩散修补等增强包需要 T2 及以上（助手在此之下使用离线规则引擎）。

| 档位 | 典型硬件 | 体验 |
|---|---|---|
| **T0** | 无独显 / 老笔记本，8 GB+ 内存 | 快速分析可用；标准分析与生成式修复较慢（超分每张可能数十秒） |
| **T1** | Apple M 系列 ≥ 8 GB 统一内存、4–6 GB 独显、DirectML 集显 | 标准分析、AI 蒙版、降噪、人脸修复 |
| **T2** | 8–12 GB 显存（RTX 3060 / 4070）、M Pro/Max ≥ 16 GB | 加上本地 AI 助手模型（LLM/VLM） |
| **T3** | ≥ 16 GB 显存（如 RTX 4090） | 全部可选增强包（如 SDXL 修补） |

NVIDIA 显卡需要安装 AI 组件的 CUDA 版本才会被识别为 GPU 档位（见用户指南）。

### 安装与快速开始

**普通用户**：安装包将发布在 [GitHub Releases](https://github.com/harveyxiacn/imagePicker/releases)（Windows / macOS / Linux 含 CachyOS）。发布前请使用下面的方式从源码运行。安装步骤详见 [用户指南 · 安装](docs/user-guide/zh-CN.md#2-安装)。

**从源码运行（桌面版）**

```sh
# 需要 Rust、Node 24、pnpm 10；Linux 另需 WebKitGTK 等依赖（见 apps/desktop/README.md）
cd web && pnpm install && cd ../apps/desktop && pnpm install
pnpm tauri dev                          # 开发模式
pnpm tauri build                        # 打包当前系统的安装包
```

**只用 WebUI（浏览器）**

```sh
cd web && pnpm install && pnpm build && cd ..
cargo run -p ip-cli -- serve --web-dir web/dist     # http://127.0.0.1:7878
```

**AI 组件（可选，但分析/修图的 AI 功能需要）**：需要 [uv](https://docs.astral.sh/uv/) 与 Python 3.12。

```sh
cd ai-worker
uv sync --extra cuda --extra mediapipe   # NVIDIA GPU；仅 CPU 用 --extra cpu --extra mediapipe
```

**5 步上手**：① 拖入文件夹导入 → ② 点「一键分析」（首次会提示下载模型）→ ③ 用数字键 1–5 打星、`P` 精选、`X` 淘汰，`S` 展开堆栈、`B` 进组视图 → ④ `D` 进入编辑 → ⑤ `Ctrl+E` 导出。完整流程见 [用户指南](docs/user-guide/zh-CN.md)。

### 隐私

- 照片、人脸特征、评分、编辑都只保存在本机（目录库 `catalog.db` 与缓存）。
- 联网只用于**按需下载模型**（Hugging Face / 镜像 / ModelScope），下载前会展示模型清单与大小；在「设置 → 人脸与隐私」关闭「允许联网」后完全离线运行。
- 人脸识别可整体关闭，也可一键清除全部人脸数据。
- 局域网访问默认关闭；开启必须先设置所有者密码，访客只读且看不到人物。

详见 [用户指南 · 隐私与数据位置](docs/user-guide/zh-CN.md#16-隐私与数据位置)。

### 许可

代码采用 [Apache-2.0](LICENSE)。AI 模型权重**不随仓库分发**，由应用按需下载并遵循各自许可证；标记为「非商业」的模型仅限个人使用。详见 [模型清单与许可](docs/07-模型清单与许可.md)。

### 文档

[用户指南（中文）](docs/user-guide/zh-CN.md) · [User Guide (English)](docs/user-guide/en.md) · [设计文档总览](docs/README.md) · [待办与已知问题](docs/backlog.md) · [贡献指南](CONTRIBUTING.md) · [安全策略](SECURITY.md)

---

## English

Thousands of shots from a trip, party or wedding? Drop the folder in and hit Analyze: bursts and near-duplicates are stacked automatically, every photo gets an explainable 0–5 star suggestion, closed eyes / blur / clipping are flagged, group shots can be composed from everyone's best expression, and auto tone and portrait retouching are one click away. **Everything runs on your own machine; your photos are never uploaded.**

### Status

**Pre-release (v0.1, in development).** The core feature set (import, analysis, grouping, scoring, people, best take, editing, portrait retouching, repair, export, AI assistant, LAN WebUI, XMP interop, settings) is implemented and verified against a mock backend and synthetic data. See [backlog](docs/backlog.md) for known limitations: score thresholds are not yet calibrated on a large real-photo set; HEIC/HEIF thumbnails, the search box (natural-language search) and the Android app are not available yet. Installer and AI-runtime packaging is still in progress, so for now see "Run from source" below.

### Highlights

| | |
|---|---|
| ⚡ Fast | Native Rust thumbnail pipeline (embedded previews first), virtualised grid for 10k+ photos |
| 🧠 One-click analysis | Burst / scene grouping, explainable multi-factor score mapped to 0–5 stars, issue tags (closed eyes, blur, over/underexposure…) |
| 📚 Stacks & group view | Bursts collapse into stacks; A/B compare with synced zoom, "keep best, reject the rest" |
| 👥 People | On-device face clustering; filter by person (include / exclude, eyes open, smiling, looking), best-per-person, export per person, search by face |
| 😀 Best take | Person × frame expression matrix; pick each person's best expression and composite onto one base frame |
| 🎨 Non-destructive editing | Basic / curves / HSL / colour grading, AI masks (subject, sky, person, skin, hair, clothes), gradients, crop, presets, LUTs |
| 💄 Portrait retouch | Smooth, brighten, blemish removal, eyes, face and body reshaping; per-person profiles; naturalness warning |
| 🧽 Repair | Remove bystanders, brush erase, denoise, face restore, upscale on export |
| 🤖 AI assistant | Natural-language commands (zh / en): shows a plan first, runs it after you confirm, undo in one step |
| 🌐 LAN WebUI | Phone / tablet in the browser; owner and read-only guest roles, QR code |
| 🔁 XMP interop | Shares ratings, flags, colour labels and keywords with Lightroom / darktable |
| 🎛️ Hardware tiers | CPU-only to RTX 4090 (T0–T3); models are downloaded on demand, mirrors / ModelScope supported |

### Screenshots

| Grid after analysis (stacks + AI stars) | Group view with expression matrix |
|---|---|
| ![grid](docs/images/grid.png) | ![group view](docs/images/group-view.png) |
| **Best take editor** | **Editing: local masks** |
| ![best take](docs/images/besttake.png) | ![masks](docs/images/edit-masks.png) |
| **Portrait retouch** | **AI assistant** |
| ![portrait](docs/images/portrait.png) | ![assistant](docs/images/assistant.png) |
| **People** | **Light theme** |
| ![people](docs/images/people.png) | ![light theme](docs/images/grid-light.png) |

> Screenshots were captured against the mock backend with generated sample images (gradient landscapes and cartoon faces); no real photos are shown. The UI language in the images is Chinese; switch to English in Settings.

### Hardware tiers

Detected on first start and shown in *Settings → Hardware & models*. The tier only decides which models are enabled by default. Core features work on CPU, just slower; the local assistant models and diffusion inpainting need T2 or higher (below that the assistant uses its offline rules engine).

| Tier | Typical hardware | What to expect |
|---|---|---|
| **T0** | No discrete GPU / old laptop, 8 GB+ RAM | Fast analysis works; standard analysis and generative repair are slow (upscaling can take tens of seconds per photo) |
| **T1** | Apple M-series ≥ 8 GB unified memory, 4–6 GB GPU, DirectML iGPU | Standard analysis, AI masks, denoise, face restore |
| **T2** | 8–12 GB VRAM (RTX 3060 / 4070), M Pro/Max ≥ 16 GB | Adds the local AI assistant models (LLM / VLM) |
| **T3** | ≥ 16 GB VRAM (e.g. RTX 4090) | All optional add-on packs (e.g. SDXL inpainting) |

NVIDIA GPUs are only recognised as a GPU tier when the AI components are installed with the CUDA extra (see the user guide).

### Install & quick start

**End users:** installers will be published on [GitHub Releases](https://github.com/harveyxiacn/imagePicker/releases) (Windows / macOS / Linux incl. CachyOS). Until then run from source as below. Details: [User Guide · Installation](docs/user-guide/en.md#2-installation).

**Run from source (desktop)**

```sh
# needs Rust, Node 24, pnpm 10; Linux also needs WebKitGTK etc. (see apps/desktop/README.md)
cd web && pnpm install && cd ../apps/desktop && pnpm install
pnpm tauri dev                          # development
pnpm tauri build                        # installer for the host OS
```

**WebUI only (browser)**

```sh
cd web && pnpm install && pnpm build && cd ..
cargo run -p ip-cli -- serve --web-dir web/dist     # http://127.0.0.1:7878
```

**AI components (optional, required for the AI features):** needs [uv](https://docs.astral.sh/uv/) and Python 3.12.

```sh
cd ai-worker
uv sync --extra cuda --extra mediapipe   # NVIDIA GPU; CPU only: --extra cpu --extra mediapipe
```

**Five steps:** ① drop a folder to import → ② click *Analyze* (the first run asks to download models) → ③ rate with `1`–`5`, `P` pick, `X` reject, `S` expand a stack, `B` group view → ④ `D` to edit → ⑤ `Ctrl+E` to export. Full walkthrough in the [User Guide](docs/user-guide/en.md).

### Privacy

- Photos, face data, ratings and edits stay on your machine (catalog `catalog.db` plus caches).
- The network is only used to **download models on demand** (Hugging Face / mirror / ModelScope), after showing the model list and sizes. Turn off *Allow network* in *Settings → Faces & privacy* to run fully offline.
- Face recognition can be switched off entirely, and all face data can be wiped in one click.
- LAN access is off by default; enabling it requires an owner password first. Guests are read-only and cannot see people.

See [User Guide · Privacy & data locations](docs/user-guide/en.md#16-privacy--data-locations).

### License

Code is licensed under [Apache-2.0](LICENSE). AI model weights are **not** distributed with this repository; the app downloads them on demand under their own licenses, and models marked "non-commercial" are for personal use only. See [model list & licenses](docs/07-模型清单与许可.md) (Chinese).

### Docs

[User Guide (中文)](docs/user-guide/zh-CN.md) · [User Guide (English)](docs/user-guide/en.md) · [Design docs](docs/README.md) · [Backlog](docs/backlog.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)
