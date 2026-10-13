# API 契约 · M5（最佳表情 + 生成式修复）

> 在 M1–M4 契约基础上**增量**扩展。`patch` 操作与 `RgbaImage`、`MaskProvider::patch()` 的权威定义见 `crates/ip-render/src/lib.rs`。
> 算法依据：[03 §5](03-AI与算法设计.md)（Best Take）、§6.5（降噪/超分/人脸修复）、§7.3（背景补全）、§8.2（显存调度）。

## A. 编辑栈：`patch` 操作

```jsonc
{ "type": "patch", "kind": "best_take" | "inpaint" | "denoise" | "face_restore",
  "asset": "bt_4231_881",           // 资产 id（Core 管理的 RGBA PNG，alpha = 融合蒙版）
  "rect": [x, y, w, h],             // 归一化，正向图像、裁剪前坐标
  "feather": 0.08, "amount": 1, "enabled": true,
  "person_id": 12, "source_photo_id": 4229 }   // best_take 专用
```

- 渲染顺序：**patch**（贴到正向原图上）→ crop → warp → global → local → beauty → LUT → sharpen。多个补丁按出现顺序叠加。
- `enabled:false` 的补丁保留在栈中，但不渲染（历史中可开关）。
- 资产存放：`<data_dir>/edits/<photo_id>/<asset>.png`；随照片删除一并清理；导出时按全分辨率重采样。

## B. Worker 新方法

| 方法 | 参数 | 结果 |
|---|---|---|
| `besttake.compose` | `{"base": {photo_id,path,orientation}, "source": {...}, "base_face": [x,y,w,h], "source_face": [x,y,w,h], "out_dir"}` | `{"patch": path /*RGBA PNG*/, "rect": [x,y,w,h], "quality": {"score": 0..1, "aligned": bool, "warnings": ["large_pose_change"\|"camera_moved"\|"occlusion"\|"seam"]}}`；不可合成时 `{"patch": null, "reason": "..."}` |
| `inpaint.run` | `{"photo": {...}, "mask": path /*8-bit PNG，255=移除*/, "model": "lama"\|"sdxl", "out_dir", "allow_download"}` | `{"patch": path, "rect": [x,y,w,h], "model": id}`（补丁只覆盖蒙版外接矩形+边距，alpha 为羽化蒙版） |
| `enhance.run` | `{"photo": {...}, "op": "denoise"\|"face_restore"\|"upscale", "strength": 0..1, "scale"?: 2\|4, "faces"?: [[x,y,w,h]], "out_dir", "allow_download"}` | denoise：`{"patch", "rect":[0,0,1,1]}`；face_restore：`{"patches":[{"patch","rect"}]}`（每张脸一个）；upscale：`{"image": path}`（仅用于导出） |

- 默认模型宽松许可：LaMa（Apache-2.0）、Real-ESRGAN（BSD-3）、SCUNet / NAFNet（Apache/MIT）、GFPGAN v1.4（Apache-2.0）。SDXL Inpainting（OpenRAIL++）为 T3 可选「增强包」；NC 模型（CodeFormer、FLUX Fill）仅可选并标注。
- 大模型由 ModelManager 调度：与 VLM/其他扩散模型互斥，空闲卸载，OOM 降档（03 §8.2）。

