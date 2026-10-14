# API 契约 · M2（智能分析）

> 在 [M1 契约](api-contract-m1.md) 基础上**增量**扩展；M1 中的一切保持不变。
> 涉及三方：`ai-worker`（Python）⇄ `ip-core`（Rust）⇄ `web`（React）。改动须同时更新各方。
> 算法依据：[03 AI 与算法设计](03-AI与算法设计.md) §2（分组）、§3（评分）、§4（人脸/人物/按人脸筛选）。

## A. Worker → Core：`analyze.batch` 结果新增字段

在现有字段（`phash`、`quality`、`sharpness`、`exposure`、`noise`、`faces[]`、`embedding_file`…）基础上新增步骤：

| step | 结果字段 | 说明 |
|---|---|---|
| `identity` | 每张人脸 `identity_index: number`；照片级 `identity_file: string` | `<out_dir>/<photo_id>.faces.npy`：float16，形状 `(n_faces, 512)`，L2 归一化，行序与 `faces[]` 一致（无人脸则不写文件、字段为 null）。模型：AuraFace（Apache-2.0） |
| `aesthetic` | `aesthetic: number`（0–1，越高越好） | 美学分；实现可选（SigLIP 特征上的线性头 / pyiqa 模型），在 `aesthetic_model` 中注明模型 id |
| `quality`（增强） | `quality.tilt_deg: number`（度，±15，> 0 = 画面顺时针转，地平线右低；无直线时为 0）；`quality.tilt_confidence: number`（0–1） | 倾斜检测（doc 03 §3.1.1），lite 档由 `ip-lite` 计算同样两个字段；旧 worker 缺这两个字段时 Core 视为未知（不判倾斜） |
| `iqa` | `iqa: number`（0–1，越高越好） | 无参考技术质量；`iqa_model` 注明模型 id。（M0 中 `iqa` 为 `quality` 的别名，M2 起为独立步骤） |
| `scene` | `scene_type: string`；`scene_scores: {[k]: number}` | SigLIP2 零样本，类别固定：`portrait` `group` `landscape` `food` `architecture` `night` `pet` `other` |
| `faces`（增强） | 每张人脸新增 `gaze: number \| null`（0–1，看镜头程度，虹膜相对眼眶位置 + 头部姿态） | |

`standard` 档默认步骤：`phash, quality, faces, identity, embed, aesthetic, iqa, scene`；`fast` 档：`phash, quality, faces`。某步骤缺模型时，worker 在 `skipped_steps` 中列出，Core 用其余分项继续评分。

## B. Core 内部（不对外，供实现参考）

- **Worker 进程管理**（新 crate `ip-worker-client`）：开发模式下在仓库 `ai-worker/` 执行 `uv run imagepicker-ai serve --host 127.0.0.1 --port 0 --token <随机> --parent-pid <core pid>`，读取 stdout 第一行 `{"event":"ready","port":N}`；可用 `IMAGEPICKER_WORKER_CMD`（完整命令行）与 `IMAGEPICKER_WORKER_DIR` 覆盖。懒启动（首次分析时）、崩溃自动重启（指数退避，最多 3 次）、退出时杀进程树。
- **存储**：`analysis`、`face`、`person`、`burst`、`scene` 表按 doc 05 §1；嵌入从 worker 产物文件读入 BLOB（f16）。
- **分组**（doc 03 §2.2）、**评分与星级**（§3.2）、**人物聚类**（§4.3：连拍组内轨迹关联 + 跨组按 cos ≥ 阈值的并查集/平均链接；用户合并/命名作为约束）、**is_subject**（§4.2）、`face_count`/`subject_face_count` 全部在 Rust 中完成。
- **问题标签**（`issues`）：`closed_eyes`（任一主体人脸 eyes_open < 0.45）、`blurry`、`overexposed`、`underexposed`、`noisy`、`tilted`（置信度 ≥ 0.2 且 2° ≤ |tilt_deg| ≤ 10°，只是标签不扣分；doc 03 §3.1.1）。`analysis` 表存 `tilt_deg`、`tilt_confidence`（v10 迁移）。

