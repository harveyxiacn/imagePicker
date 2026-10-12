# 合成照片库 · standard 档位 AI 全链路评测（2026-10-09）

评测集：`bench/data/qwen-photo-library`（Qwen-Image 合成，1000 张 JPEG + `manifest.json`；8 位「演员」a1–a8、60 组连拍（30 单人 `bs*` + 30 合影 `bg*`，变体 base/best/blur/eyes-closed/gaze-away/laugh/mid-talk）、60 张缺陷图、48 张边界样本）。标签是提示词意图，未逐张目检（`visual-review.json`：`bg01-family-dinner-02` 实际闭眼的是两个人），数字只作相对参考。

机器：ROG 台式（Windows 11，i7-13700K 16C/24T，64 GB，RTX 4090 24 GB，驱动 616.92）。代码：分支 `test/e2e-standard`（main `5daf33e1` 上测分析与大部分功能；合并 `origin/main` `4725274d`（最佳表情选底片）后重测最佳表情与 AI 助手；分析代码两者相同）。本分支的两个修复：`c96f7160`（消除路人空蒙版）、`ab2f2c4b`（AI 助手认不出本地模型）。同一台机器上同时有其他 agent 的构建 / 测试在跑，吞吐与 GPU 占用数字偏保守。

命令：

```powershell
# AI 组件（开发者方式）
cd ai-worker; uv sync --extra cuda --extra mediapipe
# 分析（首次带 --allow-download 下载 standard 模型）
target\release\imagepicker.exe analyze <库>\jpeg --profile standard --allow-download --models-dir target\models --data-dir target\eval\standard
python bench\eval-library.py  <库> target\eval\standard        # 问题标签（含 closed_eyes）、连拍、最佳帧
python bench\eval-standard.py <库> target\eval\standard --gpu-csv target\eval\standard\gpu.csv
python bench\besttake-align.py <库> target\eval\standard [--margin 1.2 1.3]   # 最佳表情全局对齐诊断
# 其余 AI 功能：imagepicker serve --data-dir <副本> 后
python bench\api-features.py --base http://127.0.0.1:7911 --out target\eval\api\out --library <库>\jpeg
```

## AI 运行时与模型

| 项 | 结果 |
|---|---|
| `uv sync --extra cuda --extra mediapipe` | 83 s，57 个包，`.venv` 2.28 GB；onnxruntime-gpu 1.30.0（TensorRT / CUDA / CPU EP），mediapipe 1.0.1，CPython 3.12.15 |
| `system.info` | `tier: T3`（`NVIDIA GeForce RTX 4090 23 GB`），`providers: [CUDAExecutionProvider, CPUExecutionProvider]`，`device: cuda`，VRAM 预算 19343 MB，`features: {mediapipe: true, heif: true}`，`llm_available/vlm_available: false`（`assistant.runtime: false`） |
| 模型目录 | `<worktree>\target\models`（`--models-dir` / `IMAGEPICKER_MODELS_DIR`；默认会是 `%APPDATA%\imagePicker\models`，本账户下原本不存在） |
| standard 模型（`analyze --allow-download` 自动下载，HF 官方源） | yunet 0.2 MB、mediapipe-face-landmarker 3.6、auraface 248.6、siglip2-base-fp16 177.4、siglip2-base-text 571.5、nima-aesthetic 6.2、nima-technical 6.2，合计 **1014 MB**；含下载的首轮比第二轮多约 12 s |
| 修图 / 蒙版 / 最佳表情模型（`POST /api/models/ensure`） | birefnet-lite-fp16 109、skyseg-u2net 168、mediapipe-selfie-multiclass 15.6、mediapipe-pose-landmarker-full 9、lama-big-fp32 198、scunet-color-real-psnr 73.4、gfpgan-v1.4 324.5、realesrgan-x2/x4-fp16 各 34.5，合计 1013.8 MB（registry 966.5 MB），77 s |
| AI 助手 | **默认不安装**：安装器 `decide_extras` 在 NVIDIA 上只选 `cuda + mediapipe`，T3 也一样；需另选 `llm-cuda`（`uv sync --extra cuda --extra mediapipe --extra llm-cuda`：只多 onnxruntime-genai-cuda 0.17.1，8 MB，4 s）+ 模型 phi-4-mini-genai-int4-cuda 3350 MB、phi-3.5-vision-genai-int4-cuda 2450 MB（`models.ensure` 共 6.04 GB，629 s） |
| 未下载 | SDXL 修补包（6.7 GB，> 5 GB 且需 `--extra pro`）、birefnet-fp16 质量包、Qwen3 / CPU 版助手模型 |

