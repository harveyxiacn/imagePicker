# API 契约 · M6（AI 助手 · 局域网 WebUI · XMP · 设置）

> 在 M1–M5 契约基础上**增量**扩展。依据：[03 §9](03-AI与算法设计.md)（AI 助手工具调用）、[02 §8](02-架构设计.md)（安全与隐私）、[05 §5](05-数据与接口设计.md)（XMP 互通）、[04 §3.8](04-UI-UX设计.md)（助手抽屉）、§7（新手引导）。

## A. AI 助手

### A.1 工具（与 REST 一一对应，执行时调用同一套 Core 方法）

| 工具 | 参数 | 说明 |
|---|---|---|
| `filter` | 与 `/api/photos` 查询参数相同的对象（含人物、问题、场景、星级、旗标、拍摄日期、`collection`）；`scene_type` 只能是分析的 8 种场景之一（portrait/group/landscape/architecture/night/food/pet/other，其他值 → 校验失败）；`taken_from` / `taken_to` 为 `YYYY-MM-DD`（拍摄地当地日期，含两端，与 `/api/photos` 同义）；请求里没写年份的日子（「10月2日」）和相对日（「上周六」）由规则引擎按会话自己的拍摄日期解析成这两个键（见 A.2），不取墙上时钟；**没有地点 / 事件键**（见 A.2「地点与事件」） | 只改变界面筛选，不改数据 |
| `set_rating` | `{selection, rating: 0..5 \| null}` | |
| `set_flag` | `{selection, flag: -1\|0\|1}` | |
| `accept_ai` | `{selection}` | |
| `group_keep_top` | `{selection?, n, reject_rest: bool}` | 每个连拍组保留前 n |
| `scene_keep_top` | `{selection?, n, reject_rest: bool}` | 每个场景保留前 n |
| `apply_preset` | `{selection, preset_id}` | 发给模型的 schema 中 `preset_id` 为当前存在的预设 id 枚举（内置 + 用户预设） |
| `auto_adjust` | `{selection, mode}` | 逐张 AI 一键并保存 |
| `adjust_sliders` | `{selection, values: {滑块: 数值, ...}, relative?: bool}` | 按数值调滑块（曝光 −5..5，色温 −3000..3000，减淡/加深 0..100，其余 −100..100）；`relative:true` 在当前值上增减，否则直接设为该值；只改所指滑块，现有曲线/HSL/分级与其它滑块保留 |
| `apply_profiles` | `{selection}` | 人物美颜档案 |
| `besttake_auto` | `{selection}` | 对所选连拍组一键全员最佳 |
| `remove_bystanders` | `{selection}` | |
| `export` | `{selection, preset?: "original"\|"wechat"\|"xiaohongshu"\|"instagram", dest?}` | 无 `dest` 时由界面询问 |
| `describe` | `{photo_id}` | VLM 描述（仅高端档） |
| `suggest_edits` | `{photo_id}` | VLM 修图建议，返回 Adjust（不保存） |

`selection` = `{"ids": [...]}` 或 `{"query": "<photos 查询串>"}` 或 `"current_filter"`（由前端在请求上下文中提供当前筛选）。