## C. REST 新增 / 变更

### C.1 `Photo` 新增字段

```ts
interface Photo {
  // ...M1 字段
  ai_score: number | null;        // 0–1 综合分
  ai_rating: number | null;       // 0–5，0.5 步长
  issues: Issue[];                // [] = 无问题或未分析
  burst_id: number | null;
  rank_in_burst: number | null;   // 0 = 组内最佳
  burst_size: number | null;
  scene_type: SceneType | null;
  face_count: number | null;
  subject_face_count: number | null;
  analyzed: boolean;
  duplicate_group_id: number | null;  // 重复组（见 C.6）：组内最小照片 id；null = 不是任何照片的重复
  duplicate_kind?: "exact" | "near";  // 仅在有重复组时出现
}
type Issue = "closed_eyes" | "blurry" | "overexposed" | "underexposed" | "noisy" | "tilted";
type SceneType = "portrait" | "group" | "landscape" | "food" | "architecture" | "night" | "pet" | "other";
```

### C.2 新接口

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/system/hardware` | — | `{"worker": {"state": WorkerState, "tier": "T0".."T3" \| null, "device": string \| null, "providers": string[], "gpu": {"name","vram_mb"} \| null, "error": string \| null}}` |
| GET | `/api/models` | — | `{"models": [{"id","task":string[],"size_mb","license","noncommercial":bool,"installed":bool,"required_for":("fast"\|"standard")[]}]}` |
| POST | `/api/models/ensure` | `{"ids": string[]}` | `202 {"task_id"}`；进度走 `task.progress`（`kind:"model_download"`，done/total 为字节） |
| POST | `/api/analysis/run` | `{"session_id", "profile": "lite"\|"fast"\|"standard", "photo_ids"?: number[], "force"?: bool, "fallback_lite"?: bool}` | `202 {"task_id"}`；若所需模型未安装 → `409 {"error":{"code":"models_missing","message",...},"models":[id...]}`（UI 征得同意后调用 `/api/models/ensure` 再重试）；会话已有运行中或已暂停的分析 → `409 conflict`。`fallback_lite` 见 C.5.3 |
| POST | `/api/analysis/cancel` | `{"session_id"}` | `204`；已暂停的分析随即收尾（已分析的照片分组、评分）并结束为 `idle` |
| POST | `/api/analysis/pause` | `{"session_id"}` | `202`；运行中的分析在**当前批次**完成后停下，状态变为 `paused`（见 C.5.1）。没有运行中的分析 → `409 conflict` |
| POST | `/api/analysis/continue` | `{"session_id"}` | `202 {"task_id"}`（与暂停前相同）；只处理剩余照片。没有已暂停的分析 → `409 conflict` |
| GET | `/api/analysis/plan?session_id=&profile=` | — | `{"new_photos","incremental_photos","incremental_steps":string[]}`：此刻对该档位发起分析会做什么（见 C.5.4） |
| GET | `/api/analysis/status?session_id=` | — | `{"state":"idle"\|"running"\|"paused"\|"done"\|"failed","profile","done","total","stage":"analyzing"\|"grouping"\|"scoring"\|"clustering"\|null,"error":string\|null,"error_code":"worker_unavailable"\|"models_missing"\|null,"fallback_from":"lite"\|"fast"\|"standard"\|null,"incremental":number,"resumed":bool,"skipped_steps":string[]}` |
| GET | `/api/photos/{id}/analysis` | — | 见 C.3 |
| GET | `/api/groups?session_id=` | — | `{"scenes":[{"id","start_at","end_at","bursts":[{"id","best_photo_id","photo_ids":number[] /*按 rank 排序*/,"size","start_at"}]}]}`（时间序） |
| POST | `/api/groups/split` | `{"burst_id","at_photo_id"}` | `{"burst_ids":[a,b]}`（`at_photo_id` 及之后的照片进入新组；手动组标记 manual） |
| POST | `/api/groups/merge` | `{"burst_ids": number[]}` | `{"burst_id"}` |
| GET | `/api/bursts/{id}/faces` | — | 表情矩阵：`{"photo_ids":number[],"tracks":[{"track_id","person_id":number\|null,"person_name":string\|null,"cells":{"<photo_id>": Face \| null},"best_photo_ids":number[] /*该人表情 Top-3*/}]}` |
| GET | `/api/faces/{id}/crop?s=128\|256` | — | `image/jpeg`，人脸外扩 40% 的方形裁切 |
| POST | `/api/photos/accept-ai` | `{"ids": number[]}` | `{"updated"}`；`user_rating = round(ai_rating)`，仅对已分析照片；写入撤销历史由前端负责（M1 方式） |
| GET | `/api/people?session_id=` | — | `{"people":[{"id","name":string\|null,"cover_face_id","photo_count","hidden":bool}]}`（按 photo_count 降序；`session_id` 省略 = 全库） |
| PATCH | `/api/people/{id}` | `{"name"?: string\|null, "hidden"?: bool}` | `{"person"}` |
| POST | `/api/people/merge` | `{"ids": number[], "into": number}` | `{"person"}` |
| POST | `/api/faces/{id}/person` | `{"person_id": number \| null}` | `{"face"}`（"不是此人"= null，会新建单脸人物） |

### C.3 `GET /api/photos/{id}/analysis`

```jsonc
{
  "photo_id": 12,
  "analyzed": true,
  "profile": "standard",
  "scores": { "sharpness": 0.82, "exposure": 0.74, "noise": 0.12, "iqa": 0.71, "aesthetic": 0.64, "face": 0.58, "composition": 0.71 },
  "ai_score": 0.68, "ai_rating": 4.0,
  "contributions": [ { "key": "face", "label_key": "score.face", "delta": -0.12 } ],  // 对综合分的贡献（可正可负）
  "reasons": [ { "key": "closed_eyes", "params": { "person": "小红" } } ],          // 前端按 key 本地化；tilted 带 { "deg": 3.5 }（|倾角|，0.1°）
  "scene_type": "group",
  "faces": [ Face ]
}
```

```ts
interface Face {
  id: number; photo_id: number; person_id: number | null; person_name: string | null;
  bbox: [number, number, number, number];   // 归一化 x,y,w,h（相对显示方向）
  eyes_open: number | null; smile: number | null; gaze: number | null;
  yaw: number | null; pitch: number | null; roll: number | null;
  sharpness: number | null; expression_score: number | null; is_subject: boolean;
}
```

### C.4 `GET /api/photos` 新增查询参数

| 参数 | 说明 |
|---|---|
| `ai_rating_gte` | AI 星级 ≥ N |
| `issues_none` | `1` = 无任何问题标签 |
| `issues_any` | 逗号分隔，含任一 |
| `burst_best_only` | `1` = 每个连拍组只返回 rank 0（堆栈折叠模式；未分组照片照常返回） |
| `burst_id` | 只返回该组 |
| `event_id` | 只返回该事件的照片（事件 id 见 C.6 的 `GET /api/sessions/{id}/timeline`；事件随分组重算，id 会变，客户端以最新的时间线为准）。不可用于智能集合 |
| `duplicates` | `1` = 只返回有重复的照片，`0` = 只返回没有重复的（C.6）。可用于智能集合 |
| `scene_type` | 场景类型 |
| `persons` / `person_mode` / `exclude_persons` / `person_state` / `include_background` / `faces_min` / `faces_max` | 见 [05 §3.2](05-数据与接口设计.md)（`person_state` M2 支持 `eyes_open,smiling,looking,subject`） |
| `sort` | 新增 `ai`（ai_score 降序，未分析在后） |
| `face_ids` / `face_similarity` | 按脸找照片的阈值与语义见 [api-contract-m1 「筛选补全」](api-contract-m1.md)（另有 `lens` / `taken_from` / `taken_to` / `tags` / `tag_mode` / `sharp`） |

### C.5 暂停 / 继续、续跑、lite 兜底、按步骤增量

#### C.5.1 暂停与继续

- `pause` 只在两个批次之间生效（每批最多 32 张）：已发出的批次完成并入库后，分析停止，`state` 变为 `paused`，**不**做分组 / 评分（继续或取消时才做）。`done` / `total` 保持，`stage` 为 null。
- `continue` 按存储的运行参数重新选出剩余照片（本次运行已分析的不再发送），状态回到 `running`；`task_id` 不变，任务历史里是同一行。
- 暂停的运行占用该会话：再次 `run` 返回 `409`。`cancel` 对已暂停的分析：先给已分析的照片分组、评分，再结束为 `idle`，任务记为 `cancelled`。删除会话同样结束其暂停的任务。
- `GET /api/tasks` 中暂停的分析任务为 `status:"paused"`（`cancellable:false`；从 `analysis/cancel` 取消）。`task.progress` 的 `state` 同样可为 `"paused"`。

#### C.5.2 重启后续跑

- 运行参数（`session_id`、档位、`photo_ids`、`force`、`fallback_lite`、开始时间、续跑次数）存在任务行的 `params` 中，无需新表。
- 服务启动时（构建路由时，`Core::resume_analyses`）：上次退出时仍为 `running` 的分析**自动续跑**——只发送剩余照片，已入库的批次不重算（以本次运行开始时间之后写入的 `analysis.analyzed_at` 判断，所以 `force` 运行也不会重做已完成的照片）；任务行从 `interrupted` 改回 `running`，`status.resumed=true`。上次为 `paused` 的分析恢复为 `paused`（不自动继续）。
- 同一运行最多自动续跑 3 次，之后保持 `interrupted`，用户可手动重新发起（只会处理未分析的照片）。续跑时若 AI 不可用（组件已卸载、模型缺失）：运行带 `fallback_lite` 则改用 lite，否则记为 `failed` 并带 `error_code`，UI 可据此提供 lite。

#### C.5.3 lite 兜底

- `lite` 档（`ip-lite`，纯 Rust，无需 Python 与模型）在桌面端与 Android 一样可用：`profile:"lite"` 不触发 `models_missing`。
- `fallback_lite:true` 的 `run`：AI 档位的模型缺失（`models_missing`）、AI 组件未安装或 worker 无法启动 / 超时（`worker_unavailable`）时，改用 `lite` 完成**同一批照片**，而不是返回 409 / 503。`status.profile` 为 `lite`，`status.fallback_from` 为原档位。分析中途 worker 不可用（运行时被卸载、崩溃重启仍失败）时，剩余照片同样改用 `lite`，已入库的结果保留。
- 不带 `fallback_lite` 的运行以 `failed` 结束时，`status.error_code` 为 `worker_unavailable` 或 `models_missing`；UI 据此提供「改用快速分析」，即对同一会话发起 `profile:"lite"`（只处理尚无分析的照片）。lite 结果之后仍可被 `fast` / `standard` 升级（见 M8 的 `analysis_version` 规则）。

#### C.5.4 按步骤增量

- 每次分析把每步实际使用的模型写入 `analysis.model_versions`（`{step: model id}`）。standard 档的可补算步骤为 `embed`、`aesthetic`、`iqa`、`scene`（各自只写自己的列；`faces` / `identity` 与人脸行绑定，不在此列）。
- 安装新模型后，已分析（同档位）照片中**缺某步骤且该步骤已有已安装模型**的，下次分析只补算这些步骤：请求带 `steps:[…]`，结果经 `ingest_partial` 只更新这些列并合并 `model_versions`（嵌入、人脸、人物不变），随后照常分组 / 评分。`status.incremental` 为本次这样补算的照片数；`plan` 的 `incremental_*` 预告同样的内容。
- worker 报告为 `skipped_steps` 的步骤，以「已安装模型集合的指纹」记入该照片 `model_versions._skipped`；同一指纹下不再重试，安装 / 删除模型改变指纹后才会再试。

### C.6 事件层与重复检测（09 P1-1 / P1-3）

**事件**把一个会话里的场景再切成「行程 / 外出 / 一天」：`事件 → 场景 → 连拍组 → 照片`。相邻两个场景（按拍摄时间）之间满足任一条件就切开：

- 停顿 **> 2 小时**（`EVENT_GAP_MS`，下一场景开始时间 − 此前所有场景的最晚结束时间）；
- 此前最后一个有 GPS 的位置与下一场景第一个有 GPS 的位置相距 **> 5 公里**（`EVENT_DISTANCE_M`，大圆距离；`0,0` 与越界坐标视为没有 GPS；没有 GPS 的照片不会按距离切开，没有任何 GPS 的会话只按时间切）。

拍摄时间是绝对时间（UTC 毫秒），跨时区拍摄不会被当成停顿；本地日期与时刻由 `taken_at_offset_min` 得出。事件整体包含完整的场景，一个场景不会被切开。事件由 `regroup_session` 在分组之后重算，手动拆分 / 合并连拍组之后也会重算；事件 id 每次重算都会重新分配。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/sessions/{id}/timeline` | `{"events":[{"id","name","title":string\|null,"place":string\|null,"start_at","end_at","offset_min":number\|null,"gps_lat":number\|null,"gps_lon":number\|null,"photo_count","scenes":[{"id","start_at","end_at","event_id","bursts":[{"id","best_photo_id","photo_ids","size","start_at"}]}]}]}`，事件按时间序。`name` = `title`（用户命名，目前没有界面）或本地日期与时间段 `2024-05-03 10:12-14:40`（跨零点 `2024-05-03 22:10-05-04 01:20`）；`place` 预留给地名（目前不自动生成），`gps_*` 是事件内有 GPS 照片的平均位置。会话不存在 → `404`；有场景而没有事件时（旧目录库）按需补建 |
| GET | `/api/photos/{id}/duplicates` | `{"group_id":number\|null,"photo_ids":number[],"exact_ids":number[]}`：该照片所在重复组的全部成员（整个目录库，跨会话）；`exact_ids` 是有完全相同副本的成员 |

