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
  missing?: boolean;            // 文件已不在磁盘上（见「文件」）；始终返回，缺省按 false
  companions?: Companion[];     // 与本照片配对的 RAW 文件（见「文件」）；无配对时省略
}

interface Companion { id: number; file_name: string; format: ImageFormat; file_size: number; missing: boolean }

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
| POST | `/api/import` | `{"path": string, "recursive"?: bool=true, "title"?: string, "exclude"?: string[]}` | `201 {"session": Session}`；扫描在后台继续，照片分批出现（见事件）。`exclude`（键名 `exclude[]` 亦可）见「文件」 |
| POST | `/api/sessions/{id}/rescan` | — | `202 {"started": true}`；重新扫描文件夹（新增文件导入、消失的标 `missing`），与文件监听同一逻辑 |
| POST | `/api/sessions/{id}/relocate` | `{"path": string, "dry_run"?: bool}` | `{"relocate": RelocateReport}`；见「文件」 |
| POST | `/api/photos/reject-files` | `{"session_id": number, "ids"?: number[], "mode": "trash" \| "folder", "confirm": true}` | `{"result": RejectFilesReport}`；见「文件」 |
| GET | `/api/sessions` | — | `{"sessions": Session[]}`（按 created_at 倒序） |
| GET | `/api/sessions/{id}` | — | `{"session": Session}` |
| DELETE | `/api/sessions/{id}` | — | `204`（仅删除目录库记录与缓存，**绝不删除原图**） |
| GET | `/api/sessions/{id}/filter-options` | — | `FilterOptions`（见「筛选补全」；会话不存在 404） |
| GET | `/api/sessions/{id}/devices` | — | `{"devices": SessionDevice[]}`（见「设备」；会话不存在 404） |
| GET | `/api/photos` | 查询参数见下 | `{"photos": Photo[], "total": number, "next_cursor": string \| null}` |
| GET | `/api/photos/{id}` | — | `{"photo": Photo}` |
| PATCH | `/api/photos` | `{"ids": number[], "user_rating"?: number \| null, "flag"?: Flag, "color_label"?: ColorLabel}` | `{"updated": number}`；同时广播 `photos.updated` |
| GET | `/api/thumb/{id}?s=256\|512&v=...` | — | `image/jpeg`；未生成时 **同步生成**后返回（插队）；`Cache-Control: public, max-age=31536000, immutable` |
| GET | `/api/preview/{id}?s=2048` | — | `image/jpeg`，按需生成并缓存（s 允许 1024/2048/4096） |
| GET | `/api/full/{id}?v=...` | — | `image/jpeg`：全分辨率图（转正、sRGB、照片自身像素尺寸、质量 92；已编辑的照片为全尺寸渲染），Loupe 放大超过 4096 预览所含像素时使用；按需生成并缓存在 `cache/full/`（按最近使用裁剪到 1 GB）；`Cache-Control: public, max-age=3600`；照片不存在或原图缺失 `404 not_found`，原图无法解码（无 HEIF 解码器、RAW 无内嵌预览、文件损坏）`415 unsupported_media` |
| GET | `/api/original/{id}` | — | 原文件字节流（Content-Type 按格式；RAW 用 `application/octet-stream`） |
| POST | `/api/viewport` | `{"ids": number[]}` | `204`；提升这些照片缩略图生成优先级 |
| POST | `/api/export` | `{"ids": number[], "dest": string, "long_edge"?: number \| null, "quality"?: number=90, "name_template"?: string="{name}", "pair_member"?: "jpeg" \| "raw" \| "both"}` | `202 {"task_id": string}`；进度走事件 |
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