## C. REST 新增

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/bursts/{id}/besttake[?base_photo_id=]` | — | `{"base_photo_id","people":[{"track_id","person_id","person_name","base_face_id","candidates":[{"photo_id","face_id","expression_score","composable":bool,"reason":string\|null}],"best_photo_id"}],"base_choice":{"mode":"auto"\|"manual","reason","group_best_photo_id","auto_photo_id","frames":[{"photo_id","replacements","below_best","missing","issues":[...],"cost"}]}}`（底片按 [03 §5](03-AI与算法设计.md)「选底片」自动选择，或用 `base_photo_id` 指定（须为本组照片，否则 422）；`best_photo_id` 只在可合成且表情分高出 0.04 以上时不是底片；候选按表情分降序） |
| POST | `/api/besttake` | `{"base_photo_id", "choices":[{"base_face_id","source_photo_id","source_face_id"}]}` | `202 {"task_id"}`；完成后底片编辑栈中**替换/插入**对应人物的 `best_take` 补丁并广播 `edits.updated`；每个选择的结果经 `besttake.done {photo_id, results:[{base_face_id, ok, warnings, reason}]}` 推送 |
| POST | `/api/bursts/{id}/besttake/auto` | 可选 `{"base_photo_id"}` | `202 {"task_id"}`：底片 = 计划的底片（或指定的，同上 422）+ 每人最佳可合成表情（即计划中 `best_photo_id` 不是底片的人），同上 |
| GET | `/api/photos/{id}/bystanders` | — | `{"faces":[{"face_id","bbox"}]}`（非主体人脸） |
| POST | `/api/photos/{id}/inpaint` | `{"bystanders": true}` 或 `{"face_ids":[...]}` 或 `{"strokes":[{"points":[[x,y],...],"radius":0.02}]}`（归一化，正向裁剪前坐标）；可选 `"model":"lama"\|"sdxl"` | `202 {"task_id"}`；完成后追加 `inpaint` 补丁，广播 `edits.updated` 与 `inpaint.done {photo_id, ok, reason}` |
| POST | `/api/photos/{id}/enhance` | `{"op":"denoise"\|"face_restore","strength":0..1}` | `202 {"task_id"}`；追加/替换同类补丁 |
| GET | `/api/assets/{photo_id}/{asset}` | — | `image/png`（调试/前端预览补丁用） |

- **`POST /api/besttake`（及 `besttake/auto`）的同人校验**：每个选择中，源脸（`source_face_id`）必须与被替换的底片脸（`base_face_id`）是同一个人，否则整个请求 `422`，不启动任务、不改任何编辑栈。错误体 `{"error":{"code","message"}}`，`code` 为：
  - `besttake_cross_burst`：源照片与底片不在同一连拍/分组（`photo.burst_id` 不同或为空）；
  - `besttake_person_mismatch`：两张脸不在同一条人脸轨迹（`face.track_id`）；任一方没有轨迹时，改为要求同一个非空 `person_id`；
  - `besttake_low_similarity`：两张脸都有身份嵌入，但余弦相似度低于 `MIN_IDENTITY_COS = 0.30`（`ip-core/src/generate.rs`，取自聚类的连拍内轨迹延续阈值 `TRACK_COS`；AuraFace 同一人约 0.43-0.66、不同人 < 0.2，0.30 落在两者之间，又容许连拍帧之间姿态与表情的变化）。
  - 校验顺序：先 `besttake_cross_burst`，再 `besttake_person_mismatch`，最后 `besttake_low_similarity`。原有的 422（`unprocessable`：脸不在所给照片、源照片即底片、同一底片脸重复出现）不变。
  - **身份嵌入缺失**（lite 档位不计算身份嵌入，或该脸没有嵌入）：跳过相似度检查，只做连拍与轨迹/人物检查；此时同一轨迹内被聚类误并的人无法被拦截。
  - 自动路径（`besttake/auto`、计划里的 `best_photo_id`）只在同一轨迹内选脸，因此永远通过这些检查；`tests_m5.rs` 有断言。
- 所有生成任务：模型缺失 → `409 models_missing`；worker 不可用 → `503`；进度走 `task.progress`（`kind`: `besttake`/`inpaint`/`enhance`）。生成任务可取消并留有记录，见 F 节。
- `/api/export` 新增 `"upscale"?: 2 | 4`：渲染后经 `enhance.run upscale` 放大再编码。
- 重编码的导出（应用编辑 / `long_edge` / `upscale` / 裁切适配）的元数据：像素为 sRGB（带非 sRGB ICC 或 DCF Adobe RGB 标记的来源在解码时已转换），嵌入 sRGB ICC；EXIF 取自原文件（JPEG/HEIF/RAF 原样；TIFF 类 RAW、TIFF、CR3 由 IFD0/Exif/GPS 重建，不含 MakerNote；都没有时按目录元数据合成），去掉朝向与 IFD1 缩略图并改写像素尺寸；XMP 取 JPEG 内嵌包，否则取 `.xmp` 侧车，去掉 `tiff:Orientation`/尺寸、`crs:*`、darktable 历史、`xmp:Thumbnails`、人脸区域、动态照片/景深/增益图与扩展 XMP 引用；IPTC 只保留 APP13 中的 IPTC 记录（0x0404/0x0425）。`"strip_gps": true` 删除 EXIF GPS IFD 与 XMP 中所有 `GPS*` 属性。原样复制的导出不改动文件（P0-8 起 `strip_gps` / `strip_metadata` 时改为清理，见 F 节）。

## D. 人物与人像相关的修正（M4 联调发现）

- 只出现在一张照片中的**主体**人脸也建立人物（`person.singleton = true`），使其可在人像面板中被单独选中；`GET /api/people` 默认不返回单张人物，`?include_singletons=1` 返回。
- `face_box`、`bbox`、`similarity` 等浮点字段输出时不得出现 f32→f64 噪声（四舍五入到 4 位小数或按 f32 最短表示序列化）。

## E. WebSocket 新增事件

```jsonc
{ "type": "besttake.done", "photo_id": 4231, "results": [{ "base_face_id": 881, "ok": true, "warnings": [], "reason": null }] }
{ "type": "inpaint.done",  "photo_id": 4231, "ok": true, "reason": null }
{ "type": "enhance.done",  "photo_id": 4231, "op": "denoise", "ok": true, "reason": null }
```

- 被取消的生成任务仍先发 `*.done`：`inpaint.done` / `enhance.done` 为 `ok:false, reason:"cancelled"`；`besttake.done` 中每个选择都是 `ok:false, reason:"cancelled"`。随后 `task.progress` 的 `state` 为 `cancelled`（`error` 为空）。

## F. 导出：格式、去元数据、预设与命名（P0-8）

`POST /api/export` 新增字段（均可省略，省略时与之前相同）：

| 字段 | 取值 | 说明 |
|---|---|---|
| `format` | `"original"`（默认）\| `"jpeg"` \| `"png"` \| `"tiff"` \| `"webp"`（别名 `jpg`、`tif`） | 输出格式。`original`：未改动的照片（未编辑、无 `long_edge` / `aspect` / `upscale`）按原文件复制（RAW 仍是 RAW），其余重新编码为 JPEG。指定格式时所有输出都是该格式；源文件已是该格式且未改动时直接复制（或按下文清理） |
| `strip_gps` | bool | 删除 EXIF GPS IFD 与 XMP 中所有 `GPS*` 属性；**对原样复制的照片同样生效**（见下） |
| `strip_metadata` | bool | 不写入任何 EXIF / XMP / IPTC（隐含 `strip_gps`）；ICC 保留；清理后的原图保留朝向 |
| `aspect` | `[w, h]`，各 1..10000 | 居中裁切到该宽高比，再按 `long_edge` 缩小（不放大）；没有 `long_edge` 时只裁切 |

- 扩展名：JPEG `.jpg`、PNG `.png`、TIFF `.tif`、WebP `.webp`；复制或清理的原图保留原扩展名。`quality` 只作用于 JPEG。
- WebP 为**无损**（VP8L）：工作区内的纯 Rust 编码器（`image-webp`）没有有损模式；每边最多 16383 像素，超出时等比缩小。TIFF 为 8 位 RGB、无压缩（单个文件小于 4 GB）。不输出 16 位。

重新编码的输出（像素为 sRGB，均嵌入 sRGB ICC）携带的元数据，清理规则同 C 节（去朝向、IFD1 缩略图、冲印参数等）：

| 格式 | EXIF | XMP | IPTC | ICC |
|---|---|---|---|---|
| JPEG | APP1 | APP1 | APP13（只留 IPTC 记录） | APP2 |
| PNG | `eXIf` | `iTXt`（`XML:com.adobe.xmp`，不压缩） | —（无标准位置，字段一般已在 XMP） | `iCCP` |
| TIFF | IFD0 描述性标签（Make、Model、Artist、Copyright…）+ Exif IFD + GPS IFD，按来源 EXIF 的字节序写出；不含 MakerNote、Interop | 标签 700 | 标签 33723（IPTC-NAA） | 标签 34675 |
| WebP | `EXIF` | `XMP ` | — | `ICCP`（VP8X 扩展头） |

**原图的清理**（未改动的照片 + `strip_gps` / `strip_metadata`）：

| 源格式 | 做法 |
|---|---|
| JPEG | 按段重写，扫描数据逐字节不变。`strip_gps`：EXIF 中 GPS IFD 原位清零（段长度不变，缩略图、朝向、MakerNote 保留）；XMP 去掉 `GPS*` 后用空白填回原长度；扩展 XMP 含 GPS（或无法解析）时整体去掉；EOI 之后附带图像（MPF、增益图）的 EXIF GPS 同样原位清零；头部长度有变化时修正 MPF 索引。`strip_metadata`：只保留 JFIF APP0、ICC APP2、Adobe APP14 与编码段，去掉 APP1 / APP13 / 其他 APPn / COM 与 EOI 之后的数据；朝向不为 1 时写入只含朝向的最小 EXIF |
| PNG | 按块重写，`IDAT` 不变。`strip_gps`：`eXIf` 去 GPS；未压缩的 XMP `iTXt` 去 `GPS*`；压缩的 XMP 与 `Raw profile type …` 文本块去掉。`strip_metadata`：只保留关键块与颜色、透明、密度、动画块（`iCCP`、`sRGB`、`gAMA`、`cHRM`、`cICP`、`pHYs`、`tRNS`、`acTL`…），朝向同上 |
| WebP | 按 RIFF 块重写，`VP8` / `VP8L` / `ALPH` / `ANMF` 不变；`EXIF`、`XMP ` 块按上述规则处理并更新 VP8X 标志 |
| TIFF / RAW / HEIF | 无法就地清理：改为全尺寸重新编码（`format: original` 时为 JPEG，否则为所选格式） |

原图无法解析时按其自身格式重新编码。

**预设**：`GET /api/export/presets` → `{"presets":[{"id","long_edge","aspect","quality","format","strip_gps"}]}`。这是唯一的预设表（`ip_core::export::EXPORT_PRESETS`），导出对话框与 AI 助手的 `export` 工具（`preset` 枚举）共用：

| id | long_edge | aspect | quality | format | strip_gps |
|---|---|---|---|---|---|
| `original` | null | null | 95 | `original` | false |
| `wechat` | 2048 | null | 85 | `jpeg` | true |
| `xiaohongshu` | 1920 | `[3,4]`（1440×1920） | 90 | `jpeg` | true |
| `instagram` | 1350 | `[4,5]`（1080×1350） | 90 | `jpeg` | true |

**命名模板**：`{name}` 原文件名、`{seq}` 4 位序号、`{date}` 拍摄日期 `YYYYMMDD`（没有时用文件时间，再没有为 `nodate`）、`{event}` 照片所属事件的名称（事件层，api-contract-m2 C.6：用户给的标题，否则日期与时间段去掉冒号，如 `2024-05-03 1012-1440`）；不在任何事件里的照片（尚未分析）用导入（会话）的标题（`noevent`）、`{scene}` 分析得到的场景类型，如 `portrait`（`noscene`）、`{camera}` 相机品牌型号（`nocamera`）、`{rating}` 用户星级 0–5（未评为 `0`）。占位符一次展开（值里的 `{…}` 不会再展开），目录取值截断为 64 个字符，未知占位符原样保留；之后按原规则替换非法字符。与本次导出或目标文件夹中已有文件重名时加 `_1`、`_2`…，**从不覆盖**。

## G. 任务记录与取消

生成任务（`besttake` / `inpaint` / `enhance`）、导出（`export`）与分析（`analysis`）开始时写入 `task` 表，结束时更新；只保留最新 200 条（新任务开始时删除更旧的已结束记录）。进行中的任务登记在内存中，可按 id 取消。

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/tasks?limit=50` | `limit` 1..200，默认 50 | `{"tasks":[TaskRecord]}`，按开始时间倒序 |
| POST | `/api/tasks/{id}/cancel` | — | `202 {"task": TaskRecord}`（`status:"cancelling"`）；未知 id → `404 not_found`；任务不在运行 → `409 conflict` |

