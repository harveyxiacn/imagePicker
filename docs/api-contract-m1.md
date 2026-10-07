# API 契约 · M1（前后端联调基准）

> 本文件是 `crates/ip-server` 与 `web/` 之间的**唯一约定**。改动须同时更新两端。
> 完整远期设计见 [05 数据与接口设计](05-数据与接口设计.md)；本文件只包含 M1 实际实现的子集。

## 通用

- 基址：`/api`。服务默认监听 `127.0.0.1:7878`（`imagepicker serve --port`）。
- 鉴权：M1 仅本机访问，不鉴权；预留 `Authorization: Bearer <token>`，服务端先忽略。
- JSON 字段一律 `snake_case`；时间为 Unix 毫秒整数；ID 为整数。
- 错误：HTTP 4xx/5xx + `{"error": {"code": "not_found", "message": "..."}}`。
- 静态资源：服务端在非 `/api` 路径上提供 `web/dist`（SPA，未知路径回退到 `index.html`）。开发时前端使用 Vite 代理 `/api` → `http://127.0.0.1:7878`（含 WebSocket）。

## 数据类型

```ts
type Flag = -1 | 0 | 1;              // -1 淘汰, 0 无, 1 精选
type ColorLabel = "red" | "yellow" | "green" | "blue" | "purple" | null;
type ImageFormat = "jpeg" | "png" | "webp" | "heif" | "avif" | "tiff" | "raw";

interface Photo {
  id: number;
  session_id: number;
  path: string;                 // 绝对路径（仅展示用）
  file_name: string;
  format: ImageFormat;
  file_size: number;
  width: number | null;         // 已按朝向旋转后的显示宽高（EXIF 朝向 5–8 已交换宽高）
  height: number | null;
  taken_at: number | null;      // ms（UTC 瞬间；无时区信息时为按 UTC 编码的拍摄地墙上时间）
  taken_at_offset_min: number | null; // 拍摄时 UTC 偏移（分钟，EXIF OffsetTimeOriginal）；UI 显示 taken_at+offset 的 UTC 渲染，即拍摄地当地时间
  camera: string | null;        // 规范化设备名："Sony ILCE-7M4" / "Apple iPhone 15 Pro"（见「设备」）
  device_id: number | null;     // 拍摄设备；无 EXIF 机型/序列号时为 null
  device_kind: "phone" | "camera" | "drone" | "action" | "unknown" | null; // device_id 为 null 时为 null
  lens: string | null;
  focal_mm: number | null;
  aperture: number | null;
  shutter_s: number | null;
  iso: number | null;
  user_rating: number | null;   // 0..5, null=未评分
  ai_rating: number | null;     // 0..5（0.5 步长），M1 恒为 null
  flag: Flag;
  color_label: ColorLabel;
  burst_id: number | null;      // M1 恒为 null
  thumb_ready: boolean;         // 256 网格缩略图已生成
  thumb_version: string;        // 缓存破坏用，拼到 URL 上 ?v=（当前为 fast_key 前 8 位）
}

interface Session {
  id: number;
  title: string;
  root_path: string;
  created_at: number;
  photo_count: number;
  picked_count: number;         // flag = 1
  rejected_count: number;       // flag = -1
  rated_count: number;          // user_rating not null
  cover_photo_id: number | null;
  import_state: "scanning" | "thumbnailing" | "ready";
}
```