| `lens` | 镜头名（EXIF `LensModel` 原样，区分大小写），逗号分隔，OR；名称里的逗号写 `\,`、反斜杠写 `\\`。见「筛选补全」 |
| `taken_from` / `taken_to` | 拍摄时间范围，含两端：`YYYY-MM-DD`（整天，按拍摄地当地时间）或毫秒（与 `photo.taken_at` 同单位）；没有拍摄时间的照片被排除；`from` 晚于 `to` → 400 |
| `tags` / `tag_mode` | 关键词（`photo_tag`，即 XMP `dc:subject`；同 `lens` 的转义），默认任一（`tag_mode=any`），`all` = 须包含全部；其他值 → 400 |
| `sharp` | `sharp` / `blurry` / `unknown`（清晰度状态，定义见「筛选补全」）；其他值 → 400 |
| `face_ids` / `face_similarity` | 有与这些脸（任一，最多 50 张）相似的脸的照片；`face_similarity` 为余弦相似度阈值 0..1，默认 `0.38`（聚类阈值）。见「筛选补全」 |

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

## 筛选补全（镜头、时间范围、关键词、清晰度、相似人脸）

这些参数与既有过滤、排序、分页任意组合（AND），`total` 为过滤后的数量；游标分页对它们同样稳定（排序键不变，`id` 兜底）。智能集合的 `query` 也接受它们（同样校验）。

- **镜头 `lens`。** 直接匹配 `photo.lens`（导入时从 EXIF 读入的原始字符串，不做规范化，所以 `RF24-70mm F2.8 L IS USM` 与 `RF 24-70mm F2.8 L IS USM` 是两支镜头）。没有镜头信息的照片不会被任何 `lens` 值命中。
- **时间范围。** `taken_at` 是毫秒、已应用设备偏移（即拍摄地的墙上时间当作 UTC，见 `Photo.taken_at_offset_min`），所以 `YYYY-MM-DD` 按同一把尺子换算：`taken_from` 取当天 00:00:00.000，`taken_to` 取当天 23:59:59.999。
- **关键词 `tags`。** 按 `POST /api/photos/tags` / XMP 同步写入的关键词匹配；比较前与写入时一样去掉首尾空白、去重，大小写敏感。
- **清晰度 `sharp`。** 三态互不重叠、合起来覆盖会话：`blurry` = 分析的 `sharpness` < 0.35（`BLURRY_BELOW`，与「模糊」问题标签同一阈值）**或**照片带 `blurry` 问题标签（含「比同组最清晰的一张明显更糊」）；`sharp` = 有 `sharpness` 且 ≥ 0.35 且没有 `blurry` 标签；`unknown` = 没有清晰度度量（未分析，或该步骤没有输出）且没有 `blurry` 标签。
- **相似人脸 `face_ids`。** 取这些脸的身份特征（不要求已聚类），在会话内所有有特征的脸里找余弦相似度 ≥ `face_similarity` 的，返回它们所在的照片（给定的脸所在的照片自然包含，若在该会话内）。多张脸取并集。脸不存在 → 400；脸没有身份特征（未做标准分析 / 未同意人脸识别）→ 409；`face_similarity` 超出 0..1 → 400。代价与会话内的人脸数成正比（每次查询一次线性扫描）。默认阈值 0.38 是聚类用的同一个值（同一人通常 0.43–0.66，不同人 < 0.2）；调高更严格。
- **以脸搜脸的阈值。** `POST /api/faces/search`（JSON `{face_id, session_id?, min_similarity?}` 或 multipart 字段 `min_similarity`）新增 `min_similarity`（0..1，默认 0.25，低于它的结果不返回；超出范围 → 400）。

### `GET /api/sessions/{id}/filter-options`

筛选器的选项，游客可读，会话不存在 → 404：

```ts
interface FilterOptions {
  lenses: { lens: string; photo_count: number }[];  // 未标记 missing 的照片，photo_count 降序，再按名称
  tags: { tag: string; photo_count: number }[];     // 同上，最多 500 个
  taken_min: number | null;                          // 最早 / 最晚的 taken_at（毫秒）；没有拍摄时间时为 null
  taken_max: number | null;
}
```

## 文件：RAW+JPEG、排除、监听、缺失与安全淘汰