## 结果摘要（standard，1000 张）

| 项 | 结果 |
|---|---|
| 吞吐 | 导入 1000 张 1.1–7.9 s；分析 150.8 s（**6.5 张/s**），分组 0.05 s、聚类 0.38 s、评分 0.13 s。设 `OPENCV_FOR_THREADS_NUM=1` 重跑：分析 114.4 s（**8.5 张/s**），结果逐项相同 |
| GPU | 分析期间平均利用率 13.5%（忙碌采样均值 20%，峰值 100%），显存 +3.4 GB，功耗峰值 63–79 W：GPU 远未吃满，瓶颈在 CPU 侧（见「吞吐」） |
| Worker 错误 / 超时 | 0（1000/1000 分析完成，无 skipped step；3 轮 analyze、7 次 serve 均无 worker 崩溃或超时） |
| 星级分布 | 0★ 51 · 1★ 63 · 2★ 158 · 2.5★ 4 · 3★ 424 · 3.5★ 20 · 4★ 232 · 4.5★ 47 · 5★ 1 |
| 问题标签 | 闭眼 50 · 模糊 147 · 噪点 23 · 过曝 11 · 欠曝 11 |
| 闭眼 vs `eyes-closed` | **P 0.94 / R 0.90**（47/3/5） |
| 人物聚类（单人照 392 张主体脸） | 两两 P 0.954 / R 0.943 / **F1 0.948**；纯度 0.980、完整度 0.967；8 位演员 → 9 个簇 |
| 合影中认出的演员 | 1085 人次中 1050（0.97）；20 张脸被认成不在场的演员 |
| 连拍分组 | 60/60 组成员完全正确，两两 P 0.96 / R 1.00（含重复样本时 1.00 / 1.00） |
| 最佳帧 = `best` 标签 | 7/23；最佳帧是 `best` 或 `base` 21/60（多数被笑得更开的 `laugh` 帧抢走） |
| 场景分组 | 两两 P 0.999 / R 0.789（266 个场景 vs 清单 225） |

## 问题标签（`eval-library.py`，新增 `closed_eyes`）

| 标签 | TP | FP | FN | 精确率 | 召回率 | 备注 |
|---|---|---|---|---|---|---|
| 闭眼 | 47 | 3 | 5 | 0.94 | 0.90 | 单人连拍闭眼变体 25/26，合影 22/26 |
| 模糊 | 34 | 113 | 37 | 0.23 | 0.48 | FP：连拍 63（其中 gaze-away 21）、缺陷图的其他类（含 shake）28、风景 10、物品 10 |
| 过曝 | 11 | 0 | 1 | 1.00 | 0.92 | |
| 欠曝 | 7 | 4 | 5 | 0.64 | 0.58 | |
| 噪点 | 11 | 12 | 1 | 0.48 | 0.92 | FP：风景 8（阴天 5） |

