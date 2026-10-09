# API 契约 · M8（Android）

> 设计依据：[08 Android 版本设计](08-Android版本设计.md)。Android 版复用同一 SPA 与 Rust 核心（Tauri 2 移动端），端侧只做轻量分析，重计算交给家中主机（「远程 AI」）。

## A. 相册访问（Kotlin 插件 `imagepicker-media`）

Android 11+ 允许持有 `READ_MEDIA_IMAGES`（13+）/ `READ_EXTERNAL_STORAGE`（≤12）的应用通过**文件路径**读取共享存储中的媒体文件，因此 Rust 核心沿用桌面的路径式导入，插件只负责权限与相册枚举：

| 命令（`invoke('plugin:imagepicker-media|…')`） | 参数 | 结果 |
|---|---|---|
| `request_permission` | — | `{"granted": bool, "partial": bool}`（Android 14「部分照片访问」时 `partial=true`） |
| `list_albums` | — | `{"albums": [{"id","name","path" /*如 /storage/emulated/0/DCIM/Camera*/,"count","cover_path","latest_ms"}]}`（按 MediaStore bucket 聚合） |
| `share` | `{"paths": string[]}` | 系统分享面板 |
| `trash` | `{"paths": string[]}` | `MediaStore.createTrashRequest`，系统确认后移入回收站；`{"trashed": n}` |
| `start_foreground` / `stop_foreground` | `{"title","text"}` | 分析期间的前台服务（`dataSync`）与进度通知 |
| `keep_awake` | `{"on": bool}` | 挑片时保持屏幕常亮 |

- 导出写入 `Pictures/imagePicker/`（MediaStore insert，Android 10+ 无需写权限）：Core 导出到应用私有目录后由插件 `publish_exports {"paths"}` 发布到 MediaStore。
- 路径白名单（M6）在 Android 上的默认根：`/storage/emulated/0/DCIM`、`/storage/emulated/0/Pictures`、`/storage/emulated/0/Download` 与 `list_albums` 返回的路径。

## B. 端侧分析（移动档 M）

- 无 Python worker。Core 内置 **`LiteAnalyzer`**（纯 Rust）：pHash、清晰度/曝光/噪点（与 worker `quality` 步骤同公式，结果可互换）、时间线与连拍/场景分组、星级映射；人脸/嵌入步骤缺省 → 评分按可用分项重新归一（M2 已支持）。
- 可选 `ip-infer`（Rust `ort` + ONNX Runtime Android 库）运行 YuNet 人脸检测与 SigLIP2 int8 嵌入；若集成成本过高，M8 先交付 LiteAnalyzer + 远程 AI，`ip-infer` 列为 M8.1。
- 分析档位在手机上显示为「快速（本机）」与「标准（远程 AI）」。

### B.1 `lite` 档位（API 追加，已实现）

- `POST /api/analysis/run` 的 `profile` 新增取值 `"lite"`（`fast` / `standard` 不变）；`GET /api/analysis/status` 的 `profile` 回显 `"lite"`。`models_missing`（409）永不会因 `lite` 触发，`allow_download` 对其无意义。
- `lite` 在**所有平台**可用（桌面无 Python 运行时的 T0 机器也可用）：纯 Rust `LiteAnalyzer`（`ip-lite` + `ip-core::lite::LiteWorker`）在 analysis 尺寸（长边 1024）上计算 `phash`、`sharpness`/`exposure`/`noise`（及 `mean_luminance`、`clipped_highlights`、`crushed_shadows`、倾斜 `tilt_deg`/`tilt_confidence`），**不含**人脸、嵌入、`iqa`、`aesthetic`、`scene_type`；之后的分组（时间线 + pHash 相似度）、评分（按可用分项重新归一）、星级映射与 `fast`/`standard` 完全相同。
  - 与 worker `phash.py`/`quality.py` 同公式：pHash 位级一致（跨语言夹具测试 `crates/ip-lite/tests/parity.rs`），质量指标在 float32 误差内一致（相对 ≤ 0.4%）。人脸清晰度分支（`sharpness_face`）不适用。
  - 结果存库时 `analysis.profile = "lite"`、`photo.analysis_version = 1`（与 `fast` 同级）。`fast`/`standard` 运行会重新分析仅有 `lite` 结果的照片；`lite` 运行跳过任何已分析的照片（`force=true` 除外）。`GET /api/photos/{id}/analysis` 的 `profile` 为 `"lite"`，`scores.iqa/aesthetic/face` 为 `null`，`faces` 为空。
