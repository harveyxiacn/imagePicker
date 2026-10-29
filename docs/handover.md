# 接手文档

> 写给下一位接手的人（或 agent）。目标：一小时内能在本机跑起来、知道改哪里、哪些不能碰、还剩什么。基础环境与仓库结构见 [CONTRIBUTING](../CONTRIBUTING.md)，这里只写「文档里没写、但接手必须知道」的部分。基线日期 **2026-10-23**。

## 1. 现在是什么状态

桌面端功能面基本铺满（导入、分析、挑片、修图、生成式修复、面部/人物、助手、局域网、XMP、导出、Android 外壳）。最近的变动见 [开发日志](dev-log.md)。**1.0 之前缺的主要不是代码，而是真实数据的证据和负责人的决定**：真实照片/手机/相机的样张、GPU 机上的评测数字、签名与商店账号。逐条清单见 [路线图 §1](10-路线图.md)（1.0 阻塞项）与 [backlog](backlog.md)（按 P0/P1/P2 排序）。

最近一轮已完成的：

- 分析性能：standard 档 GPU 1000 张 73 s（PRD < 2 min 达成）；`quality` 步骤改走 Rust 原生库，同机 A/B 墙钟 −34%。
- 风格学习（内部 id `taste`）：只用用户亲手的精选/淘汰/星级，跨会话、AUC 过关才启用；可以按单张/单会话遗忘。
- 助手：数值滑块指令（`adjust_sliders`）与模型计划校验（`llm_tool_mismatch`）。
- HEIC：Windows 上方向与色彩问题修复；生成任务可真正取消。

## 2. 第一小时

1. 按 [CONTRIBUTING「Development setup」](../CONTRIBUTING.md) 装好 Rust / Node 24 / pnpm 10 / uv + Python 3.12。
2. 只想看界面：`cd web && pnpm install && pnpm dev:mock`（浏览器内 mock 后端，不需要 Rust、Python 或模型）。
3. 跑一遍门禁（见 §5），确认本机基线是绿的。
4. 读顺序：[backlog](backlog.md) → [路线图](10-路线图.md) → [09 完成度评审](09-完成度评审与收尾计划.md) → 本文 §4。
5. 想碰 AI：在 `ai-worker/` 里 `uv sync --extra cpu --extra mediapipe`（有 NVIDIA 显卡用 `--extra cuda`），模型放 `ai-worker/.models` 或 `<数据目录>/models`。

## 3. 代码地图（最常改的地方）

| 要改什么 | 去哪 |
|---|---|
| 扫描、EXIF、RAW 预览、缩略图、HEIF 解码 | `crates/ip-imaging`（HEIF 契约见 `imp/heif.rs` 文件头 + docs/02 §5.1） |
| 分析编排、评分/分组、目录库、助手、风格、导出的业务逻辑 | `crates/ip-core`（`analysis/`、`assistant/`、`taste.rs`、`edit/`） |
| HTTP/WebSocket 路由与鉴权 | `crates/ip-server`（`lib.rs` 路由表、`m*.rs` 各里程碑接口、`auth.rs`） |
| 修图渲染（编辑栈 → 像素） | `crates/ip-render`（`prep.rs` 解析、`cpu.rs`/`shaders/` 双实现、`patch.rs` 图层） |
| 纯 Rust 端侧分析（lite 档、风格与分析的共享指标） | `crates/ip-lite`；给 worker 用的原生库在 `crates/ip-lite-ffi` |
| Python AI worker（分析、面具、修复、LLM） | `ai-worker/imagepicker_ai`（`pipeline.py`、`steps/`、`rpc.py`） |
| 前端 | `web/src`（`api/`、`components/`、`pages/`、`i18n/`、`mock/`） |
| 桌面外壳与打包 | `apps/desktop`（`src-tauri/`、`scripts/`） |
| 契约与行为说明 | `docs/api-contract-m1…m9.md`（**与代码同步维护**） |

## 4. 不变量与约定（改之前先读）

