# API 契约 · M3（修图引擎）

> 在 [M1](api-contract-m1.md)、[M2](api-contract-m2.md) 契约基础上**增量**扩展。
> 编辑栈 JSON 的权威定义是 `crates/ip-render/src/lib.rs`（`EditStack`/`Op`/`Adjust`/`MaskRef`…，serde 序列化即 JSON 形状），与 [05 §2](05-数据与接口设计.md) 一致。前端 TypeScript 类型须与之逐字段对应。
> 涉及四方：`ip-render`（渲染引擎）、`ip-core`/`ip-server`（存储、接口、导出）、`web`（编辑界面）、`ai-worker`（AI 蒙版）。

## A. 编辑栈要点（摘自 ip-render）

```jsonc
{ "version": 1, "ops": [
  { "type": "crop",   "rect": [x, y, w, h], "angle": -1.3, "aspect": "4:5" },        // 归一化，旋转后坐标
  { "type": "global", "exposure": 0.35, "contrast": 12, "highlights": -30, "shadows": 22, "whites": 5, "blacks": -4,
                      "temp": 300, "tint": 2, "vibrance": 10, "saturation": 0, "clarity": 8, "dehaze": 0,
                      "curve": { "rgb": [[0,0],[1,1]], "r": [], "g": [], "b": [] },
                      "hsl": { "orange": { "h": 0, "s": -5, "l": 8 } },
                      "grading": { "shadows": [220, 0.1], "midtones": [0, 0], "highlights": [40, 0.08], "balance": 0 },
                      "source": "ai_auto@1" },
  { "type": "local",  "mask": { "kind": "ai", "target": "sky" }, "amount": 1, "invert": false, "adjust": { "exposure": -0.3 } },
  { "type": "local",  "mask": { "kind": "radial", "center": [0.5,0.4], "radius": [0.3,0.4], "feather": 0.5 }, "adjust": { "exposure": 0.2 } },
  { "type": "local",  "mask": { "kind": "linear", "start": [0.5,0], "end": [0.5,0.45] }, "adjust": { "dehaze": 20 } },
  { "type": "lut",    "file": "film_warm", "amount": 0.6 },
  { "type": "output_sharpen", "amount": 20 }
]}
```

- 滑块范围：`exposure` −5..5 EV；`temp` −3000..3000 K（相对原始白平衡）；其余 −100..100。全部默认 0。
- 一个栈中通常只有一个 `global`（UI 只编辑第一个）；`local` 可多个；未知 `type`（`warp`、`beauty`、`patch` 等 M4/M5 操作）原样保留、M3 渲染时忽略。
- AI 蒙版目标：`subject` `background` `sky` `person`（配合 `person_id`）`skin` `hair` `clothes`。
- 空栈 `{"version":1,"ops":[]}` = 未编辑。

## B. REST 新增

| 方法 | 路径 | 请求 | 响应 |
|---|---|---|---|
| GET | `/api/edits/{photo_id}` | — | `{"photo_id","stack":EditStack,"updated_at":number\|null}`（未编辑返回空栈） |
| PUT | `/api/edits/{photo_id}` | `{"stack": EditStack}` | `{"photo_id","stack","updated_at","thumb_version"}`；空栈等价于重置；广播 `edits.updated` |
| DELETE | `/api/edits/{photo_id}` | — | `204`（重置为原图） |
| POST | `/api/render/preview` | `{"photo_id", "stack"?: EditStack /*缺省用已保存*/, "long_edge"?: number=1600, "original"?: bool /*true=忽略编辑，用于前后对比*/}` | `image/jpeg`（q=90），响应头 `X-Render-Ms`、`X-Render-Backend: gpu\|cpu`。拖动滑块时前端用 `long_edge≈800`，松手后用屏幕分辨率 |
| POST | `/api/edits/{photo_id}/auto` | `{"mode": "auto"\|"portrait"\|"landscape"}` | `{"adjust": Adjust}`（**不保存**，前端合并进当前栈的 global 后 PUT） |
| POST | `/api/edits/sync` | `{"from_id", "to_ids": number[], "include": ("crop"\|"global"\|"local"\|"lut"\|"output_sharpen")[], "adaptive"?: bool=true}` | `{"updated": number}`；`adaptive` 时按每张照片的曝光/白平衡差异微调 global（docs/03 §6.2）；广播 `edits.updated` |
| GET | `/api/presets` | — | `{"presets":[{"id","name","builtin":bool,"stack":EditStack}]}`（内置预设 `name` 为 i18n key，如 `preset.film_warm`） |
| POST | `/api/presets` | `{"name", "stack": EditStack}` | `201 {"preset"}`（只保存 global/local(非 AI person)/lut/output_sharpen；去掉 crop） |
| DELETE | `/api/presets/{id}` | — | `204`（内置预设不可删 → 400） |
| POST | `/api/luts/import` | `{"path": string /*本机 .cube 文件*/}` | `{"id","name"}`（复制到数据目录 `luts/`，之后可用 `{"type":"lut","file":id}` 引用） |
| GET | `/api/masks/{photo_id}?target=sky&person_id=` | — | `image/png`（8-bit 灰度，分析分辨率），按需由 worker 生成并缓存；模型缺失 → `409 models_missing`（同 M2 形状），worker 不可用 → `503` |

### 已有接口的行为变化

- `Photo` 新增 `has_edits: boolean`。
- 有编辑的照片：`/api/thumb` 与 `/api/preview` 返回**渲染后**的图像；`thumb_version` 随编辑变化（包含编辑栈哈希），因此 URL 自动失效。
- `/api/export` 导出时应用已保存的编辑（全分辨率分块渲染）；新增可选 `"apply_edits": bool = true`。
- 编辑写入撤销历史由前端负责（与评分一致：保存前取旧栈，撤销时 PUT 旧栈）。

## C. WebSocket 新增事件

```jsonc
{ "type": "edits.updated", "items": [{ "id": 12, "has_edits": true, "thumb_version": "a1b2c3d4" }] }
```

按 id 合并（保留最新）。

## D. Worker：`mask.generate`

```jsonc
// 请求
{ "method": "mask.generate", "params": {
    "photo": { "photo_id": 12, "path": "E:/…/IMG_1.JPG", "orientation": 1 },
    "targets": ["sky", "subject", "skin"],          // 见 A 中目标列表；person 需配合 person_bbox
    "person_bbox": [x, y, w, h] | null,            // target=person 时：该人脸框（归一化），worker 据此选出对应人体实例
    "size": 1024,                                  // 蒙版长边
    "out_dir": "<cache>/masks",
    "allow_download": false } }
// 结果
{ "masks": { "sky": "<out_dir>/12_sky.png", "subject": "...", "skin": "..." },
  "models": { "sky": "<model id>", ... }, "skipped": { "hair": "model_unavailable" } }
```

- PNG 为 8-bit 灰度、与正向（已应用 EXIF 朝向）图像同宽高比，255 = 属于目标。
- `background` 由 Core 用 `subject` 取反得到，worker 不必支持。
- 模型优先宽松许可（MIT/Apache）；缺模型时该目标进入 `skipped`，其余照常返回。