- `GET /api/system/hardware` 追加字段 `lite: true`（恒为 `true`）。无法运行 Python worker 时（Android，或 `IMAGEPICKER_NO_WORKER=1`）：`{"worker": {"state": "unavailable", "error": "…", …}, "lite": true}`；`?probe=1` 在此情形不再报错而是照常返回该状态。此时 `fast`/`standard` 档位的运行返回与 worker 不可用相同的错误，`/api/runtime/install` 返回 409（`reason` 说明平台不支持）。
- Android 默认值：路径白名单默认根为 `/storage/emulated/0/{DCIM,Pictures,Download}`（外壳可经 `extra_roots` 追加 `list_albums` 的路径）；数据目录必须由外壳传入（`ServerConfig.data_dir` / `CoreConfig.data_dir`）；渲染后端 `auto` → 存在 Vulkan 硬件适配器用 wgpu，否则 CPU；缩略图/分析线程 ≤ 4。详见 [android-build.md](android-build.md)。

### B.2 `ip-infer`（可选，`infer` 特性；M8.1 第一片）

- 新 crate `crates/ip-infer`（Rust `ort` 2.0.0-rc.13 + ONNX Runtime，仅 CPU 执行提供器）。`FaceDetector::load(model_path)` / `detect(rgb, w, h) -> Vec<Detection { bbox: [x,y,w,h] 归一化, score, landmarks: [[x,y];5] 归一化（YuNet 顺序：右眼、左眼、鼻尖、右嘴角、左嘴角）}>`。
- 移植自 OpenCV `FaceDetectorYN` 与 worker `steps/faces.py`：BGR 0..255 输入、score ≥ 0.7、NMS 0.3、top-k 5000、stride 8/16/32 解码。`face_detection_yunet_2023mar.onnx` 的输入是固定 640x640：按比例缩放到该尺寸并在右/下补零，再把结果映射回原图（动态输入的模型则按 OpenCV 补到 32 的倍数）。与 OpenCV 在原尺寸上的结果存在微小差异（实测 score 差 ≤ 0.005，bbox 差 ≤ 0.01 归一化单位）。
- `ip-core` 特性 `infer`（默认关闭，CI 与 Android 交叉编译不受影响）：找到 YuNet 模型时（`IMAGEPICKER_YUNET_MODEL`，其次 `<models_dir>/yunet/face_detection_yunet_2023mar.onnx`，用 `scripts/fetch-yunet.sh` 下载）`LiteWorker` 在 `quality` 之后增加 `faces` 步骤：每张脸输出 `bbox`、`det_score`、`sharpness`（与 `quality.py` 的 `face_sharpness` 同法：128x128 灰度裁剪的中心 60%，人脸常数），`eyes_open`/`smile`/姿态/嵌入为 `null`；`quality.sharpness_face` 为各脸均值，`sharpness` 随之取该值（与 worker 一致）。评分按可用分项重新归一。
- `models.list` 的 `lite` 档位仅在模型可用时包含 `faces` 与 `yunet`；`/api/system/hardware` 的 `lite: true` 语义不变。无模型或未启用特性时行为与 B.1 完全相同。
- 模型仅在本机文件系统中查找；不做下载（远程 AI 的模型安装仍只发生在主机，见 C）。

## C. 远程 AI（手机 → 家中主机）

主机（桌面版或 `imagepicker serve --lan`，已开启局域网模式）向配对设备开放其 AI worker：

| 方法 | 路径（主机） | 请求 | 响应 |
|---|---|---|---|
| POST | `/api/remote/pair/start` | （owner）— | `{"code": "6 位数字", "expires_at", "qr_svg", "url"}`（二维码内容：`imagepicker://pair?url=<主机地址>&code=<code>`） |
| POST | `/api/remote/pair/complete` | `{"code", "device_name"}`（无需登录，仅凭有效配对码；限速同登录） | `{"device_id", "token" /*256-bit，仅返回一次*/}` |
| GET | `/api/remote/devices` | （owner） | `{"devices":[{"device_id","name","created_at","last_seen"}]}` |
| DELETE | `/api/remote/devices/{id}` | （owner） | `204`（吊销） |
| POST | `/api/remote/rpc` | `Authorization: Bearer <device token>`；`multipart/form-data`：`method`（worker RPC 名，白名单：`system.info`、`models.list`、`analyze.batch`、`mask.generate`、`beauty.prepare`、`besttake.compose`、`inpaint.run`、`enhance.run`、`faces.embed`）、`params`（JSON，其中所有图片路径改写为 `file:<字段名>`）、上传文件字段 | 与 worker 结果相同的 JSON，其中输出文件路径改写为 `remote:<file_id>`；超时与错误码同 worker |
| GET | `/api/remote/files/{file_id}` | Bearer | 产物文件（`.npy`/`.png`…），10 分钟后过期 |