`GET /api/groups` 不变（场景不带 `event_id`）。导出命名的 `{event}` 取照片所属事件的名称（`title`，或日期与时间段去掉冒号：`2024-05-03 1012-1440`）；不在任何事件里的照片（尚未分析）仍用导入（会话）标题。

**重复**在整个目录库里检测（不限于一个会话）：

- **完全重复（`exact`）**：`photo.content_key`（文件头 64 KiB + 尾 64 KiB + 长度的 BLAKE3，与缩略图缓存、编辑存储用的是同一个键）与 `file_size` 都相同的不同文件，可在不同文件夹、不同会话。导入读完元数据后即检测，不需要分析。
- **近似重复（`near`）**：同一张画面去掉元数据后重新保存（微信 / 网页导出、截图再存）：pHash 汉明距离 ≤ **4**（`DUP_PHASH_MAX_BITS`，共 64 位；无关照片约 32 位）、宽高比相同（相对误差 ≤ 1%），且**两张里至少一张没有拍摄时间（无 EXIF）**。最后一条把连拍的相邻帧（距离常为 0–4 位）排除在外：只有「没有元数据的副本」会和有元数据的照片配成对；因此一张无 EXIF 副本会连带把与它距离 ≤ 2 位的照片并入同一组。需要分析写入 pHash 之后才能发现；分析结束后重新检测。
- 两种关系传递合并成组，`duplicate_group_id` = 组内最小照片 id；不在组里的照片为 `null`。已读不到元数据（`meta_done=0`）、文件已丢失（`missing`）和 RAW 附属文件不参与。
- 内置智能集合 `duplicates`（`duplicates=1`）；`GET /api/photos?duplicates=1|0` 筛选。检测结果变化时对受影响的会话发 `groups.updated`，客户端重新拉取列表。