1. **显示名与内部 id**：个性化功能对用户叫 **「我的风格 / My style」**；代码、数据库、接口一律仍是 `taste`（`taste_state`/`taste_score`/`preference_label`、`/api/taste/*`、`sort=taste`、`taste.rs`）。不要在界面或文档里复活「口味 / taste」；也不要擅自改内部名（无迁移计划）。
2. **Python 与 Rust 两份实现必须同步**：`ip-lite` 是 worker `steps/*.py` 的 Rust 移植（quality、tilt、pHash），一致性由 `crates/ip-lite/tests/parity.rs` 用固定容差（sharpness 0.002、tenengrad 0.4%、tilt 0.05°）守。改任何一边都要改另一边并跑 parity；worker 的 `quality` 优先走原生库（`steps/lite_native.py`），没有库时回退 Python。
3. **原生库是可选加速**：`crates/ip-lite-ffi` 只在库存在时加载（`IMAGEPICKER_LITE_LIB` → worker 源码 `steps/_native/` → 检出目录的 `target/`）。任何新依赖都不能让它变成硬依赖；改 ABI 要同时改 `ABI_VERSION`、`QualityOut` 与 Python 的 ctypes 结构。
4. **HEIF 契约**：Windows 上 libheif 优先、WIC 兜底（WIC 的 HEVC 色彩与 libheif 差数十级）；WIC 已自行应用 `irot`/`imir`（含镜像/180°），不要再用长边判断重复旋转。见 docs/02 §5.1 与 `real_heic_files_match_libheif_display`。
5. **助手的安全网**：模型输出必须先过 `validate_call`；规则引擎的解读作为校验（`llm_unsafe_target`、`llm_tool_mismatch`、`llm_arg_mismatch`；基准 `bench/eval-assistant.py`），必要时整份模型计划退回规则。破坏性/大范围步骤必须确认，计划执行要可一次撤销。
6. **生成任务必须可取消**：`inpaint`/`enhance`/`besttake`/`hair` 等 worker 计算在线程池里，必须在循环里检查 `Cancel` 令牌（`ai-worker/imagepicker_ai/cancel.py`），否则请求结束后计算还在跑。
7. **图层与编辑栈顺序是内容**：`PatchKind::layer_order()`（`ip-render`）与 `edit/sync.rs` 的 `rank()` 决定合成顺序；改动会改变已有照片的渲染结果，必须补一致性测试（`ip-render/tests/correctness.rs` 的不变量）。
8. **数据库迁移只追加**（当前 `PRAGMA user_version` = **18**）：新增表/列写成新的 migration，不要改历史条目；v17 归风格模型使用，v18 是 `analysis.sharpness_center`（连拍相对模糊），均已被占用。
9. **i18n 两个文件同步**：`web/src/i18n/zh-CN.ts` 与 `en.ts`；`i18n.test.ts` 校验键表，新增界面文案两边都要加。快捷键只在 `web/src/lib/keymap.ts` 定义一次。
10. **文档口径**：不要写成「逆向 / 复刻 Photoshop、Lightroom」；描述功能本身与实现原理即可。
11. **提交**：身份用仓库的 noreply 邮箱（推送会被拒）；提交信息末尾可加 `Co-Authored-By: Claude Code <noreply@anthropic.com>`；PR 描述末尾 `🤖 Generated with [Claude Code](https://claude.com/claude-code)`。分支 `feat/<area>-<topic>` / `fix/<area>-<topic>`。

## 5. 测试与基准（本机实际用的门禁）

```sh
# Rust（桌面 crate 在共享 runner 上很慢，本地可全跑；CI 会排除它们）
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Web
(cd web && npm run lint && npm test && npx tsc -b)

# Worker（不需要模型）
(cd ai-worker && .venv/Scripts/python.exe -m ruff check . && .venv/Scripts/python.exe -m pytest -m "not models and not gpu")
```