- 设备角色 `device`：只能访问 `/api/remote/rpc`、`/api/remote/files/*`、`/api/health`；不可访问照片库、设置等。
- 手机侧 Core 新增 `RemoteWorker`（实现 `AiWorker`）：上传前把原图缩放到 analysis 所需尺寸（默认长边 1536；`inpaint`/`besttake` 用 2048），下载产物到本地缓存后把路径还原，后续流程与本地 worker 完全一致。
- 手机设置「远程 AI」：扫码或手动输入主机地址 + 配对码；状态（在线/离线/主机档位）；断线时分析任务排队重试，界面提示。
- 隐私：远程模式仅在用户配对后启用；上传的是缩放后的副本；主机处理完即删除临时文件。

## D. 移动端 UI（同一 SPA，`< 768px` 断点 + 触控）

- 底部导航：图库 / 挑片 / 人物 / 设置。
- 挑片：全屏单张，左右滑切换、上滑精选、下滑淘汰、双击缩放、长按多选；「快速挑片」模式（每组只看 AI 前 3，类卡片滑动）；触觉反馈（`navigator.vibrate`）。
- 编辑：底部抽屉，横向滚动选择参数 + 单滑块；人像美化同样按人物。
- 导入：相册列表（`list_albums`）代替文件夹浏览器。
- 所有桌面快捷键保留（外接键盘）。

## C 附录 · 远程 AI：手机侧端点与实现细节（追加）

> 仅追加，不改动上文 §C。以下为已实现的行为；与上文的差异/补充单独标出。

### 手机侧端点（手机核心自身的 API，仅 owner）

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| POST | `/api/remote/connect` | `{"host_url","code","device_name"?}`（`host_url` 形如 `http://192.168.1.5:7878`，缺协议时补 `http://`；`device_name` 缺省取主机名） | 同 `GET /api/remote/status`。流程：调用主机 `pair/complete` → 令牌写入 `<data_dir>/remote.json`（Unix 0600）→ 设置 `remote_ai = {enabled:true, host_url}` → 立即探测。错误：`401 invalid_pairing_code`、`429 too_many_requests`（含 `retry_after`）、`502 host_unreachable`、`502 pair_failed`、`400`（地址非法） |
| GET | `/api/remote/status` | — | `{"enabled","host_url","connected","host_tier","last_error"}`；`connected` = 带令牌的 `GET /api/remote/ping` 成功（结果缓存 5 秒）；`host_tier` 来自主机 worker 的 `system.info`（首次连通时询问一次）；未配对/令牌属于另一主机时 `connected=false` 且 `last_error` 说明原因 |
| DELETE | `/api/remote/connect` | — | `204`：删除 `remote.json`，设置 `remote_ai = {enabled:false, host_url:""}`，回到本地 AI |

- 设置新增字段 `remote_ai: {"enabled": bool, "host_url": string}`（`PATCH /api/settings` 可改；`host_url` 必须是规范化的 `scheme://host[:port]`，否则 `422`）。**令牌永不出现在任何设置/状态响应中**。令牌与配对时的 `host_url` 绑定：把设置里的 `host_url` 改成别处不会把令牌发给新地址（状态显示「需重新配对」）。
- 仅当 `enabled` 且存在属于该 `host_url` 的令牌时，核心才把 worker 调用路由到 `RemoteWorker`；其余情况使用本地 worker。`guest` 角色对 `/api/remote/*` 一律 `403`。

### 主机侧补充与差异