### C.7 资料库时间线（F1.9，只含时间线，地图未做）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/timeline` | 全部会话（或 `session_id` 指定的一个）的照片按**本地拍摄日期**分桶 |

查询参数：

| 参数 | 说明 |
|---|---|
| `granularity` | `year` \| `month` \| `day`，缺省 `month`；其它值 → `400` |
| `from`、`to` | 本地日期窗口 `YYYY-MM-DD`（含两端，可只给一端）；非日期、`from` 晚于 `to` → `400`。带窗口时没有拍摄时间的照片不在其中 |
| `session_id` | 只统计这个会话；会话不存在 → `404`；缺省 = 整个图库 |
| 其余 | 与 `GET /api/photos` 相同的筛选（`rating_gte`、`flag`、`color_label`、`ai_rating_gte`、`issues_*`、`scene_type`、`persons`…、`device`、`lens`、`taken_from` / `taken_to`、`tags`、`sharp`、`face_ids`、`has_edits`、`duplicates` 等，校验同 C.4，错误同样 `400`），与分桶取交集。`sort`、`cursor`、`limit` 忽略；`event_id` 仅对 `session_id` 指定的会话有意义 |

响应：

```json
{"granularity":"month","from":null,"to":null,"total":4,
 "buckets":[{"key":"2023-11","from":"2023-11-01","to":"2023-11-30","count":3,"photo_ids":[7,3,5]}],
 "undated":{"count":0,"photo_ids":[]},
 "events":[{"id","session_id","name","title":string|null,"start_at","end_at","offset_min":number|null,"photo_count"}]}
```