- 连拍动态模糊变体 47 张，标「模糊」20 张；相对本组 base 帧清晰度：模糊变体 p50 0.87，其余变体 p50 0.99——组内相对阈值 0.8× 只抓到一半。
- **闭眼漏报 5 张，4 张是同一个原因**：`bg16-karaoke-04`、`bg17-airport-02`、`bg27-snow-family-02`、`bg28-concert-04` 中 eyes_open 已降到 0.23–0.31（< 0.45），但闭眼的那个人站在画面边上，被判成「非主体」（见下「合影成员被降为路人」），而 `closed_eyes` 只看主体脸。第 5 张 `bs01-a1-01` eyes_open 0.48，刚好在阈值上方。
- 闭眼误报 3 张：`bs16-a8-01`、`bg25-museum-group-03`（都是 mid-talk 说话帧，眼睛半闭）、`df08-noise`（噪点图）。
- 视觉复核说 `bg01-family-dinner-02` 闭眼的是两个人：模型给出两张脸 eyes_open < 0.45，与目检一致。

## 人脸与人物

| 项 | 结果 |
|---|---|
| 人脸 | 1688 张（主体 1419），已归入人物 1603，未归属 85 |
| 人物 | 85（多张照片的 18，单张 singleton 67） |
| 单人照（392 张） | 人脸数 = 1 的 379 张，多检 13（背景路人），无漏检 |
| 合影（304 张） | 人脸数 = 人数 284 张，多检 20，无漏检 |
| 单人照主体脸 → 演员 | 两两 P 0.954 / R 0.943 / F1 0.948；纯度 0.980、完整度 0.967；未归属 0 |
| 拆分 / 混簇 | a1 拆成 46 + 5；a8 有 7 张并入 a2 的簇（a2 55 + a8 7）；a6 1 张并入 a4 |
| 合影 | 期望 1085 人次，认出 1050（0.97）；认成不在场的演员 20；同一演员两次 0；未归属 6；归入无单人照的簇 34 |
| 以脸搜脸（a1 / a3 / a8 的演员照） | 首位候选都是本人（相似度 0.74 / 0.84 / 0.70，第二名 0.51 / 0.58 / 0.58）；返回的 10 张相似脸分别 10/10、10/10、9/10 属于本人 |

**合影成员被降为路人（bug，影响闭眼、最佳表情、消除路人）**：304 张「全员都是演员」的合影共 1110 张脸，189 张（17%）`is_subject = 0 / is_bystander = 1`，113 张合影至少一位成员被降级；按位置：画面左右 20% 内的脸 153/313 被降级，中间只有 36/797。原因在 `crates/ip-core/src/analysis/scoring.rs::subject_flags`：权重 = √面积 × 居中度（`1 − 1.2·d`，下限 0.2）× 清晰度 × 看镜头，低于最强脸 45% 即非主体；同样大小、站在边上的家人居中度只有 0.4 左右。后果：
- 闭眼漏报（上面 4 张）；
- `GET /api/photos/{id}/bystanders` 把家人列为路人：`bg01-family-dinner-00` 返回 a1（最左边，脸宽 0.14，与其他三人相当），「消除路人」会把家人抹掉；
- 最佳表情 / 评分里这些人的表情不计入。
建议：脸面积 ≥ 最大脸的 50%（或在人脸大小的中位数附近）时直接算主体，居中度只用于区分明显更小的脸。

## 表情信号（连拍变体 vs 本组 base 帧，均值）

| 变体 | n | eyes_open | smile | gaze | 预期方向命中 |
|---|---|---|---|---|---|
| 单人 eyes-closed | 26 | 0.28 vs 0.80 | 0.35 vs 0.29 | 0.09 vs 0.75 | 24/26（< 0.45 且 base ≥ 0.45） |
| 单人 laugh | 23 | 0.69 vs 0.80 | 0.59 vs 0.32 | 0.48 vs 0.76 | 14/23（smile 高 0.1 以上） |
| 单人 gaze-away | 22 | 0.88 vs 0.82 | 0.19 vs 0.27 | 0.15 vs 0.74 | 21/22（gaze 低 0.1 以上） |
| 单人 mid-talk | 24 | 0.61 vs 0.80 | 0.18 vs 0.29 | 0.74 vs 0.74 | — |
| 合影受害者 eyes-closed | 26 | 0.28 vs 0.80 | | 0.09 vs 0.74 | 25/25（按人物簇找到 `victim:` 演员的脸） |
| 合影受害者 gaze-away | 34 | | | 0.21 vs 0.71 | 32/34 |
| 合影受害者 laugh | 33 | | 0.61 vs 0.38 | | 16/32 |

