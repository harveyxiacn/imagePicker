# API 契约 · M9（精修：纹理 / 减淡加深 / 效果 / 细节 / 范围蒙版）

> 在 [M3](api-contract-m3.md)、[M4](api-contract-m4.md) 基础上**增量**扩展，形状与其它契约相同：
> 编辑栈 JSON 的权威定义仍是 `crates/ip-render/src/lib.rs`，本文档描述它新增的字段与语义。
> 本次只增不改：不含新字段的旧栈渲染结果不变，旧前端忽略新字段也不会报错。

## A. 编辑栈新增

### A.1 `Adjust` 追加三个字段

```jsonc
{ "type": "global", "exposure": 0.3,
  "texture": 25,     // -100..100，中尺度细节
  "dodge": 18,       //   0..100，减淡：提亮影调，高光受保护
  "burn": 0 }        //   0..100，加深：压暗影调，最深处受保护
```

- `texture` 与 `clarity` / `dehaze` 同级，参与「基础」面板的 *Presence* 分组。它作用于比 `clarity` 更细一档的亮度带，权重为 `t·e^(1−t)`（`t` = 局部振幅 / `0.12`）：平坦区与硬边缘都趋近 0，因此**不放大噪点、不产生边缘光晕**；负值用于磨皮方向地抹平纹理。
- `dodge` / `burn` 只接受单方向数值（负数被校验拒绝）。它们按 OKLab 亮度抬升/压低，并各自带 `1−L` / `L` 的幂次衰减，保持色相角不变——提亮肤色不会转灰。
- 三者都是**逐像素**操作，CPU 与 GPU 两条后端必须一致（见 E）。

### A.2 新 op：`effects`（暗角 + 颗粒）

```jsonc
{ "type": "effects",
  "vignette": { "amount": -35, "midpoint": 50, "roundness": 0, "feather": 50,
                "highlight_contrast": 20,
                "style": "highlight_priority" },   // highlight_priority | color_priority | paint_overlay
  "grain":    { "amount": 30, "size": 25, "frequency": 50 } }
```

| 字段 | 范围 | 默认 | 说明 |
|---|---|---|---|
| `vignette.amount` | −100..100 | 0 | 负=压暗四角，正=提亮；0 = 关闭（整个 `vignette` 可省略） |
| `vignette.midpoint` | 0..100 | 50 | 衰减起点 |
| `vignette.roundness` | −100..100 | 0 | 负=清晰区收成菱形（暗框更接近矩形），正=清晰区变椭圆 |
| `vignette.feather` | 0..100 | 50 | 过渡宽度 |
| `vignette.highlight_contrast` | 0..100 | 0 | 按像素自身亮度回撤，避免压暗后的高光被压死 |
| `vignette.style` | 枚举 | `highlight_priority` | 高光优先 / 颜色优先（保留暗角处的彩度） / 叠加绘画（在显示空间混合到黑或白） |
| `grain.amount` | 0..100 | 0 | 0 = 关闭（`grain` 可省略） |
| `grain.size` | 0..100 | 25 | 颗粒胞元 1.2..14 px |
| `grain.frequency` | 0..100 | 50 | 两个八度之间的混合比 |

- 形状是超椭圆 `(|dx|^p+|dy|^p)^(1/p)`，`p = 2 + roundness/100`，对角归一化到四角，因此三个方向上四角都是满强度。
- 颗粒是**位置的纯函数**（整数哈希 + 平滑插值），逐帧稳定；胞元尺寸按**源图**像素换算，预览与导出颗粒粗细一致。
- 渲染位置：全局/局部/人像之后、LUT 之前（见 A.4）。它是一个逐像素 op，跑在 GPU 上。

### A.3 新 op：`detail`（锐化 + 降噪）

```jsonc
{ "type": "detail",
  "sharpen": { "amount": 60, "radius": 1.0, "detail": 25, "masking": 30 },
  "noise":   { "luminance": 35, "luminance_detail": 50, "luminance_contrast": 0,
               "color": 25, "color_detail": 50 } }
```

| 字段 | 范围 | 默认 | 说明 |
|---|---|---|---|
| `sharpen.amount` | 0..150 | 0 | 0 = 关闭（`sharpen` 可省略） |
| `sharpen.radius` | 0.5..3.0 | 1.0 | 输出像素 |
| `sharpen.detail` | 0..100 | 25 | 细/粗结构的混合比，同时收紧光晕限幅 |
| `sharpen.masking` | 0..100 | 0 | 0 = 全图锐化，100 = 只保留强边缘 |
| `noise.luminance` | 0..100 | 0 | 亮度降噪；为 0 时 `noise` 可整体省略 |
| `noise.luminance_detail` | 0..100 | 50 | 加回多少被去掉的细节带 |
| `noise.luminance_contrast` | 0..100 | 0 | 回补低频对比 |
| `noise.color` | 0..100 | 0 | 彩噪平滑强度 |
| `noise.color_detail` | 0..100 | 50 | 彩噪平滑半径（越大细节越多） |

- **整图阶段**：降噪与锐化都需要全图统计（噪声水平由本帧高频带的中位数估计），无法在 GPU 的分块 pass 内完成，因此两者由共享代码 `crates/ip-render/src/detail.rs` 在**编码后的 8-bit 结果**上执行，两个后端都调用它——CPU/GPU 在这里是结构上一致，而不是数值上接近。
- 顺序：先降噪、后锐化。若栈中同时存在 `output_sharpen`，`detail` 存在时把它并入本阶段末尾执行，保证「降噪 → 锐化」不被颠倒。
- 噪声水平自适应：干净的照片不会被压平（`a_clean_image_is_not_flattened_by_noise_reduction`）。
- 掩码从**模糊后**的亮度取梯度并做 3×3 膨胀，使光晕带整体参与，而不是只有恰好一列接缝。