- `buckets` 按时间升序，只含有照片的桶；`key` 为 `2023` / `2023-11` / `2023-11-14`；`from` / `to` 是该桶第一天 / 最后一天，可直接用作 `GET /api/photos` 的 `taken_from` / `taken_to`。`photo_ids` 至多 4 张代表照片，顺序为：未被拒绝 → 用户星级高 → AI 分高 → id 小。
- **本地日期** = `taken_at` + `taken_at_offset_min`（分钟，未知按 0；固定偏移，不查夏令时表——照片记录的偏移本身已含夏令时）。没有 `taken_at` 的照片只出现在 `undated`（带 `from` / `to` 窗口时为空）。
- 同一张照片属于多个会话时只计一次；RAW 伴随文件（`pair_id`）不计。缺失文件（`missing`）与网格一致，照常计入。
- `events`：仅当 `granularity` 为 `month` 或 `day` 且 `from`、`to` 都给出时返回，是本地日期与窗口相交的事件（C.6，名称规则同 `GET /api/sessions/{id}/timeline`，至多 500 个，按开始时间排序）；`session_id` 告诉客户端该事件属于哪个会话（`event_id` 筛选要在那个会话里使用）。
- 访客可读；与 `GET /api/photos` 一样，访客带人物相关筛选 → `403`。
- **与网格筛选的差别**：`taken_from` / `taken_to` 按 `taken_at` 本身（有偏移的照片是 UTC）比较，而桶按本地日期划分，所以带非零偏移的照片在边界附近点击桶后，网格张数可能与桶的张数差几张。
- 性能：单次扫描 `photo`（不走会话排序索引），按 `taken_at` 的窗口范围可用 `idx_photo_taken`；分桶在 Rust 里完成。10 万张的实测见 PR 描述与 docs/05 §1.3。未新增索引 / 迁移。