闭眼与视线两个信号很干净；笑容信号偏弱（laugh 变体 smile 只比 base 高 0.27，近一半未拉开 0.1），而且大笑时 eyes_open 和 gaze 同时下降（眯眼）。

## 连拍、最佳帧与场景

- 连拍分组（时间 + pHash + SigLIP2 嵌入）：60 组全部成员正确，另有 12 组是 edge 重复样本与原图成组（正确）；两两 P 0.959 / R 1.000 / F1 0.979（含重复样本 1.00 / 1.00）。lite 档对照见下。
- 最佳帧：被选为最佳的帧按变体统计——单人 laugh 16、合影 laugh 14、合影 base 8、单人 best 7、单人 base 6、合影 gaze-away 3、合影 mid-talk 3、合影 blur 1、合影 eyes-closed 1、单人 gaze-away 1。`best` 帧的组内名次：第 1 名 7、第 2 名 11、第 3 名 3、第 4 名 2。表情分里笑容权重 0.20（`0.4 + 0.6·smile`），laugh 帧 smile 高出 0.27 就压过了 `best`；而 `best` 帧的标签含义（自然、睁眼、看镜头）与 laugh 帧的区别在合成图里本来就小。合影里有 1 组把 eyes-closed 帧选成了最佳（该帧没有被标闭眼）。
- 场景分组：266 个场景 vs 清单 225；两两 P 0.999 / R 0.789——不会把不同场景并在一起，但同一场景（清单里帧间隔 25 s 的同地点人像）被拆开。
- 场景类型（SigLIP2 零样本）：people 200/200 → portrait；groups 98/120 → group（22 portrait）；合影连拍 139/172 group；places 64 landscape、43 architecture、8 night；things 31 food、28 other、13 pet；文档 9 other。

## 星级与评分

| 类别 | n | 平均星级 | aesthetic | iqa |
|---|---|---|---|---|
| 演员照 | 8 | 3.62 | 0.33 | 0.72 |
| 合影 | 120 | 3.62 | 0.30 | 0.76 |
| 单人照 | 200 | 3.35 | 0.30 | 0.69 |
| 连拍 laugh / base / best | 56 / 60 / 23 | 3.88 / 3.50 / 3.41 | 0.32–0.34 | 0.68–0.73 |
| 连拍 eyes-closed | 52 | **1.18** | 0.31 | 0.72 |
| 缺陷图 | 60 | **0.93** | 0.18 | 0.44 |
| 风景 | 120 | **1.86** | 0.26 | 0.46 |
| 物品 | 100 | 2.65 | 0.28 | 0.50 |

闭眼与缺陷图被正确压到 1★ 左右。**风景偏低**：120 张干净的风景只有 1 张 4★，74 张 2★、26 张 1★；`explain` 显示 landscape 场景的分数主要由 NIMA 美学分决定（`aesthetic` 贡献 −0.10 ~ −0.18），而 NIMA（AVA）给这些合成风景 0.14–0.44。需要按场景重标定美学分（或在 landscape 上降低其权重 / 改用 SigLIP 美学头），并用真实风景照复核。

## lite 档对照（同一库，`--profile lite`，纯 Rust）

| 项 | lite | standard |
|---|---|---|
| 分析耗时 | 18.2 s（54.8 张/s） | 150.8 s（6.5 张/s；`OPENCV_FOR_THREADS_NUM=1` 时 114.4 s） |
| 连拍两两 P / R | 0.96 / 0.97 | 0.96 / **1.00**（嵌入补上 lite 漏掉的帧） |
| 场景数 | 683 | 266 |
| 模糊 P / R（FP） | 0.36 / 0.52（66） | 0.23 / 0.48（113） |
| 闭眼 | 无（0/52） | 0.94 / 0.90 |
| 星级 ≥ 4 | 863 张 | 280 张 |

