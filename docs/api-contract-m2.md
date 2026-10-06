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
| `iqa` | `iqa: number`（0–1，越高越好） | 无参考技术质量；`iqa_model` 注明模型 id。（M0 中 `iqa` 为 `quality` 的别名，M2 起为独立步骤） |
| `scene` | `scene_type: string`；`scene_scores: {[k]: number}` | SigLIP2 零样本，类别固定：`portrait` `group` `landscape` `food` `architecture` `night` `pet` `other` |
| `faces`（增强） | 每张人脸新增 `gaze: number \| null`（0–1，看镜头程度，虹膜相对眼眶位置 + 头部姿态） | |

`standard` 档默认步骤：`phash, quality, faces, identity, embed, aesthetic, iqa, scene`；`fast` 档：`phash, quality, faces`。某步骤缺模型时，worker 在 `skipped_steps` 中列出，Core 用其余分项继续评分。

## B. Core 内部（不对外，供实现参考）

- **Worker 进程管理**（新 crate `ip-worker-client`）：开发模式下在仓库 `ai-worker/` 执行 `uv run imagepicker-ai serve --host 127.0.0.1 --port 0 --token <随机> --parent-pid <core pid>`，读取 stdout 第一行 `{"event":"ready","port":N}`；可用 `IMAGEPICKER_WORKER_CMD`（完整命令行）与 `IMAGEPICKER_WORKER_DIR` 覆盖。懒启动（首次分析时）、崩溃自动重启（指数退避，最多 3 次）、退出时杀进程树。
- **存储**：`analysis`、`face`、`person`、`burst`、`scene` 表按 doc 05 §1；嵌入从 worker 产物文件读入 BLOB（f16）。
- **分组**（doc 03 §2.2）、**评分与星级**（§3.2）、**人物聚类**（§4.3：连拍组内轨迹关联 + 跨组按 cos ≥ 阈值的并查集/平均链接；用户合并/命名作为约束）、**is_subject**（§4.2）、`face_count`/`subject_face_count` 全部在 Rust 中完成。
- **问题标签**（`issues`）：`closed_eyes`（任一主体人脸 eyes_open < 0.45）、`blurry`、`overexposed`、`underexposed`、`noisy`、`tilted`（M2 可不实现）。

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
| POST | `/api/analysis/run` | `{"session_id", "profile": "fast"\|"standard", "photo_ids"?: number[]}` | `202 {"task_id"}`；若所需模型未安装 → `409 {"error":{"code":"models_missing","message",...},"models":[id...]}`（UI 征得同意后调用 `/api/models/ensure` 再重试） |
| POST | `/api/analysis/cancel` | `{"session_id"}` | `204` |
| GET | `/api/analysis/status?session_id=` | — | `{"state":"idle"\|"running"\|"done"\|"failed","profile","done","total","stage":"analyzing"\|"grouping"\|"scoring"\|"clustering"\|null,"error":string\|null}` |
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
  "scores": { "sharpness": 0.82, "exposure": 0.74, "noise": 0.12, "iqa": 0.71, "aesthetic": 0.64, "face": 0.58, "composition": null },
  "ai_score": 0.68, "ai_rating": 4.0,
  "contributions": [ { "key": "face", "label_key": "score.face", "delta": -0.12 } ],  // 对综合分的贡献（可正可负）
  "reasons": [ { "key": "closed_eyes", "params": { "person": "小红" } } ],          // 前端按 key 本地化
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
| `scene_type` | 场景类型 |
| `persons` / `person_mode` / `exclude_persons` / `person_state` / `include_background` / `faces_min` / `faces_max` | 见 [05 §3.2](05-数据与接口设计.md)（`person_state` M2 支持 `eyes_open,smiling,looking,subject`） |
| `sort` | 新增 `ai`（ai_score 降序，未分析在后） |

## D. WebSocket 新增事件

```jsonc
{ "type": "analysis.progress", "session_id": 1, "state": "running", "stage": "analyzing", "done": 120, "total": 1204 }
{ "type": "analysis.updated",  "session_id": 1, "ids": [12, 13] }   // 这些照片的 AI 字段变化，客户端重新拉取
{ "type": "groups.updated",    "session_id": 1 }
{ "type": "people.updated",    "session_id": 1 }
{ "type": "worker.status",     "state": "stopped" | "starting" | "ready" | "busy" | "crashed" | "unavailable", "tier": "T3", "error": null }
```

合并规则同 M1（100ms）：`analysis.updated` 按 session 合并 ids，其余保留最新。