### A.2 接口

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/assistant/status` | — | `{"engine": "rules" \| "llm", "llm_model": string \| null, "vlm_model": string \| null, "llm_available": bool, "vlm_available": bool}` |
| POST | `/api/assistant/plan` | `{"session_id", "message", "context": {"filter": "<查询串>", "selection": number[], "current_photo_id": number \| null, "locale": "zh-CN" \| "en"}, "engine"?: "auto" \| "rules" \| "llm"}` | `{"plan_id", "reply": string /*给用户看的一句话*/, "steps": [{"tool", "args", "summary": string, "affects": number /*受影响照片数*/, "destructive": bool, "criteria": string \| null /*作用的照片，文字描述*/, "large_impact": bool}], "needs_confirmation": bool, "engine": "rules"\|"llm", "unsupported": string \| null, "large_impact": bool, "confirm_text": string \| null, "session_photos": number, "fallback": {"code", "message"} \| null}` |
| POST | `/api/assistant/execute` | `{"plan_id", "confirm"?: bool}`（`large_impact` 的计划必须 `confirm:true`，否则 409 `confirmation_required`，计划保留可再次执行） | `202 {"task_id"}`；逐步执行，进度 `task.progress kind:"assistant"`；结束广播 `assistant.done {plan_id, ok, results:[{tool, ok, affected, error}], undo: {"edits": [{photo_id, before}], "photos": [{id, user_rating, flag, color_label}]}}`，前端据此生成一次撤销 |
| POST | `/api/assistant/describe` | `{"photo_id"}` | `{"caption", "keywords": string[]}`（VLM；不可用 → 409/503） |

- **规则引擎（T0 必备）**：中英文意图模板 + 实体抽取（人物名、星级、数量、场景词、问题词、预设名、"每个场景/每组"、"闭眼/模糊"等），覆盖常用指令；无法理解时 `unsupported` 给出原因与示例。
- **数值型修图指令**：提到具体滑块与数值的中英文请求（“把曝光提高 0.5”“lower the contrast by 10”“把对比度设为 20”）由规则引擎直接生成 `adjust_sliders`；没有数值的“一键修图”才走 `auto_adjust`。
- **无年份日期与相对日**（`assistant/rules/days.rs`，不读系统时钟，只看会话照片的拍摄日 `photo.taken_at` 的日期部分，与 `taken_from` / `taken_to` 同一口径；会话里没有任何带拍摄时间的照片 → 提示「无法确定是哪一天」，不加日期条件）：
  - **月日**（「10月2日」「十月二日」「10月2号」、`Oct 2`、`2nd of October`）：取会话里**有这一天照片的最近一年**；没有哪一年有，就取会话**最晚的一年**中该日期存在的那年（2月29日会跳过平年）。请求里写了年份的（`2026年10月2日`、`Oct 2 2021`）原样使用。
  - **相对日**以会话**最晚的拍摄日**为基准日：今天 / 昨天 / 前天 / 大前天（today / yesterday / the day before yesterday）；一周从周一到周日。中文「上周六」「这周三」「上周」「上周末」「这周」取上周 / 本周（基准日所在周）的那天或整段；英文 `last Saturday` 取基准日**之前**最近的一个周六、`this Friday` 取基准日所在周的周五，`last week` / `last weekend` / `this week` 与中文同；「上个月 / 这个月」（last month / this month）取基准日之前一个月 / 所在月的整月。
  - 解析出的日子按 `taken_from` / `taken_to` 返回（一天 = 两端相同，一段 = 首尾）；`named_days(msg, days)` 用同一套解析，所以模型给出的、与规则解析一致的日期照常被接受，其余日期仍按 `llm_arg_mismatch` 拒绝。解析出的日子在 `llm.plan` 的上下文里以 `dates` 给模型（见 A.3），让它照抄而不是自己猜年份。
- **地点与事件**：库里没有可筛选的地点名或事件名——GPS 只有坐标（`photo.gps_lat/gps_lon`），没有反向地理编码；`event.title` / `event.place` 没有任何界面或 API 可写，事件默认名只是日期时间段；事件 id 每次重算都会重新分配，所以 `event_id` 虽然是 `/api/photos` 的参数，却不能写进计划（执行时会指向别的事件）。因此规则引擎和模型的 `filter` 都**没有**地点 / 事件键；请求里的地点（「在京都拍的」、`from the beach`）仍按提示「已忽略」处理，有其他条件时保留其余部分，只有地点时给出 `unsupported`。要支持它需要：离线反向地理编码或 EXIF 城市字段 → 写入 `event.place`（或照片级地点列）；给事件命名的界面 / API；稳定的事件 id（或按标题 / 地点匹配）；`/api/photos` 的地点 / 事件标题筛选参数，再接入 `FILTER_KEYS` 与模型 schema。
- **LLM 引擎（可选）**：worker `llm.plan` 产出同一工具 JSON；Core **校验**工具名/参数后才返回计划，绝不直接执行模型输出。
- `filter` 步骤只回传给前端应用；破坏性步骤（淘汰、批量改星、导出）`destructive:true`，必须确认。

### A.4 目标照片与大范围修改

- **目标须有依据**：每个改动照片的步骤，其目标只能是：同一计划中前面 `filter` 步骤的结果（`selection:"current_filter"` 与缺省 selection 的 keep-top 绑定到该筛选，改写为显式 `{"query"}`，执行时按同一条件取照片，不再取发请求时的网格视图）、带条件或 `limit` 的显式 query、用户已选中/当前打开的 `ids`，或——仅当请求本身没有任何条件时——当前视图/整个会话。
- **LLM 计划的校验**：模型步骤若没有自己的条件却会作用于整个视图/会话，而规则引擎对同一请求的解读并非“无条件作用于全部”（或规则也没读懂），则整份模型计划被拒，退回规则引擎，`fallback.code = "llm_unsafe_target"`；ids 既未选中也未打开 → 同样拒绝。其余退回原因：`llm_failed`、`llm_no_steps`、`llm_invalid`（引用了不存在的预设/人物）、`llm_tool_mismatch` 与 `llm_arg_mismatch`（规则引擎作为校验：规则对同一请求的解读确定——无提示、非 unsupported——时，规则计划中每个改动照片的步骤必须按顺序出现在模型计划中，模型计划里某个改动工具的步骤数也不得超过规则读到的与请求文字点名的次数（`Parsed.mentioned`），否则 `llm_tool_mismatch`，例如点名的预设被答成 `apply_profiles`、“曝光提高 0.5”被答成 `auto_adjust`、只是筛选的请求却被答成修改；对应步骤的意图参数——`set_flag.flag`、`set_rating.rating`、`apply_preset.preset_id`、`adjust_sliders.values/relative`、keep-top 的 `n/reject_rest`、请求点名的 `export.preset`——不一致，或规则 `filter` 步骤的条件在模型的 `filter` 步骤里缺失/被改，则 `llm_arg_mismatch`；模型 `filter` 步骤里的 `taken_from` / `taken_to` 只能是请求写出的日期，或规则按上面的规则从无年份日期 / 相对日解析出的日子，否则 `llm_arg_mismatch`；模型多出的步骤允许，照常校验与确认；规则读不懂或只能近似理解的请求仍交给模型）。`engine:"llm"` 且模型不可用仍返回 409 `models_missing` / 503 `worker_unavailable`。LLM 上下文含最多 200 个选中 id。
- **大范围修改**：改动照片数 > 500，或 ≥ 20 张且超过会话照片数一半的步骤 `large_impact:true`；计划 `needs_confirmation:true`，`confirm_text` 逐条列出需确认的步骤（数量、作用的照片、占比）。执行必须 `confirm:true`（CLI `--execute` 视为确认）。
- **执行期上限**：步骤执行时重新解析目标，若照片数超过计划时计数（`affects`），该步骤以 409 失败（“重新提问”），不会改动超出用户所见范围的照片。

### A.3 Worker 新方法

| 方法 | 参数 | 结果 |
|---|---|---|
| `llm.plan` | `{"message", "tools": [JSON schema], "context": {...含 "dates": [{"phrase","from","to"}]：请求里无年份日期 / 相对日的解析结果，模型须照抄}, "locale"}` | `{"reply", "calls": [{"tool", "args"}]}` |
| `vlm.suggest` | `{"photo": {...}, "context": {"histogram", "scores", "scene_type"}}` | `{"problems": string[], "adjust": Adjust, "reason": string}` |
| `vlm.describe` | `{"photo": {...}, "locale"}` | `{"caption", "keywords"}` |

模型：宽松许可的小模型（如 Qwen2.5/Qwen3 1.5–4B Instruct、Qwen2.5-VL-3B，Apache-2.0）；作为可选 `llm` 增强包，按档位启用（T2+）。

## B. 局域网 WebUI 与鉴权

- `imagepicker serve --lan [--password <p>]` 或设置页开启：绑定 `0.0.0.0`；首次开启必须设置密码（Argon2id 存储于数据目录）。
- **桌面模式**：Tauri 生成的会话 token 通过 `Authorization: Bearer` / `?token=`（仅首次加载）/ HttpOnly Cookie 校验，**本机请求也必须带 token**（M1 时留下的 TODO）。
- **局域网访问**：`POST /api/auth/login {"password"}` → 设置 HttpOnly、SameSite=Strict 会话 Cookie；`POST /api/auth/logout`；`GET /api/auth/me` → `{"role": "owner" \| "guest" \| null, "lan": bool}`。登录失败限速（每 IP 每分钟 5 次）。
- **访客**：可选的只读访客密码；访客不能修改评分/编辑/导出/设置，看不到人物页与人脸数据（02 §8）。
- 除 `/api/health`、`/api/auth/*` 与静态资源外，所有 `/api/*` 需要已认证；未认证 → `401 {"error":{"code":"unauthorized"}}`。WebSocket 同样校验。
- **路径白名单**：`/api/fs/*`、`/api/import`、导出目录、LUT 导入仅允许设置中配置的根目录（默认：用户主目录、图片目录、已导入的根目录、可移动磁盘）；越界 → 403。
- `GET /api/system/lan` → `{"enabled", "urls": ["http://192.168.1.10:7878"], "qr_svg": "<svg…>"}`（扫码访问）。

## C. XMP 侧车互通

- 设置项 `xmp_mode`: `"off" \| "sidecar" \| "modify_originals"`（默认 `"off"`）。
- **读**（`sidecar` 与 `modify_originals`）：导入时读取 `<name>.xmp`（以及 `<name>.<ext>.xmp`），没有侧车时读取 JPEG 内嵌 XMP，取 `xmp:Rating`（−1 = 淘汰）、`xmp:Label`、`dc:subject`/`lr:hierarchicalSubject`（关键词，存入 tag）。
- **写**：用户评分/旗标/色标/关键词变化后去抖写入侧车（保留侧车中的其他内容，原子写入）；淘汰写 `xmp:Rating="-1"`（darktable 约定，与 Lightroom 兼容）。只有 `modify_originals` 还会改写 JPEG 原图内嵌的 XMP 段（其余字节不变，mtime 保留）——这是唯一会改变原图字节的设置，`PATCH /api/settings` 切换到它时必须在同一请求中带 `"confirm_modify_originals": true`（不存储），否则 `422 unprocessable`。
- 旧值 `"sidecar_and_embedded"`（旧界面称「同上，并读取内嵌 XMP」，实际会改写原图）在加载设置时改为 `"sidecar"`；`PATCH` 不再接受它。
- 冲突：侧车 mtime 晚于目录库记录 → 以侧车为准并提示；`POST /api/xmp/sync {"session_id", "direction": "read" \| "write"}` 手动同步。

## D. 设置

`GET /api/settings` / `PATCH /api/settings`：

```jsonc
{
  "language": "zh-CN",
  "theme": "dark" | "light" | "system",
  "analysis": { "default_profile": "standard", "auto_analyze_on_import": false, "group_strictness": "normal" /* loose|normal|strict */ },
  "faces": { "enabled": true, "consented": false },  // enabled=false：不做人脸识别；consented：用户已看过用途说明并同意（默认 false，同意前不检测人脸、不计算人脸特征）；DELETE /api/faces?confirm=true 清除全部人脸数据
  "privacy": { "allow_network": true },    // false：禁止一切下载
  "models": { "dir": "...", "source": "auto" | "hf" | "hf-mirror" | "modelscope" },
  "cache": { "max_gb": 20 },
  "render": { "backend": "auto" | "gpu" | "cpu" },
  "xmp_mode": "off",                        // off | sidecar | modify_originals（切换到 modify_originals 需 "confirm_modify_originals": true）
  "lan": { "enabled": false, "port": 7878, "guest_enabled": false },  // 密码通过 POST /api/auth/password 设置
  "roots": ["C:/Users/me/Pictures"],
  "assistant": { "engine": "auto" }
}
```

- `GET /api/cache` → `{"bytes", "items": {"thumbs", "previews", "masks", "edits", "gen"}, "data_dir", "data_dir_source", "portable"}`（`data_dir` 为数据目录绝对路径，`portable` 为便携模式，`data_dir_source` ∈ `explicit` / `env` / `portable` / `platform`）；`POST /api/cache/clear {"kinds": [...]}`。
- `DELETE /api/models/{id}` 删除已下载模型。
- `GET /api/onboarding` → `{"first_run": bool, "hardware": {...}, "recommended_tier", "recommended_download_mb"}`；`POST /api/onboarding/done`。

## E. WebSocket 新增事件

```jsonc
{ "type": "assistant.done", "plan_id": "...", "ok": true, "results": [...], "undo": {...} }
{ "type": "settings.updated", "settings": {...} }
{ "type": "xmp.conflict", "photo_id": 12, "sidecar": {"rating": 3}, "catalog": {"rating": 4} }
```

## F. 目录库完整性与备份（数据安全）

- 启动：`catalog.db` 打不开（SQLite 报告损坏 / 不是数据库）时移到 `<数据目录>/backups/corrupt-<时间>.db`，用空目录库启动，状态 `recovery`；能打开则后台执行 `PRAGMA quick_check`（状态 `checking` → `ok` / `damaged`）。
- 每天一次自动备份 `VACUUM INTO backups/catalog-<UTC 时间>.db`，保留最近 7 份；状态不是 `ok` 或目录库没有照片时不自动备份。
- `GET /api/catalog` →
  ```jsonc
  { "state": "checking" | "ok" | "damaged" | "recovery",
    "problems": ["..."],            // quick_check 结果或打不开的原因
    "moved_to": "…/backups/corrupt-20261009-080503-007.db" | null,
    "checked_at": 1760000000000 | null, "restored_from": "catalog-….db" | null,
    "last_backup_at": 1760000000000 | null, "backup_dir": "…/backups",
    "backups": [{ "name": "catalog-20261009-080503-007.db", "kind": "auto" | "replaced" | "corrupt", "created_at": 1760000000000, "bytes": 18400000 }] }  // 新的在前
  ```
- `POST /api/catalog/backup` → 新备份的 `{name, kind, created_at, bytes}`；`recovery` 状态下 `409`（先恢复，避免空目录库挤掉好的备份）。
- `POST /api/catalog/restore {"name"}` → 恢复后的状态。备份须是可读、`quick_check` 通过、表结构不新于当前版本的目录库，否则 `422`；名称不合法 `400`、不存在 `404`；分析运行中或连接 30 秒内不释放 `409`。当前目录库另存为 `backups/replaced-<时间>.db`（`replaced`、`corrupt` 各保留 3 份）。客户端恢复后重新加载页面。
- 仅所有者可用（访客 `403`）。

## G. 日志、崩溃记录与诊断包（运维，09 P1-13）

- **日志**：`ip-cli serve` 与桌面应用把 `tracing` 日志同时写到 stderr 和 `<数据目录>/logs/imagepicker-<UTC 日期>.<序号>.log`。单个文件达到 5 MiB 或跨 UTC 日期时换新文件；超过 14 天的文件、以及总量超过 40 MiB 时最旧的文件被删除。过滤用 `RUST_LOG`（默认 `info`）。Android 仍只写控制台。AI 组件安装日志仍是 `logs/runtime-install.log`。
- **崩溃记录**（`<数据目录>/crashes/`）：
  - Rust panic：默认 panic 钩子运行**之前**写 `crash-<UTC 时间>-<pid>-<n>.txt`（`kind`、`time`、`version`、`os`、`pid`、`thread`、`location`、`message`、`backtrace`）。保留最近 30 份。
  - AI worker：核心启动 worker 时传入环境变量 `IMAGEPICKER_CRASH_DIR`；Python 未处理的异常（主线程、其他线程、asyncio 任务）写 `worker-crash-*.txt`，原生崩溃（段错误等）由 `faulthandler` 写入 `worker-native-*.log`（正常退出且为空时删除）。
  - worker 进程意外退出或启动失败时，核心把其 stderr 末尾的最多 30 行写成 `crash-*.txt`（`kind` 为 `worker-exit` / `worker-start-failed`）。
- `POST /api/system/diagnostics`（无请求体）→ `200 application/zip`，`Content-Disposition: attachment; filename="imagepicker-diagnostics-<UTC 日期>-<时间>.zip"`，`Cache-Control: no-store`。仅所有者可用：访客与已配对设备 `403`（`system` 在访客拒绝列表中），未登录 `401`。内容（全部是文本，不含任何二进制图像数据）：

  | 条目 | 内容 |
  |---|---|
  | `manifest.json` | 格式版本、生成时间、各文件大小、「不包含」清单 |
  | `system.json` | 应用版本、AI 组件（runtime）版本、操作系统 / 架构、逻辑 CPU 数、渲染后端、worker 状态 / 档位 / 计算设备 / GPU 名称与显存 |
  | `settings.json` | `GET /api/settings` 的内容；键名含 `password`、`token`、`secret`、`api_key`、`hash`、`salt`、`cookie`、`authorization`、`credential` 的值替换为 `"[redacted]"`，`*_url` 只保留「已设置」，`roots` 只保留数量与文件夹名称，其余字符串里的绝对路径被遮盖 |
  | `catalog.json` | `PRAGMA user_version` 与期望版本、完整 `PRAGMA integrity_check` 结果、目录库状态、各表计数（照片、缺失照片、会话、已分析、有编辑、连拍、人脸、人物、标签、预设、智能相册、任务）、根文件夹名称与各自的照片数、目录库与缓存的字节数、最近 20 条任务的种类 / 状态 / 错误 / 时间 |
  | `logs/*.log` | 最新的日志文件，每个文件最多取末尾 1 MiB、合计 4 MiB（从整行开始） |
  | `crashes/*` | 崩溃记录，每个最多末尾 256 KiB、合计 2 MiB |

  日志与崩溃记录逐行经过遮盖：绝对路径（Windows 盘符 / UNC / 常见 POSIX 前缀）的目录部分替换为 `<path>`，图片与 `.xmp` 文件名替换为 `<photo>`；`password=…`、`token=…`、`Bearer …`、URL 里的 `user:pass@` 替换为 `[redacted]`。这是尽力而为的正则遮盖，不保证覆盖所有格式。
  **永不包含**：照片、缩略图、预览、人脸裁剪、蒙版、人脸或照片的嵌入向量、目录库数据库及备份、任何照片路径或文件名、密码、哈希、令牌与已配对设备。