standard 的模糊误报多出 47 张，集中在连拍的 gaze-away（21）等变体：worker `quality` 步骤的清晰度对转头 / 表情变化更敏感（非模糊变体相对 base 的清晰度 p10 0.87、最低 0.52；lite 为 0.95 / 0.80），组内相对阈值 `BLURRY_IN_BURST_RATIO = 0.80`（`scoring.rs`）就把这些帧标成了模糊。

## 吞吐与 GPU

| 测量 | 张/s | 说明 |
|---|---|---|
| `imagepicker analyze --profile standard`（1000 张混合） | 6.5 / 8.5 | 后者设 `OPENCV_FOR_THREADS_NUM=1`；Core 每批 32 张串行发给 worker |
| `bench_analyze.py jpeg\people`（200 张单人，worker 单独） | 19.3 | 单步：解码 134、pHash 112、质量 67、**faces 30**、**identity 25**、embed 100、aesthetic 73、iqa 78、scene 107；SigLIP2 纯前向 447 |
| `bench_analyze.py jpeg\groups`（120 张合影，worker 单独） | **6.8** | 各步累计线程时间：identity 90.9 s、faces 24.8 s，其余合计约 25 s |

GPU 平均利用率 13.5–15%（峰值 100%），显存只多 3.4 GB：瓶颈不在 GPU。合影里 identity（AuraFace，每张脸约 0.19 s 线程时间）和 faces（MediaPipe 关键点，CPU）占了大头，值得检查 AuraFace 是否按脸逐个推理 / 是否真的在 CUDA EP 上批量跑。`OPENCV_FOR_THREADS_NUM=1`（tilt PR 的修复）在本机不但没有副作用，分析阶段还快了 24%（150.8 → 114.4 s，结果逐项相同；同机有其他任务，数字仅供参考）。本次所有 worker 进程（2 轮 analyze 不带该变量、1 轮带；7 次 serve）都没有出现 `0xc000070a` 崩溃或超时。

## 其余 AI 功能（`imagepicker serve` + HTTP API，`bench/api-features.py`）

