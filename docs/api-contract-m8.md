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