**RAW+JPEG 配对。** 同一文件夹内主文件名相同（不区分大小写）的 RAW 与 JPEG/HEIF 导入后是**一个网格项**：显示与分析用 JPEG（JPEG 优先于 HEIF），RAW 是它的 *companion*，出现在 `Photo.companions` 里。companion 在库里有自己的 `photo` 行（`pair_id` 指向 JPEG），但不属于任何会话，所以网格、分组、分析、计数都只算一次；它不生成缩略图、不读元数据。单独的 RAW（旁边没有同名 JPEG）仍是普通网格项。

- `PATCH /api/photos` 的评分 / 旗标 / 色标同时写到 companion；开启 XMP 同步时两个文件各有自己的侧车（JPEG 的 `<名>.xmp`，RAW 的 `<文件名>.<扩展名>.xmp`，如 `IMG_1.CR2.xmp`）。
- 旧目录库（配对功能之前创建的）启动时自动配对已导入的 RAW；JPEG 还没有任何评分 / 旗标 / 色标而 RAW 有时，JPEG 沿用 RAW 的。
- `POST /api/export` 的 `pair_member`：`jpeg`（默认，网格里看到的那张，含编辑）、`raw`（改导出 RAW companion；没有配对的照片照常导出）、`both`。

**导入排除。** `exclude` 的每一项是：通配符（`*` 与 `?` 不跨目录，`**` 跨目录，如 `*.dng`、`backup/**`、`**/thumbs`）、相对于导入根目录的子文件夹路径（也可写根目录内的绝对路径，如 `trip/rejects`），或不含 `/` 的名字（任意深度下该名字的文件或文件夹）。不区分大小写。排除项与 `recursive` 一起记在会话里，之后的重新扫描（文件监听、`rescan`）沿用。名为 `_rejected` 的文件夹总是被忽略。

**文件监听。** 桌面版（`serve` / 桌面应用）监视已导入的文件夹：新增、删除、改名的图片约 1 秒静默后触发一次重新扫描，通常 2–3 秒内体现在网格（`photos.added` 事件，客户端重新拉取）；移动端不监听，可手动 `rescan`。

**缺失文件。** 文件消失的照片保留目录库记录（评分、编辑、分析都在），`missing = true`，`/api/original` 等对它返回 404，网格显示断链。文件回来（重新扫描）后自动恢复。companion 有自己的 `missing`。

**重新定位。** 文件夹被移动或改名后，`POST /api/sessions/{id}/relocate` 把会话（及共用该根目录的所有会话）指向新位置，按**相对路径 + 文件大小**重新关联，评分 / 编辑 / 分析全部保留；随后自动重新扫描以接收大小变了的和新增的文件。`path` 须在允许访问的文件夹内；`dry_run: true` 只返回统计不修改。

```ts
interface RelocateReport {
  total: number;     // 该文件夹所有照片（含 companion）的文件数
  matched: number;   // 新位置相同相对路径且大小相同
  changed: number;   // 相同相对路径但大小不同
  absent: number;    // 新位置找不到
  applied: boolean;
}
```

一张都没匹配上 → `422`；新路径已作为另一个文件夹导入 → `409`；不是文件夹 → `400`。

**安全淘汰。** `POST /api/photos/reject-files` 把会话里**已标淘汰**（`flag = -1`）的照片（`ids` 可再限定范围；非淘汰的永远不动）连同 RAW companion 和 XMP 侧车移走，**绝不删除**：`mode: "trash"` 放入系统回收站（目录库里这些照片变为 `missing`，从回收站还原后重新扫描即恢复，评分保留），`mode: "folder"` 移进文件所在目录下的 `_rejected` 子文件夹（目录库里的路径跟着改，仍可见）。必须带 `confirm: true`（界面在确认对话框之后才发），否则 `400`，且不动任何文件。

```ts
interface RejectFilesReport {
  mode: "trash" | "folder";
  photos: number;    // 处理成功的照片数（各含其 RAW）
  files: number;     // 移动的文件数
  skipped: number;   // 文件本来就不在的
  errors: string[];  // 失败项；其余照片不受影响
}
```

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
- `catalog.db`、`cache/thumbs/`、`cache/previews/`、`cache/full/`、`logs/`。