```jsonc
// TaskRecord
{ "id": "inpaint-12", "kind": "inpaint",
  "status": "running" | "cancelling" | "done" | "failed" | "cancelled" | "interrupted",
  "params": { "photo_id": 4231, "faces": 1, "strokes": 0, "model": "lama" },  // 按 kind：
  //  besttake {photo_id, choices}；enhance {photo_id, op, strength}；export {dest, long_edge, upscale, count}；analysis {session_id, profile, count}
  "done": 1, "total": 2, "error": null,
  "created_at": 1760000000000, "updated_at": 1760000001000, "finished_at": null,   // 毫秒
  "cancellable": true }
```

- **取消语义**：取消令牌会传到正在进行的 worker 调用（JSON-RPC `cancel {req}` 通知，worker 以 `-32800` 结束该请求），任务随即结束为 `cancelled`。生成任务在**提交结果之前**都可取消：补丁只有在全部生成完、任务未被取消时才导入为资产并写入编辑栈，所以取消后不会留下资产或编辑。已进入提交阶段的任务不再中断，正常以 `done` 结束。
- 导出取消后不再开始新的照片，进行中的超分请求被中止；已写出的文件保留，`done` 为已写出的张数。分析取消与 `POST /api/analysis/cancel` 相同（已完成的批次保留），其 `task.progress` 终态为 `cancelled`。
- `cancelling` 只出现在列表中（已请求取消、尚未结束）；数据库中不保存。
- 打开目录库时，仍为 `running` 的记录属于已退出的进程，改记为 `interrupted`（并删除其临时目录）。任务 id 的序号从已有记录的最大序号继续，不会与保留的记录冲突。
- 访客（只读）无权访问 `/api/tasks`（参数里有导出目录等路径）。
