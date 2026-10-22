# 项目进度与功能检查（2026-10-14）

基线：本机 Windows、`main` `a62ad789`，版本 0.1.0。检查源码、现有测试和真实服务端浏览器流程；不将历史评测或模拟 AI 的结果当作本次真实模型验收。

后续已按用户要求完成 [1000 张生成图库与真实模型专项](library-verification-2026-10-14.md)。下文“真实模型/暂停恢复/远程 AI 未验证”描述的是基础检查阶段；这些项目的最新结果以专项报告为准。HEIC 与真机验收缺口仍有效。

## 结论

核心挑片与编辑导出流程已可运行，但不能认定全部模块均正常或已经达到发布标准。发现真实 HEIC 镜像方向错误；自然语言搜索与 SD 卡导入入口仍禁用。Android、安装包、真实模型推理效果仍需专项验收。

10 月 9 日的 [完成度评审](09-完成度评审与收尾计划.md) 是历史快照，其 40/26/15 统计不能直接用于当前版本。后续已合入数据安全、RAW+JPEG 配对、文件监听、原生像素查看、导出格式、分析暂停/续跑/增量与 lite 回退、助手计划约束等工作。当前缺口结合 [backlog](backlog.md) 和本报告查看；本次没有逐条重算全部 81 项 PRD 的完成率。

## 本次验证

| 检查 | 结果与边界 |
|---|---|
| Web lint / test / build | 45 个文件、380 项测试通过；生产构建通过。主 JS 约 1,014 kB，触发 900 kB 分包警告 |
| AI worker Ruff + pytest | 223 通过、33 排除（模型/GPU范围未纳入）；修复本机虚拟环境中的旧账户 Python 路径后执行 |
| 图库生成 Python 脚本 | 3 项 unittest 通过 |
| 静态检查 | 最终 `cargo fmt --all --check`、CI 范围 Clippy（`-D warnings`）、Web lint、`git diff --check` 均通过 |
| Rust CI 范围 | 排除桌面壳及媒体插件的 workspace 测试：625 通过、3 ignored；新增 bench 回归后 CLI 单独复跑 12 项通过。真实 HEIC 测试在默认环境中提前返回，需单独启用 |
| Windows 桌面壳 | `cargo check -p ip-desktop --locked` 通过；不等同于安装器和 GUI 实机验收 |
| 浏览器真实服务端 E2E | 40 张合成 JPEG、11 步通过、0 控制台错误。真实 Rust 服务端 + fake AI worker，不证明真实模型输出正确 |
| 真实 HEIC 专项 | **失败**：启用测试后，`o2.heic` 角落颜色应为 `GR..`，实际 `RG..` |

E2E 覆盖：首次运行、文件夹导入、缩略图、评分/旗标/撤销/重做、模拟分析与首次人脸同意、堆栈展开收起、曝光调整、预设、原图/分屏对比、编辑标记、导出 JPEG。导出后检查真实文件并比较像素亮度：129.8 → 173.3。

## 模块状态

| 模块 | 当前判断 |
|---|---|
| JPEG 导入、浏览、评分、撤销、分组 | 真实服务端 E2E 通过；RAW/HEIC 不能据此一并判定 |
| 暂停/续跑、增量分析、lite 回退 | 已合入，Rust 核心/API 测试通过；真实进程强杀、多平台恢复仍需复核 |
| 曝光、预设、前后对比、JPEG 导出 | E2E 通过；更多渲染/人像/导出路径有自动测试，未逐项 GUI 实测 |
| 人脸、最佳表情、消除、美化、AI 助手 | 已有实现及自动测试；本次未运行真实权重全链路。历史 [standard 评测](../bench/standard-eval-2026-10-09.md) 有 1000 张合成图库数据，仍有误检和真实照片验证缺口 |
| HEIC/HEIF | 容器/合成测试通过，真实镜像方向测试失败，暂不能认定方向显示正确 |
| 桌面与 Web | Web 核心流程通过，Windows 桌面编译通过；打包安装、更新及 macOS/Linux 未验证 |
| Android、远程 AI | 本次未跑 Android 构建或设备验收，不做可用性结论 |
| 自然语言搜索、SD 卡导入 | `TopBar.tsx` 搜索输入框和 `Home.tsx` SD 卡入口仍 disabled，不能算作用户可用功能 |

## 发现的问题与最小修复

1. **待修复：Windows HEIC 镜像方向错误。** `heif.rs::rotated` 用宽高方向变化推断系统解码器是否已处理方向，无法识别镜像或 180° 变换。本机 `o2.heic` 实测失败，疑似重复处理方向。应明确各解码器方向契约，再验证 EXIF 1–8、方形图、裁切及缩略图；本次未更改生产解码策略。
2. **已修复：bench 图库生成器调试构建溢出。** 首帧用正时间戳减 `i64::MIN` 会 panic；现在显式处理第一帧，补充生成与解码回归测试。
3. **已修复：WIC 错误用例依赖本机编解码器。** 原损坏 HEIC 夹具含有效 JPEG 缩略图，本机 WIC 可返回图像。改为无有效缩略图的损坏 HEVC 夹具，保留 PNG 成功解码及错误信息断言；影像模块 44 项常规测试通过。
4. **已修复：E2E 对虚拟网格的错误断言。** 展开后挂载单元格数量可能减少（实测 18 → 11）。改为验证目标堆栈具体成员显示/隐藏及 `aria-expanded`，完整流程通过。
5. **待观察：首次 E2E 文件夹浏览出现一次 403。** 独立 API 验证短路径、完整路径均成功；改用完整临时目录后导入成功，最终 E2E 全通过。根因未定，不归类为已修复生产缺陷。

## 后续顺序

1. 修复 Windows HEIC 方向，将真实样张专项测试纳入验收。
2. 用真实照片和模型复核人脸/闭眼/最佳表情/消除/助手，重点关注合影误检、模糊标签及大面积消除。
3. 验证 Windows 安装包首次安装与 AI 组件安装、Android 真机导入/权限/远程分析。
4. 更新 PRD 完成度矩阵，区分禁用入口、未实现功能与已完成但未实测功能；随后处理前端分包。

## 复现

```powershell
# 真实 HEIC 专项：当前会失败，不能省略环境变量后声称通过
$env:IMAGEPICKER_TEST_PYTHON = "$PWD\ai-worker\.venv\Scripts\python.exe"
cargo test -p ip-imaging real_heic_files_match_libheif_display -- --nocapture
Remove-Item Env:IMAGEPICKER_TEST_PYTHON

# 核心流程（fake AI，使用当前源码 binary）
cargo build -p ip-cli -p ip-worker-client --bins --locked
cd web
pnpm build
node scripts/e2e-real.mjs --bin ../target/debug/imagepicker.exe --worker ../target/debug/ip-fake-worker.exe
```

清理由用户转交 Claude Code 后已停止。先前部分清理记录保留在 `.claude/cleanup-2026-10-14.log`。源码修改未提交。
