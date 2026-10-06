# API 契约 · M4（人像美化 + M2 补完）

> 在 M1–M3 契约基础上**增量**扩展。编辑栈中 `beauty` / `warp` 操作与 `PersonGeometry` 的权威定义见 `crates/ip-render/src/lib.rs`。
> 算法依据：[03 §7](03-AI与算法设计.md)（人像美化）、§3.3（个性化评分）、§4.4（按人脸筛选：每人最佳、以脸搜脸）。
> 涉及：`ip-render`（美颜/形变渲染）、`ai-worker`（`beauty.prepare` 几何）、`ip-core`/`ip-server`、`web`。

## A. 编辑栈新增操作（摘自 ip-render）

```jsonc
{ "type": "beauty", "person_id": 12 /* 省略或 null = 所有人脸 */, "level": "natural" | "standard" | "refined",
  "smooth": 30, "whiten": 20, "blemish": true, "eye_brighten": 10, "teeth_whiten": 0, "dark_circles": 15 }      // 0..100
{ "type": "warp", "kind": "face", "person_id": 12, "level": "natural", "slim": 25, "chin": 0, "eyes": 10, "nose": 0 }  // -100..100
{ "type": "warp", "kind": "body", "person_id": 12, "level": "natural", "arms": 20, "legs": 15, "waist": 10, "lengthen_legs": 0,
  "protect_background": true }                                                                                    // 0..100
```

- 渲染顺序：crop → **warp** → global → local → **beauty** → LUT → sharpen。
- **形变是参数化的**：渲染器根据该人的几何（人脸 478 点、人体 33 点、人体蒙版）和滑块值实时计算位移场，不需要往返 worker。
- `level` 对所有数值做缩放与上限保护：自然档人脸位移 ≤ 脸宽 3%，精致档 ≤ 6%；身体形变自然档 ≤ 肢体宽度 8%，精致档 ≤ 15%。
- 同一 `person_id` 的同类操作，UI 只编辑第一个。`person_id` 指向的人在该照片中不存在时，操作被跳过（复制到别的照片时尤其常见）。

## B. Worker：`beauty.prepare`

```jsonc
// 请求
{ "method": "beauty.prepare", "params": {
    "photo": { "photo_id": 12, "path": "...", "orientation": 1 },
    "faces": [ { "face_id": 881, "bbox": [x, y, w, h] } ],   // 来自分析结果（归一化，正向图）；为空时 worker 自行检测
    "size": 1536,                                          // 蒙版长边
    "out_dir": "<cache>/beauty", "allow_download": false } }
// 结果
{ "people": [ {
    "face_id": 881,
    "face_box": [x, y, w, h],
    "face_landmarks": [[x, y], ...],          // 478 点，归一化；不可用时 []
    "pose": [[x, y, visibility], ...],        // 33 点（MediaPipe Pose）；不可用时 []
    "skin_mask": "<out_dir>/12_881_skin.png", // 仅此人，已扣除眉/眼/唇（由人脸关键点多边形）
    "body_mask": "<out_dir>/12_881_body.png", // 仅此人的人体蒙版
    "blemishes": [[x, y, r], ...]             // r 相对图像长边归一化
  } ],
  "models": { ... }, "skipped": { ... } }
```

- 坐标一律为正向（已应用 EXIF 朝向）图像的归一化坐标，裁剪前。
- 模型宽松许可优先：MediaPipe Face Landmarker / Pose Landmarker / Selfie Multiclass（Apache-2.0）、BiRefNet（MIT）。

## C. REST 新增 / 变更

### C.1 人像美化

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/photos/{id}/people` | — | `{"people":[{"face_id","person_id":number\|null,"person_name":string\|null,"face_box":[x,y,w,h],"is_subject":bool,"has_pose":bool,"has_profile":bool}],"ready":bool}`；`ready=false` 表示几何尚未准备，前端调用下面的 prepare |
| POST | `/api/photos/{id}/beauty/prepare` | — | `202 {"task_id"}`；完成后广播 `beauty.ready {photo_id}`；模型缺失 → `409 models_missing` |
| GET | `/api/people/{id}/beauty-profile` | — | `{"profile": BeautyProfile \| null}` |
| PUT | `/api/people/{id}/beauty-profile` | `{"profile": BeautyProfile \| null}` | `{"profile"}`（null = 删除） |
| POST | `/api/edits/apply-profiles` | `{"photo_ids": number[]}` | `{"updated"}`：为每张照片中有档案的人物，替换/插入其 `beauty` 与 `warp` 操作；广播 `edits.updated` |

```ts
interface BeautyProfile {           // 不含 person_id；应用时填入
  beauty?: Omit<Beauty, "person_id">;
  face?: Omit<WarpFace, "person_id" | "kind">;
  body?: Omit<WarpBody, "person_id" | "kind">;
}
```

### C.2 M2 补完

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/people/best?session_id=&ids=1,2&n=3` | — | `{"people":[{"person_id","photos":[{"photo_id","score"}]}]}`（03 §4.4：该人表情分 × 照片综合分，同连拍组只取一张） |
| POST | `/api/faces/search` | `multipart/form-data` 字段 `image`，或 JSON `{"face_id"}`；可选 `session_id` | `{"faces_detected":[[x,y,w,h]],"query_face":0,"candidates":[{"person_id","person_name","similarity"}],"similar_faces":[{"face_id","photo_id","similarity"}]}`（多张脸时可再带 `face_index` 重试） |
| GET | `/api/collections` | — | `{"collections":[{"id","name","query":string /*URLSearchParams，与 /api/photos 相同参数，不含 session_id/cursor/limit*/,"builtin":bool}]}`；内置：`best_per_group`、`has_closed_eyes`、`undecided`、`edited` |
| POST | `/api/collections` | `{"name","query"}` | `201 {"collection"}` |
| PATCH/DELETE | `/api/collections/{id}` | `{"name"?,"query"?}` | `{"collection"}` / `204`（内置不可改删 → 400） |
| GET | `/api/taste` | — | `{"labels":number,"active":bool,"alpha":number /*0..0.6 融合权重*/,"holdout_accuracy":number\|null,"traits":[{"key","params"}] /*「你偏好低饱和」等，i18n key*/,"updated_at"}` |
| POST | `/api/taste/reset` | — | `204`（清空偏好数据，回到基础评分） |

- 个性化（03 §3.3）：Core 在用户评分/旗标/组内选择时记录偏好（显式评分、成对：选中 A 而淘汰同组 B ⇒ A ≻ B），每新增 50 条自动后台重训（Rust，排序头：嵌入 ⊕ 分项 ⊕ 场景），保留集上不优于基础分时自动停用；启用后 `ai_score`/`ai_rating` 为融合结果并广播 `analysis.updated`。
- `/api/export` 新增 `"folders"?: {"<子文件夹名>": number[]}`：按人物等分组导出到子文件夹（与 `ids` 二选一）。

## D. WebSocket 新增事件

```jsonc
{ "type": "beauty.ready",   "photo_id": 12 }
{ "type": "taste.updated",  "labels": 150, "active": true, "alpha": 0.3 }
{ "type": "collections.updated" }
```