- **新增** `GET /api/remote/ping`（设备令牌）：`{"ok":true,"tier":string|null,"worker_state":string}`，不会启动主机 worker，供手机做连通性探测。设备端点集合为 `/api/remote/rpc`、`/api/remote/files/*`、`/api/remote/ping`（外加匿名的 `/api/health`）。
- `pair/start` 要求主机已开启局域网模式（否则 `403 lan_disabled`）；响应在契约字段之外附加 `urls`（候选主机地址数组）；`expires_at` 为毫秒时间戳；`url` 为 `imagepicker://pair?url=<URL 编码的主机地址>&code=<code>`。配对码 6 位、5 分钟、一次性；任意来源累计输错 5 次即作废；`pair/complete` 另按来源 IP 限速（与登录同一参数，超限 `429` + `Retry-After`）。设备令牌 256 位，主机仅存 BLAKE3 摘要（`security.json` 的 `devices`），`last_seen` 至多每分钟落盘一次。最多 32 台设备。
- 认证结果：设备端点上无令牌/未知或已吊销令牌 → `401`；owner/guest 会话或桌面令牌 → `403`。设备令牌访问其余任何 `/api/*`（照片库、设置、设备列表…）→ `403`。
- `rpc`：multipart 字段必须先给 `method`、`params`，再给文件；文件字段名 `[A-Za-z0-9_-]{1,32}`；`params` 中图片路径字段（`items[].path`、`photo.path`、`base.path`/`source.path`、`mask`、`path`）**必须**是 `file:<字段名>`，否则 `400`（主机绝不读取其他路径）；`file:` 只允许出现在这些字段。主机把上传落到请求临时目录（保留净化后的文件名，目录结构 `in/<字段>/<文件名>`），强制 `out_dir` 为该请求的 `out/`，并强制 `allow_download=false`（手机无法让主机下载模型，缺模型走 `409`）。输入目录在处理完立即删除；输出文件保留至过期。
- 产物：结果 JSON 中位于 `out/` 下的文件路径被改写为 `remote:<32 位十六进制 id>`；`GET /api/remote/files/{id}` 仅创建者设备可取（否则 `404`），响应头 `X-File-Name` 给出相对文件名（净化为 `[A-Za-z0-9._-]` 路径段，如 `1.emb.npy`），10 分钟后过期，后台任务每 30 秒清扫；主机重启时清空残留。
- 限制（均可通过 `ServerOptions.remote` 调整）：请求体 ≤ 64 MB（`413 payload_too_large`）、单文件 ≤ 32 MB、每请求文件 ≤ 64、每请求产物 ≤ 256 MB（`413 result_too_large`）、产物库总量 ≤ 1 GiB（淘汰最旧）、每设备并发 ≤ 2（`429 device_busy`）、上传读取 5 分钟、`analyze.batch` 墙钟上限 30 分钟、其他方法 15 分钟（worker 自带超时通常先触发）。手机断开连接时主机取消正在进行的 `analyze.batch`。
- 错误映射（主机 → 手机 `WorkerError`）：`409 models_missing`（附 `models`）→ `Rpc{-32010, model_unavailable}`（`CoreError::ModelsMissing`）；`503 worker_unavailable` → `Unavailable`；`504 worker_timeout`（附 `method`、`secs`）→ `CallTimeout`；`502 worker_error`（附 `worker:{code,message,kind,detail}`）→ 原样 `Rpc`；`401` → `Unpaired`；`429` → `Unavailable`；`403 method_not_allowed` 对应白名单外的方法。

### 手机侧上传规则

- 上传前用 `ip-imaging` 解码并按 EXIF 方向转正，**上传直立像素并把 `orientation` 改写为 1**；长边：`analyze.batch`/`mask.generate` 1536，`beauty.prepare`/`besttake.compose`/`inpaint.run`/`enhance.run`/`faces.embed` 2048；不放大。`inpaint.run` 的 `mask` 原样上传（PNG）。归一化坐标（人脸框、`rect`）与缩放无关。
- `analyze.batch` 单请求上传超过 48 MB 时自动拆成多个请求并合并结果；无法解码/不存在的照片返回逐项错误（`kind: decode_failed`），其余照片照常分析；每个子请求完成后向 `progress` 通道发送 `{"kind":"analyze","done","total"}`。响应里的 `width/height` 是上传副本的尺寸（核心不使用）。
- 白名单外的 RPC（`llm.plan`、`vlm.*`、`models.delete`、`models.ensure`）在手机侧直接返回 `Unavailable`；模型只能在主机上安装。
- 断线：连接失败按 0.5/1/2 秒退避重试 3 次（请求体尚未送达，可安全重试），仍失败则 `Unavailable`；`kill()` 对远程 worker 无效（主机的 worker 不归手机管）。

### C 附录修订（与移动端 UI 对齐，以本节为准）

- `POST /api/remote/connect` 请求体用 `{url, code, device_name?}`（`host_url` 作为 `url` 的别名同样接受）。
- `GET /api/remote/status`（及 connect 的响应）：`{paired, connected, url, host_name, host_tier:"T0".."T3"|null, last_error, last_seen /*unix 秒，0=从未*/}`；为兼容另保留 `enabled`、`host_url`。`paired` = 已存有属于该地址的令牌；`last_error` 连不上时为 `host_unreachable`，否则为说明文字。
- 错误码（`error.code`）：`invalid_code`（配对码错误/已用，替代 `invalid_pairing_code`）、`pair_expired`（配对码超过 5 分钟）、`host_unreachable`、`too_many_requests`（429）。主机 `pair/complete` 同样返回 `invalid_code` / `pair_expired`。
- `pair/start` 的 `expires_at` 改为 unix 秒（替代上文附录的毫秒）。`GET /api/remote/ping` 另返回 `host_name`。