### A.4 局部调整新增 `range`（亮度 / 颜色范围蒙版）

```jsonc
{ "type": "local",
  "mask": { "kind": "linear", "start": [0.5, 0], "end": [0.5, 0.6] },
  "amount": 1, "invert": false,
  "range": { "kind": "luminance", "min": 20, "max": 70, "softness": 25 },
  "adjust": { "exposure": -0.5 } }

{ "type": "local", "mask": { "kind": "ai", "target": "sky" },
  "range": { "kind": "color", "hue": 210, "width": 45, "sat_min": 10, "sat_max": 100, "softness": 40 },
  "adjust": { "saturation": -30 } }
```

| `range.kind` | 字段 | 范围 | 默认 |
|---|---|---|---|
| `luminance` | `min` / `max` | 0..100，`min ≤ max` | — / — |
| | `softness` | 0..100 | 50 |
| `color` | `hue` | 0..360° | — |
| | `width` | 0..180°（半角） | — |
| | `sat_min` / `sat_max` | 0..100，`sat_min ≤ sat_max` | 0 / 100 |
| | `softness` | 0..100 | 50 |

- 语义是**乘在空间蒙版上**（`m = 空间蒙版 × 范围权重`），永远不会扩大蒙版；`invert` 只反转空间部分，范围部分不变。
- 选择曲线是梯形/楔形，肩宽由 `softness` 决定，因此不会出现阈值硬边。
- 颜色范围用 OKLab：`hue` 是 OKLab 色相角，`sat_min`/`sat_max` 是归一化彩度（`chroma / 0.26`）。**中性色（归一化彩度 < 0.05）不参与色相判断**——灰色的色相角是浮点残差，不这样处理 CPU 与 GPU 会在灰像素上给出不同结果。
- 未知 `kind` 被忽略（等价于没有范围蒙版），保证前向兼容。

### A.5 流水线顺序（已更新）

```
decode → geometry(crop/rotate) → [warp/patch] → global → local(蒙版×范围) → [beauty]
       → effects → LUT → detail(降噪→锐化) → output_sharpen
```

`effects` 是逐像素 op，位置影响结果；`detail` 是整图阶段，永远在 LUT 之后、输出锐化之前执行，与它在 `ops` 数组中的位置无关。前端 `orderOps` 会把它排到 `lut` 之后，使等价外观共享同一缓存键。

## B. 校验（`POST/PUT /api/edits`、`edits/sync`）

新增的校验都在 `ip-core::edit::check_op` / `check_adjust` / `check_range`，失败一律 `422 unprocessable`：

- `texture` −100..100；`dodge`、`burn` 0..100。
- `effects`：`vignette.amount`/`roundness` −100..100，其余 0..100。
- `detail`：`sharpen.amount` 0..150，`radius` 0.5..3.0，其余 0..100。
- `range`：`min ≤ max`、`sat_min ≤ sat_max`，各字段按上表范围。
- `op_is_noop` 认可中性的 `effects` / `detail`（与 `EditStack::is_identity` 一致），因此「效果全 0」不会把照片标记为已编辑。

## C. 接口变化

| 接口 | 变化 |
|---|---|
| `PUT /api/edits/{photo_id}` | 请求体接受 A.2/A.3 的 op 与 A.4 的 `range`；响应不变 |
| `POST /api/edits/sync` | `include` 新增取值 `"effects"` 与 `"detail"`（`SyncKind` 已扩展）；旧取值语义不变 |
| `POST /api/presets` | 保存的区段新增 `effects` 与 `detail`（`PRESET_SECTIONS`） |
| `POST /api/render/preview`、导出 | 无变化；新 op 由渲染引擎自动应用 |

WebSocket 事件无需新增：所有改动都通过既有的 `edits.updated` 广播。

## D. 前端

- `web/src/api/types.ts`：`Adjust` 增加 `texture`/`dodge`/`burn`（`ADJUST_NUMERIC_KEYS` 同步，`ONE_WAY_KEYS` 标记单向滑块），新增 `EffectsOp`、`DetailOp`、`MaskRange`；`EDIT_SECTIONS` 增加 `effects`、`detail`（复制/粘贴、同步、预设自动覆盖新 op）。
- `web/src/lib/edit.ts`：`setVignette`/`setGrain`/`setSharpenDetail`/`setNoiseReduction`/`setLocalRange`；中性子对象在 `normalizeStack` 中归一化掉（全 0 的 `effects` 不出现在栈里）。
- 新面板：`EffectsPanel`（暗角 + 颗粒，含样式切换）、`DetailPanel`（锐化 + 降噪）；「基础」面板的 Presence 组加入 Texture，并新增减淡/加深组；`LocalPanel` 每个局部调整下新增范围蒙版编辑器。
- i18n：`edit.*`、`adjust.*`、`history.*`、`edit.section_*` 均已补齐 zh-CN 与 en，`i18n.test.ts` 里登记了新键。

## E. 后端一致性（测试如何钉住）

- `crates/ip-render/tests/retouch.rs`：每个新控件都有行为断言（方向、单调性、中性即恒等、确定性），并对 `scene` / `macbeth` / `noise` 做 CPU↔GPU 的 ΔE2000 对比。
- `crates/ip-render/tests/render.rs` 的随机栈 fuzz 现在会生成 texture / dodge / burn / effects / detail / range，任一随机栈的 `mean ΔE < 1`、`max ΔE < 3`。
- `detail` 阶段的 CPU/GPU 一致性是结构性的（同一个函数），测试仍然钉住它，防止将来把它单独移植到 GPU 后悄悄漂移。