- **Windows 上偶发编译怪错误**（`no resolution for an import`、只找到 rlib 的 metadata stub）：先 `CARGO_INCREMENTAL=0` 或 `cargo clean -p <crate>`；不要同时跑 clippy 和 test（共享 target 目录会互相踩）。
- **真实 HEIC 测试**：`IMAGEPICKER_TEST_PYTHON=<带 pillow-heif 的 python> cargo test -p ip-imaging real_heic`（用 `ai-worker/.venv/Scripts/python.exe`）。
- **GPU 渲染**：`IMAGEPICKER_REQUIRE_GPU=1 cargo test -p ip-render -- --nocapture`（没有适配器时该变量让测试失败而不是跳过）。
- **分析档位基准**：`PYTHONPATH=ai-worker ai-worker/.venv/Scripts/python.exe bench/profile-analysis.py bench/data/qwen-photo-library/jpeg --profile standard --device auto --limit 1000 --models-dir ai-worker/.models`；A/B 原生 quality 用 `IMAGEPICKER_LITE_LIB=0` 跑对照组。当前数字与文件见 [路线图 §2.1](10-路线图.md) 与 `bench/results/`。
- **评测库与模型都不进仓库**：`bench/data/qwen-photo-library`（1000 张合成图库，2.7 GB）与 `ai-worker/.models`（约 11 GB）在本机存在，clone 后需要自己生成/下载（见 `bench/README.md`、`ai-worker/README.md`）。

## 6. CI 与发布

- `ci.yml`、`nightly.yml`、`audit.yml` 是**手动触发**（`workflow_dispatch`）；日常以本地门禁为准。
- `release.yml` 会在 **改动以下文件的 PR 上自动干跑一次四平台构建**（不建 Release）：`tauri.conf.json`、`release.yml` 本身、`scripts/bump-version.sh`、Android gradle、`cliff.toml`。约 20 分钟，是唯一会真正打包安装器的地方——动打包配置时务必等它绿。
- Tauri 的 `beforeBuildCommand` **以 `apps/desktop` 为 cwd**（不是 `src-tauri`）；原生 quality 库由 `apps/desktop/scripts/copy-lite-lib.mjs` 复制进 `ai-worker/imagepicker_ai/steps/_native/`（git 忽略），缺失时构建脚本会先 `cargo build -p ip-lite-ffi`。
- 发布当前**不签名**；`v1.0.0` 的时机等真实照片校准完成，**打 tag 前必须得到负责人确认**（tag 会触发草稿 Release）。细节见 [release.md](release.md)。

## 7. 交接时未完成的事（建议顺序）

1. **真实数据验证**（需要负责人）：`bench/eval-search.py`、`bench/eval-time-align.py`、`bench/eval-bystander.py`、standard 端到端评测、真实 HEIC/RAW 样张、GPU 渲染耗时。命令与验收线都在 [路线图 §1](10-路线图.md)。
2. **分析性能下一步**：`quality` 已不是最大项；现在是 `faces`（YuNet + MediaPipe 关键点，GPU 213 ms/张）与 `decode`（63 ms），可先在 `bench/profile-analysis.py` 上量再改。
3. **风格**：真实图库 `imagepicker taste --eval`；`sort=taste` 的索引与 10 万张会话耗时。
4. **打包**：macOS universal 安装包里的原生 quality 库目前是构建机单一架构（Intel 机会自动回退 Python），如需 universal 需在 `copy-lite-lib.mjs` 里 lipo 两个架构。

## 8. 文档地图

| 想知道 | 看 |
|---|---|
| 产品要什么 | [01 PRD](01-PRD-产品需求文档.md) |
| 架构与进程/解码/渲染管线 | [02 架构](02-架构设计.md) |
| 算法、阈值、实验证据 | [03 AI 与算法](03-AI与算法设计.md) |
| 界面与交互 | [04 UI/UX](04-UI-UX设计.md) |
| 表结构、路由、事件 | [05 数据与接口](05-数据与接口设计.md) + `api-contract-m*.md` |
| 模型清单与许可 | [07](07-模型清单与许可.md) |
| 各需求完成度、缺口、发布门槛 | [09](09-完成度评审与收尾计划.md) |
| 现在做什么、下一步 | [10 路线图](10-路线图.md) |
| 已知问题与校准待办 | [backlog](backlog.md) |
| 最近做了什么 | [开发日志](dev-log.md) |
| 用户视角 | [用户指南（中）](user-guide/zh-CN.md) / [User Guide (en)](user-guide/en.md) |
