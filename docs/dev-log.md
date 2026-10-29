# 开发日志

> 按日期倒序记录「做了什么、怎么验证、留下什么」。发布说明见 [CHANGELOG](../CHANGELOG.md)（由 git-cliff 从提交生成，不回填这里）；待办与已知问题见 [backlog](backlog.md)；下一步计划见 [路线图](10-路线图.md)。接手请先读 [接手文档](handover.md)。

## 2026-10-29

### 1000 张图库第二轮：遗留问题修复（#112–#117）

- 搜索时按相关度平铺（#112）；两个负载下偶发失败的 ip-core 测试（#113）；助手 34 条指令评测脚本、用规则引擎核对模型计划，错误计划被采用 13 → 0（#114）；倾斜透视误报 24 → 6，代价是召回减半（#115）；人物按聚类投票排除狗与剪影、封面按脸部亮度选，迁移 **v19**（#116）；axe 扫描 148 个状态 serious/critical → 0，新增 `npm run a11y`（#117）。Dependabot #96–#99 检查后合入。
- 复测：main b1c9d9fc 全部门禁与评测通过，详见 [1000 张图库实测](library-verification-2026-10-29.md)「第二轮」；分析耗时与第一轮 main 同作业交替对比无退化。
- 踩坑：作业里 `pnpm install` 要 `CI=true`；`git worktree add` 的 stderr 在作业里会中止脚本；PowerShell 变量不区分大小写。
- 后续：真实照片复核新常数；读屏与真机；手机端搜索入口；助手多出步骤的核对。

### 1000 张图库实测与界面体验优化（#100–#110）

- 做法：在真实服务端与真实模型上分析 1000 张合成图库，三种尺寸逐页截图（新增 `web/scripts/ux-audit.mjs`）+ 源码审查 + GPU 评测，修复后在合并后的 main 上全量复测。完整记录与证据路径见 [1000 张图库实测（2026-10-29）](library-verification-2026-10-29.md)。
- 界面：分组网格小场景同行排布（每屏约 3 → 13+ 张，#105）；网格键盘/读屏模型与筛选后空状态（#100）；共享弹层焦点管理、手机键盘避让（#101）；错误提示本地化与提示可访问性（#102）；对比度令牌与 `npm run contrast`（#103）；计数说明、组视图提示、人物编号与封面（#104）；照片角标主题无关颜色（#108）。
- 算法：搜索阈值按查询自适应 + `low_confidence`，中文应用内 R@20 0.086 → 0.606（#107）；模糊精确率 0.23 → 0.63、「说话中」召回 48% → 74%，迁移 **v18**、原生库 ABI 2（#109）；时间对齐按独立时刻计数并校准置信度，「全部采纳」0 错误（#106）。
- 打包：`copy-lite-lib.mjs` 只在缺失时构建，会把旧 ABI 的库打进安装包、worker 静默回退 Python（分析 134 s vs 88 s）；改为总是构建（#110）。
- 踩坑：`uv` 发现 `ai-worker/.venv` 指向已不存在的解释器时会删除重建空环境（用 `uv sync --python 3.12 --extra cuda --extra mediapipe --extra llm-cuda` 恢复；`dev` 是依赖组不是 extra）；`imagepicker analyze` 需要 `--face-consent` 才分析人脸；在 worktree 里测 Python 改动必须同时设置 `PYTHONPATH=<worktree>/ai-worker`，否则导入的是主检出的包。
- 后续：真实照片复核新阈值（模糊 0.85、`jawOpen` 0.06、搜索 0.095）；读屏与真机验收；分组视图下搜索结果按相关度排序；人物里的非人脸聚类。

## 2026-10-23

### 分析性能：基线，然后 quality 走 Rust（#90、#91、#92）

- **基线**（`bench/profile-analysis.py`，`bench/data/qwen-photo-library/jpeg` 前 1000 张，1024 px，8 解码线程，batch 32）：
  standard **GPU 73.1 s / 13.7 张每秒**（RTX 4090，PRD「1000 张 < 2 min」达成）、CPU 133.1 s / 7.5 张每秒；fast 两设备都约 54 s；lite（纯 Rust）约 10.5 ms/张。结果：`bench/results/analyze-*-1000.json`、`e2e-lite-cpu-100.json`；数字写进 [路线图 §2.1](10-路线图.md)。
- **quality 是最大头**（GPU 上占 57%，CPU 上 45%），且是纯 Python/OpenCV，CUDA 帮不上。新增 `crates/ip-lite-ffi`：把 `ip_lite::analyze_quality` 以 C ABI 导出，worker 的 `steps/lite_native.py` 用 `ctypes` 调用（调用期间释放 GIL），`quality.py` 在库存在时走原生、否则保持原 Python 实现。桌面包由 `apps/desktop/scripts/copy-lite-lib.mjs` 把库放进 worker 源码的 `steps/_native/`（git 忽略）；`IMAGEPICKER_LITE_LIB` 可覆盖路径，`=0` 强制关闭（A/B 用）。
- **效果**（同一机器背靠背交替各两轮）：原生 91.0 s / 11.0 张每秒 vs Python 137.6 s / 7.3 张每秒，**墙钟 −34%**，`quality` 线程时间 458 → 121 ms。数值与 Python 一致到 `ip-lite` 既有容差（sharpness 0.002、tenengrad 0.4%、tilt 0.05°）。证据：`bench/results/analyze-gpu-standard-1000-ab.txt`。
- **踩坑**：#91 把打包步骤写成 `node ../scripts/copy-lite-lib.mjs`，但 Tauri 的 `beforeBuildCommand` 以 `apps/desktop` 为 cwd，四个平台构建全部失败；#92 改成 `scripts/...` 后 Release 干跑四平台全绿。
- **后续**：现在最大的是 `faces`（GPU 213 ms）与 `decode`（63 ms）；原生库目前只覆盖 quality。