数据目录是 standard 目录库的副本（`target\eval\api`、`target\eval\api-bt`），worker 由 serve 按需拉起（`uv run imagepicker-ai serve`，CUDA，T3）。图片与数值证据在 `target\eval\api*\out\`（只在 Windows 工作树里，未入库）。

| 功能 | 结果 | 证据 |
|---|---|---|
| AI 蒙版 subject / sky / person / skin / hair / clothes / background | **可用** | `beach-walk-a1`：覆盖率 subject 0.25、sky 0.67（上 1/3 0.83、下 1/3 0.55）、person 0.25、skin 0.12、hair 0.07（上 0.09 / 下 0.01）、clothes 0.05（上 0 / 下 0.15）、background 0.75（= 1 − subject）；首个蒙版 21.9 s（BiRefNet 加载 + CUDA 初始化），之后 1.0–2.3 s。合影 `bg01-family-dinner-00` 四个人的 person 蒙版两两 IoU ≤ 0.016 |
| 最佳表情（自动） | **可用，但合影几乎合成不了**（见下节） | 合并前（底片 = 组内最佳）：30 组合影 71 个替换，成功 16，`camera_moved` 55；合并后自动选底片：30 组都选到无需替换的帧（0 个替换）；指定闭眼帧为底片：26 组 57 个替换只成功 6（4 组换掉了闭眼的人），`camera_moved` 51；指定视线偏离帧：28 个替换成功 3 |
| 消除路人（`bystanders`） | 修复前**失败**，修复后**可用** | 修复前 `noon-market-a4`：`inpaint.done ok=false "the removal mask is empty"`；修复后：路人框内平均差 12.9 / 框外 0.48，补丁 0.14×0.45（30 s，含 LaMa 首次加载）；`lantern-night-a8` 15.2 / 0.29（0.8 s） |
| 画笔消除（`strokes`） | **可用** | `pl05-noon`：补丁内平均差 11.2、补丁外 0.0，0.6 s |
| 降噪 | **可用** | `df01-noise`：噪声 σ（Immerkaer，2048 预览）6.37 → 1.85，6.7 s |
| 人脸修复 | **可用** | `park-golden-a2`：补丁只在脸框（0.47×0.36），框内平均差 1.55、框外 0；脸部拉普拉斯方差 263 → 230（GFPGAN 对本来就清晰的合成脸是轻微磨平），2.5 s |
| 导出超分 ×2 | **可用** | `park-golden-a1/a2` 1024×1536 → 2048×3072（941 / 744 KB），两张 5.8 s |
| 人像美化（美颜 + 脸型 / 身体形变） | **可用** | `beauty/prepare` 1.8 s（478 点 + 姿态，`has_pose: true`）；standard 档磨皮 60 / 美白 30 / 去瑕疵 / 亮眼 / 瘦脸 40 / 手臂腰 30：脸框内平均差 6.96、框外 0.56；只美颜 4.15 / 0.35（脸部拉普拉斯方差 229 → 173），只形变 4.93 / 0.25 |
| 一键修图（auto adjust） | **可用** | `pl01-overcast` 0.03 s：曝光 −0.38、阴影 +15、去雾 +30 等 |
| AI 助手 · 规则引擎（默认） | **可用** | 「把闭眼的照片都淘汰」→ `filter closed_eyes` + `set_flag −1`，50 张全是闭眼照，执行 0.23 s；按 `assistant.done.undo` 撤销后与执行前逐张一致。「keep 2 per scene and reject the rest」→ `scene_keep_top n=2`，淘汰 496 张，撤销一致 |
| AI 助手 · 本地 LLM（增强包） | 修复前**不可用**（Core 认不出模型），修复后**可用但计划有危险错误** | 见 Bug 2、3。修复后 `/api/assistant/status` = `llm`（phi-4-mini-genai-int4-cuda），首个计划 40 s（加载），之后 0.45 s |
| 照片描述 / 修图建议（VLM） | 修复前 503，修复后**可用** | `describe`：「一个戴着白衬衫的黑发人，站在户外，树木和阳光在背景中。」+ 6 个关键词，20.4 s（含加载）；`suggest`：对比度 +20、阴影 +25，5.6 s |
| 以脸搜脸 | **可用** | 上传 a1 / a3 / a8 演员照：首位候选都是本人；10 张相似脸 10/10、10/10、9/10 是本人；按 `face_id` 搜也正确 |
| XMP 写 / 读往返 | **可用** | 导入时读侧车（3 星、红标、关键词 2 个）；改星 / 淘汰 / 色标 / 关键词后自动写侧车（`xmp:Rating="5"`、`"-1"`、`"2"`，`dc:subject` + `lr:hierarchicalSubject`）；外部把侧车改成 1 星后 `xmp/sync read` → 目录库 1 星（报告 1 个冲突，符合「侧车较新以侧车为准」）。测试用的是复制到 `target\eval\api\out\xmp-src` 的 4 张图，没有写库目录 |
| SDXL 修补 | 未测 | 增强包 6.7 GB 且需 `--extra pro`（> 5 GB，按约定不下载） |

## 最佳表情：合影里全局对齐几乎总是失败

`bench/besttake-align.py` 用 worker 自己的 `besttake.align.align_global`，把每个连拍帧对齐到本组 base 帧：

| | 单人连拍 142 帧 | 合影连拍 142 帧 |
|---|---|---|
| 当前排除区（每张脸 2.6 倍脸宽 × 3.3 倍脸高，向下延伸） | 124 对齐成功 | **52** 对齐成功（82 帧 0 个匹配点） |
| base 帧被排除的面积 | 中位 0.27 | **中位 0.93**，最大 1.00 |
| 只排除脸框 ×1.2 / ×1.3（实验，`--margin 1.2 1.3`） | 132 | **128** |

合影里人脸铺满画面，排除「头发 + 脖子」的大框后背景几乎不剩，ORB 找不到 8 个以上特征点，`align_global` 返回 `camera_moved`（inliers 0）。这不是合成数据特有的：真实的饭桌 / 合照也是脸占满画面。见 Bug 4。

## Bug 与问题清单

**已修复（本分支，带测试）**

1. **消除路人对真实路人全部失败**（`c96f7160`）。复现：`POST /api/photos/704/inpaint {"bystanders":true}`（`noon-market-a4`）→ `inpaint.done {ok:false, reason:"the removal mask is empty"}`；`lantern-night-a8`、`garden-tea-a6`、`forest-trail-a3` 同样（`bench/worker-rpc.py` 直接调 `mask.generate person + person_bbox`：4/4 返回全黑 PNG，`skipped` 为空）。原因：BiRefNet 只抠主角，小的背景人物没有连通域；`crates/ip-core/src/generate.rs::build_removal_mask` 只在「无蒙版 / skipped」时才退回人体框。修复：空蒙版按缺失处理，用 `body_box`；`FakeWorker.empty_person_masks` + 测试 `an_empty_person_mask_falls_back_to_the_body_box`。worker 侧建议另修：`masks` 的 `select_person` 找不到连通域时应报 `skipped` 或改用人体裁切再抠。
2. **装了本地 LLM / VLM，Core 仍认为没有**（`ab2f2c4b`）。复现：`uv sync --extra cuda --extra mediapipe --extra llm-cuda`，`models.ensure` phi-4-mini + phi-3.5-vision；worker `system.info` 报 `llm_available: true`，但 `GET /api/assistant/status` 仍是 `rules`，`engine:"llm"` 静默回退规则，`POST /api/assistant/describe` 返回 503 `this AI worker has no vision-language model`。原因：`crates/ip-core/src/assistant/mod.rs` 按 `task == "llm" / "vlm"` 找模型，而 registry 里是 `llm_plan`、`vlm_suggest`/`vlm_describe`（只有 FakeWorker 用 `llm`/`vlm`，所以测试一直是绿的）。修复：`serves()` 认 `<role>` 与 `<role>_*`，FakeWorker 改用 registry 的任务名，新增测试。附带：明确请求 `engine:"llm"` 但没有 LLM 时仍静默用规则（`llm_calls` 返回的 `Conflict` 不在报错列表里），界面不会告诉用户。

**未修复（需要设计或调参）**

3. **LLM 计划会淘汰整个会话**（危险，可撤销）。复现（修复 2 之后）：`POST /api/assistant/plan {"session_id":1,"message":"reject every blurry photo","engine":"llm","context":{"filter":""}}` → `filter {issues_any:[blurry]}`（147 张）+ `set_flag {selection:"current_filter", flag:-1}`，后者 `affects: 1000`，执行后 1000 张全部淘汰；「把闭眼的照片都淘汰」→ 只有 `set_flag current_filter`，同样 1000 张。规则引擎会写成 `{"query":"issues_any=blurry"}`。怀疑：`crates/ip-core/src/assistant/mod.rs::build_step` / `exec.rs` 把 `current_filter` 解析成请求上下文里的筛选，而不是同一计划里前面的 `filter` 步骤；`ai-worker/imagepicker_ai/llm/prompts.py` 的 few-shot 也应示范显式 query。建议：计划里有 `filter` 步骤时，后续 `current_filter` 用该筛选；破坏性步骤覆盖整个会话且用户没说「全部」时拒绝 LLM 计划、回退规则。确认框会显示「淘汰 1000 张」，撤销也正常。
4. **合影最佳表情几乎都是 `camera_moved`**。复现：`POST /api/bursts/{id}/besttake/auto {"base_photo_id":<闭眼帧>}`，26 组只成 6 个；`python bench/besttake-align.py <库> <data> [--margin 1.2 1.3]` 给出原因（上节）。文件：`ai-worker/imagepicker_ai/besttake/align.py::_exclusion_mask`（半宽 1.3 脸宽、半高 1.5 脸高、向下 1.4 倍）。建议：剩余背景不足（例如 < 30%）时退回只排除脸框，或只排除要替换的那张脸。
5. **合影边上的成员被判为路人**：17% 的合影脸、画面左右 20% 内近一半。文件：`crates/ip-core/src/analysis/scoring.rs::subject_flags`。影响闭眼漏报（4/5）、「消除路人」会抹掉家人（`bg01-family-dinner-00` 的 a1）、最佳表情。建议见「人脸与人物」。
6. **standard 档模糊误报**（P 0.23，113 个 FP，gaze-away 21 个）：组内相对阈值配上 worker 清晰度，把转头帧当成动态模糊。文件：`scoring.rs` `BLURRY_IN_BURST_RATIO` 及其清晰度来源。
7. **风景星级偏低**（120 张干净风景平均 1.86★）：landscape 评分由 NIMA 美学分主导。需按场景重标定。
8. **最佳帧偏向大笑**：60 组连拍里 30 组把 laugh 帧选为最佳，`best` 帧只中 7/23。表情分的笑容项（权重 0.20）需要上限或「自然度」项；同时需要目检确认 `best` 标签。
9. **性能**：GPU 平均利用率 15%，合影上 identity（AuraFace）每张脸约 0.19 s 线程时间，是 worker 单独跑合影（6.8 张/s）的主要开销。建议确认 AuraFace 在 CUDA EP 上按批推理。

## 需要的测试图片

- **真实连拍**：手持、轻微机位移动、背景有人走动；以及「人脸铺满画面」的真实合影连拍——当前 besttake 对齐的失败是否只在合成图上出现，需要它来确认。
- **至少两人在不同帧各有缺陷的合影连拍**：本库每组合影只有一个 `victim`，总有一帧全员都好，自动最佳表情于是什么都不用合成；要考到合成，需要「没有一帧全员都好」的组。
- **真实路人**：不同大小 / 远近 / 部分遮挡的背景人物，以及雕像、海报上的人脸（backlog P1）。
- **有人工评分的风景 / 物品照**，用来重标定美学分与星级；`best` 帧需要目检（laugh 与 best 在合成图里差别很小）。
- **主体运动模糊 vs 转头**的成对样本，用来校准组内模糊阈值。
- 更多身份（8 个演员太少）：相似长相、戴 / 不戴眼镜、儿童、侧脸，用来测聚类的拆分与误并（本次 a8 有 7 张并进了 a2）。
- 大面积消除（> 512 px 的洞）样本，配合 SDXL 增强包测试。

## 产物位置（Windows 工作树，不入库）

- 分析目录：`target\eval\standard-dl`（含下载的首轮）、`target\eval\standard`、`target\eval\standard-cv1`（`OPENCV_FOR_THREADS_NUM=1`）、`target\eval\lite`；各目录里有 `analyze.stdout.log`、`gpu.csv`、`eval-library.txt/json`、`eval-standard.txt/json`、`faces.csv`。
- worker 吞吐：`target\eval\standard\bench-analyze-{people,groups}.txt/json`；对齐诊断：`target\eval\standard\besttake-align{,-tight}.txt/json`。
- API 端到端：`target\eval\api\api-*.jsonl`、`target\eval\api-bt\api-run3/4.jsonl`，图片在 `target\eval\api\out\`、`target\eval\api-bt\out\`（蒙版、前后对比、补丁、导出、XMP 侧车）。
- 模型：`target\models`（standard 1.01 GB + 修图 1.01 GB + 助手 6.04 GB）；worker venv：`ai-worker\.venv`（加 `llm-cuda` 后 2.30 GB）。
