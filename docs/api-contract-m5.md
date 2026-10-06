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
| GET | `/api/bursts/{id}/besttake` | — | `{"base_photo_id","people":[{"track_id","person_id","person_name","base_face_id","candidates":[{"photo_id","face_id","expression_score","composable":bool,"reason":string\|null}],"best_photo_id"}]}`（底片 = 组内最佳；候选按表情分降序） |
| POST | `/api/besttake` | `{"base_photo_id", "choices":[{"base_face_id","source_photo_id","source_face_id"}]}` | `202 {"task_id"}`；完成后底片编辑栈中**替换/插入**对应人物的 `best_take` 补丁并广播 `edits.updated`；每个选择的结果经 `besttake.done {photo_id, results:[{base_face_id, ok, warnings, reason}]}` 推送 |
| POST | `/api/bursts/{id}/besttake/auto` | — | `202 {"task_id"}`：自动选底片 + 每人最佳可合成表情，同上 |
| GET | `/api/photos/{id}/bystanders` | — | `{"faces":[{"face_id","bbox"}]}`（非主体人脸） |
| POST | `/api/photos/{id}/inpaint` | `{"bystanders": true}` 或 `{"face_ids":[...]}` 或 `{"strokes":[{"points":[[x,y],...],"radius":0.02}]}`（归一化，正向裁剪前坐标）；可选 `"model":"lama"\|"sdxl"` | `202 {"task_id"}`；完成后追加 `inpaint` 补丁，广播 `edits.updated` 与 `inpaint.done {photo_id, ok, reason}` |
| POST | `/api/photos/{id}/enhance` | `{"op":"denoise"\|"face_restore","strength":0..1}` | `202 {"task_id"}`；追加/替换同类补丁 |
| GET | `/api/assets/{photo_id}/{asset}` | — | `image/png`（调试/前端预览补丁用） |

- 所有生成任务：模型缺失 → `409 models_missing`；worker 不可用 → `503`；进度走 `task.progress`（`kind`: `besttake`/`inpaint`/`enhance`）。
- `/api/export` 新增 `"upscale"?: 2 | 4`：渲染后经 `enhance.run upscale` 放大再编码。

## D. 人物与人像相关的修正（M4 联调发现）

- 只出现在一张照片中的**主体**人脸也建立人物（`person.singleton = true`），使其可在人像面板中被单独选中；`GET /api/people` 默认不返回单张人物，`?include_singletons=1` 返回。
- `face_box`、`bbox`、`similarity` 等浮点字段输出时不得出现 f32→f64 噪声（四舍五入到 4 位小数或按 f32 最短表示序列化）。

## E. WebSocket 新增事件

```jsonc
{ "type": "besttake.done", "photo_id": 4231, "results": [{ "base_face_id": 881, "ok": true, "warnings": [], "reason": null }] }
{ "type": "inpaint.done",  "photo_id": 4231, "ok": true, "reason": null }
{ "type": "enhance.done",  "photo_id": 4231, "op": "denoise", "ok": true, "reason": null }
```
