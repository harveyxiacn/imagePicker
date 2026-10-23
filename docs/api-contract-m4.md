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
| POST | `/api/faces/search` | `multipart/form-data` 字段 `image`，或 JSON `{"face_id"}`；可选 `session_id` | `{"faces_detected":[[x,y,w,h]],"query_face":0,"candidates":[{"person_id","person_name","similarity"}],"similar_faces":[{"face_id","photo_id","person_id":number\|null,"person_name":string\|null,"similarity"}]}`（多张脸时可再带 `face_index` 重试；`similar_faces` 的 `person_id` 为该脸当前所属人物，`null` = 未归属） |
| POST | `/api/people` | `{"face_ids": number[], "name"?: string\|null}` | `201 {"person"}`：「这是新的人」，见下 |
| GET | `/api/collections` | — | `{"collections":[{"id","name","query":string /*URLSearchParams，与 /api/photos 相同参数，不含 session_id/cursor/limit*/,"builtin":bool}]}`；内置：`best_per_group`、`has_closed_eyes`、`undecided`、`edited`、`duplicates`（`duplicates=1`，api-contract-m2 C.6） |
| POST | `/api/collections` | `{"name","query"}` | `201 {"collection"}` |
| PATCH/DELETE | `/api/collections/{id}` | `{"name"?,"query"?}` | `{"collection"}` / `204`（内置不可改删 → 400） |
| GET | `/api/taste` | — | `TasteOut`（见下） |
| PATCH | `/api/taste` | `{"enabled": bool}` | `TasteOut`。关闭：不再记录新决定、不训练，立即撤销融合与风格分（标签和模型保留）；打开：沿用保留的模型，没有可用模型时后台训练 |
| POST | `/api/taste/train` | — | `202` + 当前 `TasteOut`（`training: true`），训练在后台进行，完成后广播 `taste.updated`；关闭时 `400` |
| POST | `/api/taste/reset` | 可选 `{"disable": bool}` | `204`。删除全部标签、模型、指标与风格分，AI 分回到基础分；`disable: true` 同时关闭学习（忘记并退出） |
| POST | `/api/taste/forget` | `{"photo_id": n}` 或 `{"session_id": n}`（恰好一个） | `202` + 当前 `TasteOut`（`training: true`）：删除这些照片记录的决定，后台用剩下的标签重新训练，完成后广播 `taste.updated`；关闭学习时 `400`，两个都给或都不给 `400` |
| GET | `/api/taste/explain/{photo_id}` | — | `{"photo_id","taste_score":number\|null,"base_score":number\|null,"contributions":[{"feature","delta","direction":"high"\|"low"}]}`：对风格分贡献最大的正、负各 ≤3 项（`feature` 为可解释特征名、`scene_<x>` 或 `look` = 整体画面与内容）；没有可用模型时 `taste_score: null`、`contributions: []`；未知照片 `404` |

`TasteOut`：

```jsonc
{
  "labels": 184,              // 与照片当前星级/旗标一致的用户标签数
  "legacy_labels": 40,        // 1.0 记录的标签（可能含助手「淘汰其余」的旗标）
  "training": false,          // 正在训练
  "positives": 92, "negatives": 61, "min_per_class": 30,   // 已分析照片中留下 / 淘汰的张数（3 星为中性）
  "pending": 12, "retrain_every": 50,                      // 上次训练以来的新决定
  "enabled": true,
  "state": "active",          // disabled | collecting（冷启动）| ready | not_better（不优于通用分）| active
  "active": true, "alpha": 0.55,                           // 融合权重 0..0.6
  "holdout_accuracy": 0.74, "base_accuracy": 0.63,         // 交叉验证准确率：风格模型 / 通用分
  "metrics": { "auc": 0.78, "base_auc": 0.64, "accuracy": 0.74, "base_accuracy": 0.63, "margin": 0.04,
               "eval_positives": 92, "eval_negatives": 61, "folds": 5, "lambda": 0.03, "dim": 768,
               "fit_photos": 153, "train_ms": 210 },       // 未训练过时为 null
  "traits": [{ "key": "likes", "params": { "feature": "eyes_open", "strength": 0.8 } }],  // likes | dislikes，仅 active 时
  "scored": 4210,             // 有风格分的照片数
  "feature_version": 2,
  "updated_at": 1760000000000
}
```

`PATCH /api/photos` 新增可选 `"source": "user" | "auto" | "undo"`（缺省 `user`）：`auto` = 应用自己的决定（客户端重做助手 / 接受 AI 评分的步骤），`undo` = 撤销这类步骤；两者都不产生风格标签，只让与照片不再一致的旧标签静音、或让恢复一致的标签重新生效。其它值 `400`。

照片列表（`GET /api/photos`、`/api/search` 等）在有可用模型时每项多一个 `taste_score`（0..1，用户留下它的概率；没有时省略），`sort=taste` 按它降序排列（无分的排最后）。

- 「这是新的人」（以脸搜脸结果里勾选同一个人的脸；以库内人脸搜索时查询脸本身也计入）：`POST /api/people` 新建人物，`face_ids` 中的人脸离开原人物（或「未归属」）并**锁定**到新人物——与「不是此人」/合并相同的用户约束，重新聚类与强制重新分析都不改变（锁定的脸按位置继承人物）。`name` 去空白后为空 = 未命名。原人物重算封面与中心，没有脸且未命名时删除；新人物的中心参与之后的聚类（相似的未锁定人脸可能自动归入）。同一照片的两张脸 → `400`（一个人在一张照片中只出现一次）；`face_ids` 为空 → `400`；未知人脸 → `404`。广播 `people.updated`（主角因此变化时另有 `analysis.updated`）。没有撤销（与合并、「不是此人」一致）：用合并或「不是此人」改回。
- 个性化（03 §3.3，taste v2）：只有用户亲手的 `PATCH /api/photos` 评分 / 旗标是标签（助手操作、「接受 AI 评分」不是，并会删除被覆盖的旧标签）；每张照片按「淘汰或 ≤2 星 = 负、精选或 ≥4 星 = 正、3 星中性」归类，跨全部会话训练 L2 逻辑回归（嵌入 ⊕ 可解释分项 ⊕ 问题位 ⊕ 场景）。正负例各 ≥30 张才训练；5 折交叉验证 AUC 须 ≥0.6 且超过通用分一定余量才启用。新决定满 50 个立即、满 10 个且停顿 2 分钟后在后台重训，也可 `POST /api/taste/train`。启用后 `ai_score`/`ai_rating` 为融合结果、`taste_score` 写入缓存并广播 `analysis.updated`。1.0 的「同组未标注照片作隐式负例」与 `accept_ai` 标签在 v2 中不再使用。
- `/api/export` 新增 `"folders"?: {"<子文件夹名>": number[]}`：按人物等分组导出到子文件夹（与 `ids` 二选一）。

## D. WebSocket 新增事件

```jsonc
{ "type": "beauty.ready",   "photo_id": 12 }
{ "type": "taste.updated",  "labels": 150, "active": true, "alpha": 0.3 }
{ "type": "collections.updated" }
```