### C.8 自然语言搜索（F2.9，09 P1-4）

`GET /api/search`：按描述找照片（中文或英文），在标准分析写下的 SigLIP2 图像嵌入（`analysis.embedding`）上做精确余弦检索，查询向量由 worker 的 SigLIP2 文本塔（`siglip2-base-text`）给出。

| 参数 | 说明 |
|---|---|
| `q` | 必填，查询文本，去掉首尾空白并折叠空白后 1–256 个字符（按字符数，不是字节） |
| `session_id` | 省略 = 搜整个目录库（不含已标记 `missing` 的照片）；给出 = 只搜该会话，并可同时带 `GET /api/photos` 的所有筛选参数（评分、旗标、人物、设备、标签……，语义相同）。`sort` / `cursor` / `ids` 无效（结果按匹配度排序）；没有 `session_id` 时带筛选参数 → `400`（筛选以会话为范围） |
| `limit` | 最多返回多少条，1–5000，默认 200 |
| `min_score` | 分数下限 0..1，默认 `0.05`（余弦约 0.12）；`0` = 不设阈值，只按名次返回 |

```jsonc
// 200
{ "query": "海边 日落", "model": "siglip2-base-text", "min_score": 0.05,
  "hits": [ { "photo_id": 812, "score": 0.9931, "similarity": 0.2514 }, … ],   // 按余弦从高到低（相同按照片 id）；强匹配的 score 都会饱和成 1.0，名次以余弦为准
  "total_matched": 57,   // 分数 ≥ min_score 的照片数（limit 截断之前）
  "searched": 980,       // 参与比较的照片数（有嵌入的）
  "unindexed": 20 }      // 范围内没有嵌入的照片数：它们永远搜不到（先做标准分析）
```

