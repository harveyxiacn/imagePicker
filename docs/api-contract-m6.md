# API 契约 · M6（AI 助手 · 局域网 WebUI · XMP · 设置）

> 在 M1–M5 契约基础上**增量**扩展。依据：[03 §9](03-AI与算法设计.md)（AI 助手工具调用）、[02 §8](02-架构设计.md)（安全与隐私）、[05 §5](05-数据与接口设计.md)（XMP 互通）、[04 §3.8](04-UI-UX设计.md)（助手抽屉）、§7（新手引导）。

## A. AI 助手

### A.1 工具（与 REST 一一对应，执行时调用同一套 Core 方法）

| 工具 | 参数 | 说明 |
|---|---|---|
| `filter` | 与 `/api/photos` 查询参数相同的对象（含人物、问题、场景、星级、旗标、`collection`） | 只改变界面筛选，不改数据 |
| `set_rating` | `{selection, rating: 0..5 \| null}` | |
| `set_flag` | `{selection, flag: -1\|0\|1}` | |
| `accept_ai` | `{selection}` | |
| `group_keep_top` | `{selection?, n, reject_rest: bool}` | 每个连拍组保留前 n |
| `scene_keep_top` | `{selection?, n, reject_rest: bool}` | 每个场景保留前 n |
| `apply_preset` | `{selection, preset_id}` | |
| `auto_adjust` | `{selection, mode}` | 逐张 AI 一键并保存 |
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
| POST | `/api/assistant/plan` | `{"session_id", "message", "context": {"filter": "<查询串>", "selection": number[], "current_photo_id": number \| null, "locale": "zh-CN" \| "en"}, "engine"?: "auto" \| "rules" \| "llm"}` | `{"plan_id", "reply": string /*给用户看的一句话*/, "steps": [{"tool", "args", "summary": string, "affects": number /*受影响照片数*/, "destructive": bool}], "needs_confirmation": bool, "engine": "rules"\|"llm", "unsupported": string \| null}` |
| POST | `/api/assistant/execute` | `{"plan_id"}` | `202 {"task_id"}`；逐步执行，进度 `task.progress kind:"assistant"`；结束广播 `assistant.done {plan_id, ok, results:[{tool, ok, affected, error}], undo: {"edits": [{photo_id, before}], "photos": [{id, user_rating, flag, color_label}]}}`，前端据此生成一次撤销 |
| POST | `/api/assistant/describe` | `{"photo_id"}` | `{"caption", "keywords": string[]}`（VLM；不可用 → 409/503） |

- **规则引擎（T0 必备）**：中英文意图模板 + 实体抽取（人物名、星级、数量、场景词、问题词、预设名、"每个场景/每组"、"闭眼/模糊"等），覆盖常用指令；无法理解时 `unsupported` 给出原因与示例。
- **LLM 引擎（可选）**：worker `llm.plan` 产出同一工具 JSON；Core **校验**工具名/参数后才返回计划，绝不直接执行模型输出。
- `filter` 步骤只回传给前端应用；破坏性步骤（淘汰、批量改星、导出）`destructive:true`，必须确认。

### A.3 Worker 新方法

| 方法 | 参数 | 结果 |
|---|---|---|
| `llm.plan` | `{"message", "tools": [JSON schema], "context": {...}, "locale"}` | `{"reply", "calls": [{"tool", "args"}]}` |
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

- 设置项 `xmp_mode`: `"off" \| "sidecar" \| "sidecar_and_embedded"`（默认 `"off"`）。
- **读**：导入时读取 `<name>.xmp`（以及 `<name>.<ext>.xmp`）与 JPEG 内嵌 XMP 中的 `xmp:Rating`（−1 = 淘汰）、`xmp:Label`、`dc:subject`/`lr:hierarchicalSubject`（关键词，存入 tag）。
- **写**：用户评分/旗标/色标/关键词变化后去抖写入（保留侧车中的其他内容，原子写入）；淘汰写 `xmp:Rating="-1"`（darktable 约定，与 Lightroom 兼容）。
- 冲突：侧车 mtime 晚于目录库记录 → 以侧车为准并提示；`POST /api/xmp/sync {"session_id", "direction": "read" \| "write"}` 手动同步。

## D. 设置

`GET /api/settings` / `PATCH /api/settings`：

```jsonc
{
  "language": "zh-CN",
  "theme": "dark" | "light" | "system",
  "analysis": { "default_profile": "standard", "auto_analyze_on_import": false, "group_strictness": "normal" /* loose|normal|strict */ },
  "faces": { "enabled": true },            // false：不做人脸识别；DELETE /api/faces?confirm=true 清除全部人脸数据
  "privacy": { "allow_network": true },    // false：禁止一切下载
  "models": { "dir": "...", "source": "auto" | "hf" | "hf-mirror" | "modelscope" },
  "cache": { "max_gb": 20 },
  "render": { "backend": "auto" | "gpu" | "cpu" },
  "xmp_mode": "off",
  "lan": { "enabled": false, "port": 7878, "guest_enabled": false },  // 密码通过 POST /api/auth/password 设置
  "roots": ["C:/Users/me/Pictures"],
  "assistant": { "engine": "auto" }
}
```

- `GET /api/cache` → `{"bytes", "items": {"thumbs", "previews", "masks", "edits", "gen"}}`；`POST /api/cache/clear {"kinds": [...]}`。
- `DELETE /api/models/{id}` 删除已下载模型。
- `GET /api/onboarding` → `{"first_run": bool, "hardware": {...}, "recommended_tier", "recommended_download_mb"}`；`POST /api/onboarding/done`。

## E. WebSocket 新增事件

```jsonc
{ "type": "assistant.done", "plan_id": "...", "ok": true, "results": [...], "undo": {...} }
{ "type": "settings.updated", "settings": {...} }
{ "type": "xmp.conflict", "photo_id": 12, "sidecar": {"rating": 3}, "catalog": {"rating": 4} }
```