## 接口

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/health` | — | `{"ok":true,"version":"0.1.0"}` |
| POST | `/api/import` | `{"path": string, "recursive"?: bool=true, "title"?: string}` | `201 {"session": Session}`；扫描在后台继续，照片分批出现（见事件） |
| GET | `/api/sessions` | — | `{"sessions": Session[]}`（按 created_at 倒序） |
| GET | `/api/sessions/{id}` | — | `{"session": Session}` |
| DELETE | `/api/sessions/{id}` | — | `204`（仅删除目录库记录与缓存，**绝不删除原图**） |
| GET | `/api/sessions/{id}/devices` | — | `{"devices": SessionDevice[]}`（见「设备」；会话不存在 404） |
| GET | `/api/photos` | 查询参数见下 | `{"photos": Photo[], "total": number, "next_cursor": string \| null}` |
| GET | `/api/photos/{id}` | — | `{"photo": Photo}` |
| PATCH | `/api/photos` | `{"ids": number[], "user_rating"?: number \| null, "flag"?: Flag, "color_label"?: ColorLabel}` | `{"updated": number}`；同时广播 `photos.updated` |
| GET | `/api/thumb/{id}?s=256\|512&v=...` | — | `image/jpeg`；未生成时 **同步生成**后返回（插队）；`Cache-Control: public, max-age=31536000, immutable` |
| GET | `/api/preview/{id}?s=2048` | — | `image/jpeg`，按需生成并缓存（s 允许 1024/2048/4096） |
| GET | `/api/original/{id}` | — | 原文件字节流（Content-Type 按格式；RAW 用 `application/octet-stream`） |
| POST | `/api/viewport` | `{"ids": number[]}` | `204`；提升这些照片缩略图生成优先级 |
| POST | `/api/export` | `{"ids": number[], "dest": string, "long_edge"?: number \| null, "quality"?: number=90, "name_template"?: string="{name}"}` | `202 {"task_id": string}`；进度走事件 |
| GET | `/api/fs/roots` | — | `{"roots": string[]}`（Windows 盘符 / `/` 与用户目录） |
| GET | `/api/fs/list?path=` | — | `{"path": string, "parent": string \| null, "dirs": string[], "image_count": number}`（用于 WebUI 选择文件夹） |

### `GET /api/photos` 查询参数

| 参数 | 说明 |
|---|---|
| `session_id` | 必填 |
| `rating_gte` | 0..5，user_rating ≥ N（未评分视为不满足，除非 N=0） |
| `flag` | `picked` / `rejected` / `unflagged` / `not_rejected` |
| `color_label` | 颜色名 |
| `sort` | `taken_at`（默认，升序，null 排最后）/ `-taken_at` / `name` / `rating`（降序） |
| `cursor` | 不透明字符串（上一页的 `next_cursor`） |
| `limit` | 默认 500，最大 5000 |
| `device` | 设备 id，逗号分隔（`device=3,5`，OR）；`none` = 没有设备的照片（可与 id 混用：`3,none`）；其他值 → 400。可与其余过滤、排序组合；`total` 为过滤后的数量 |

> 前端会一次性按 5000 分页把整个会话的 ID 与字段拉完（2 万张以内），客户端虚拟化渲染。

## 设备

拍摄设备（相机 / 手机 / 无人机 / 运动相机）由 EXIF Make/Model/序列号区分，库中 `device` 表保留**原始** make/model/serial，另存规范化的 `name` 与 `kind`。

- `photo.camera` 为规范化显示名 `"{make} {model}"`：厂商写法统一（`NIKON CORPORATION` → `Nikon`，`samsung` → `Samsung`，`HMD Global` → `Nokia` 等，单个全大写词转首字母大写，其余保持原样），型号去掉重复的厂商前缀（`NIKON Z 6` → `Nikon Z 6`，`Canon EOS R5` → `Canon EOS R5`）、折叠空白。
- `device_kind`：`phone`（Apple、Samsung `SM-`/Galaxy、Xiaomi、Huawei、HONOR、OPPO、vivo、OnePlus、Google Pixel、realme、Meizu、Nokia、Sony `XQ-`/Xperia）、`camera`（Nikon/Canon/Sony/Fujifilm/Olympus/OM System/Panasonic/Ricoh/Leica 等传统相机厂商）、`drone`（DJI 的 Mavic/Mini/Air/Phantom/FCxxxx 等）、`action`（GoPro、Insta360、DJI Osmo Action/Pocket）、`unknown`。
- 同一型号不同序列号视为不同设备（不同 `id`），名称可以相同。

```ts
interface SessionDevice {
  id: number;
  make: string | null;       // 原始 EXIF
  model: string | null;
  name: string;              // 规范化显示名，同 photo.camera
  kind: "phone" | "camera" | "drone" | "action" | "unknown";
  photo_count: number;       // 该会话内、未标记 missing 的照片数
}
```

`GET /api/sessions/{id}/devices` → `{"devices": SessionDevice[]}`，按 `photo_count` 降序、再按 `name`（不区分大小写）排序；只列出在该会话中有照片的设备。游客可读。会话不存在 → 404。会话计数（`photo_count` 等）仍为全局，不受 `device` 过滤影响。

## 行为澄清（实现后补充）

- `user_rating`：`null` = 未评分，`0` = 明确评为 0 星（与 null 不同）。`PATCH` 中字段缺省 = 不修改，显式 `null` = 清除；没有任何可修改字段时返回 400；未知 id 忽略。
- `GET /api/photos`：缺 `session_id` → 400，会话不存在 → 404；`-taken_at` 降序且 null 在后；`rating` 降序且未评分在后；`name` 不区分大小写。
- `GET /api/fs/list`：`dirs` 为**绝对路径**；缺省 `path` = 用户主目录；相对路径 → 400；跳过隐藏项。`/api/fs/roots`：主目录、图片目录、Windows 盘符（其他系统为 `/`）。
- `/api/thumb`：未生成时同步生成（插队、同一张并发请求去重）；前端仍以 `thumb_ready`/`thumbs.ready` 控制骨架屏，并通过 `viewport` 提升优先级。
- 导出：`task_id` 形如 `export-N`；模板变量 `{name}`、`{seq}`（4 位补零）、`{date}`（`YYYYMMDD`，取拍摄时间→修改时间→`nodate`）；非法字符替换为 `_`；目标文件存在时追加 `_1`、`_2`；部分失败时 `state=done` 且带 `error`，全部失败为 `failed`。
- `import_state`：scanning → thumbnailing → ready；服务重启时未完成的会话标为 ready，缩略图按需补生成。

## WebSocket：`GET /api/events`

服务端 → 客户端，JSON 文本帧；服务端对同类事件**每 100ms 合并**一次发送。

```jsonc
{ "type": "session.updated", "session": Session }
{ "type": "photos.added",    "session_id": 1, "count": 512 }        // 新增行或元数据补全后都会发送；客户端随后重新拉取
{ "type": "thumbs.ready",    "items": [{ "id": 12, "v": "a1b2c3" }] }  // 仅 256 网格缩略图
{ "type": "photos.updated",  "items": [{ "id": 12, "user_rating": 4, "flag": 1, "color_label": null }] }
{ "type": "task.progress",   "task_id": "export-1", "kind": "export", "done": 10, "total": 40, "state": "running" | "done" | "failed", "error"?: string }
```

客户端 → 服务端（可选）：`{ "type": "viewport", "ids": [..] }`，等价于 `POST /api/viewport`。

## 数据目录

- 默认：Windows `%APPDATA%\imagePicker`，macOS `~/Library/Application Support/imagePicker`，Linux `$XDG_DATA_HOME/imagePicker`（或 `~/.local/share/imagePicker`）；可用 `--data-dir` 或环境变量 `IMAGEPICKER_DATA_DIR` 覆盖。
- `catalog.db`、`cache/thumbs/`、`cache/previews/`、`logs/`。