- **分数**：`score = sigmoid(112.67 × cos + b)`，SigLIP 的 sigmoid 损失使它与其他照片无关；`similarity` 是查询与照片的余弦。`112.67` 是 `siglip2-base-patch16-224` 学到的 `exp(logit_scale)`（与零样本场景分类同一常数）；偏置 `b = -16.5` 是 SigLIP 系列的公开取值，导出的 ONNX 里没有它，**未在本机读回校准**，所以它只决定概率刻度与默认阈值，不影响排名；用 `bench/eval-search.py` 复核。
- 嵌入只在标准档位的 `embed` 步骤写入；未分析、`fast` / `lite` 分析过的照片计入 `unindexed`。
- 错误：`400`（`q` 为空 / 过长、`min_score` 或 `limit` 越界、参数格式错误）、`404`（会话不存在）、`409 models_missing`（文本模型未安装，`models` 列出 `siglip2-base-text`，与其他接口一致，界面据此提供「安装搜索模型」）、`503 worker_unavailable`、`504 worker_timeout`。访客可以搜索（只返回他们本来就能看到的照片）。
- `GET /api/photos` 新增 `ids`（逗号分隔，最多 5000 个）：只返回这些照片（`ids=` 空值 = 没有照片）。界面先调 `/api/search` 得到名次，再用 `ids` 取照片；不可用于智能集合。
- Worker：`text.embed`（`{texts: string[1..16], allow_download?}` → `{model, dim, logit_scale, vectors: float[][], cached: bool[]}`，向量已 L2 归一化；缺模型 → `-32010 model_unavailable`，`data.detail.models` 为 `["siglip2-base-text"]`）。worker 内有 256 条的 LRU（键为折叠空白并转小写的查询），重复查询不再跑文本塔。远程 AI 主机的白名单同样包含 `text.embed`。

## D. WebSocket 新增事件

```jsonc
{ "type": "analysis.progress", "session_id": 1, "state": "running", "stage": "analyzing", "done": 120, "total": 1204 }
// state: "running" | "paused" | "done" | "failed" | "idle"（idle = 已取消）。非 running 的状态事件之后，客户端重新拉取 /api/analysis/status 以取得 error / error_code / fallback_from
{ "type": "analysis.updated",  "session_id": 1, "ids": [12, 13] }   // 这些照片的 AI 字段变化，客户端重新拉取
{ "type": "groups.updated",    "session_id": 1 }
{ "type": "people.updated",    "session_id": 1 }
{ "type": "worker.status",     "state": "stopped" | "starting" | "ready" | "busy" | "crashed" | "unavailable", "tier": "T3", "error": null }
```

合并规则同 M1（100ms）：`analysis.updated` 按 session 合并 ids，其余保留最新。