### 风格（taste v2）收尾与改名（#93、#94）

- 显示名 **「我的风格 / My style」**（原「口味 / My taste」）；内部 id 保持 `taste`（表、`/api/taste/*`、`sort=taste`、`taste.rs`），已存模型、设置和客户端不受影响。文档在 03 §3.3 与 api-contract-m4 各注明一次。
- 新增 `POST /api/taste/forget {"photo_id"|"session_id"}`：删掉一张照片或整个会话的学习样本并在后台用剩下的重训，低于冷启动门槛（正负各 30）时自动停用并清空风格分；检查器「为什么？」里加了「忘记这张照片的决定」按钮。
- 清空人脸数据（`DELETE /api/faces`）后立即重训并清掉缓存分数——之前模型和分数会一直基于已删除的人脸特征。
- 测试：`tests_m4::taste_forget_drops_one_photo_or_one_session_and_refits`、`tests_m6::clearing_face_data`（植入一条 `taste_score` 后清空人脸，断言表为空）、`ip-server/tests/m4.rs` 的 HTTP 路径；web `vitest` 595 通过。
- **后续**：真实图库上跑 `imagepicker taste --eval` 复核 AUC 门槛（需要负责人的数据）；`sort=taste` 未建索引。

## 2026-10-22

### 合入两条长期分支：#72 换发型、#73 自适应预设

- **换发型（F6.10）**：worker 侧按人物/人脸身份校验 donor，`HairIdentity` 随合成结果回传；`PatchKind::layer_order()` 让发型补丁排在其它补丁之上（`ip-render` 合成稳定排序）。
- **自适应预设**：预设强度缩放补上 `dodge` / `burn` / `texture`（#86 修的是这条分支合入后暴露的 main 回归）；`ort` 升级到 2.0.0-rc.14 后 `Session::builder` 需要进程级 `Environment`（#85）。
- 两条分支各有一处测试随场景调整（hair 的拒绝码、m5 接受两种缺失原因）。

### 取消生成任务真正中断线程池（#87）

- `cancel` / `$/cancel` 通知除了取消 asyncio 任务，还给该请求的协作式 `Cancel` 令牌置位；`inpaint`（按蒙版簇）、`enhance`（按瓦片、逐人脸、去汗光/碎发）、`besttake`、`hair`（开始转移前）都会检查令牌，`Cancelled` 统一映射回 `-32800`。线程池里的计算不再在请求已结束后继续跑。
- 测试用真实 WebSocket：取消后客户端收到 `-32800`，执行线程不到 1 秒退出且工作未完成（`ai-worker/tests/test_cancel.py`）。

### 助手的数值指令与模型计划校验（#87、#89）

- 新工具 `adjust_sliders`：规则引擎解析中英文「把曝光提高 0.5」「lower the contrast by 10」「把对比度设为 20」，区分相对/绝对；执行时合并进每张照片的 `global` op（保留曲线、HSL、分级），按编辑器范围夹取并快照以便整份计划一次撤销；摘要与确认文本、web 类型/i18n/mock 同步。
- `llm_tool_mismatch`：规则引擎已确定识别出请求点名的预设或滑块数值、而模型计划没有对应工具时，整份模型计划退回规则引擎（修「暖调胶片预设」被映射成 `apply_profiles` 的实测问题）。

### HEIC 方向与色彩（#88）

- WIC 会自行应用容器里的 `irot`/`imir`（包括不换轴的镜像与 180°），旧代码只按「长边换轴」判断，导致 `o2.heic` 被镜像两次。Windows 上改为「输出尺寸等于显示尺寸即视为已应用方向」，并有单测。
- 同一轮实测发现 WIC 的 HEVC 解码与 libheif 色彩相差数十级（Display P3 文件绿通道差 45），Windows 改为 **libheif 优先、WIC 兜底**。`real_heic_files_match_libheif_display` 在本机全部通过（8 种方向、缩略图、10bit、P3）。

### 接手文档与开发日志

- 新增 [接手文档](handover.md)（当前状态、代码地图、不变量、本机门禁与基准、CI/发布、未完成事项）与本文；`CONTRIBUTING.md` 与 `docs/README.md` 加入口。

### 清理

- 删除两个陈旧 worktree、已合并的本地分支、两天前遗留的 `dist/server` 与别的项目的 vite 开发服务器（不进仓库，仅记录在此）。
