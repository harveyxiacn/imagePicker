# 1000 张生成图库实测（2026-10-14）

## 结论

主流程已经可用：1000 张导入、分析、浏览、筛选、编辑和批量导出均跑通；实际模型的人脸、蒙版、最佳表情、修复、美化、助手与远程分析也完成了专项测试。**不能认定所有模块达到发布质量**：助手存在错误工具选择与超时，模糊识别误报较多，消除有残影，部分换脸被对齐保护拒绝。此前发现的真实 HEIC 镜像方向错误仍未修复。

这是生成图库的功能与回归验证，不替代真实相机照片、Android 真机、安装包及所有参数组合的验收。未重新计算全部 PRD 的完成百分比。

## 数据、环境与安全

- 数据：`bench/data/qwen-photo-library`，manifest 中 1000 张 JPEG，覆盖 actors、bursts、defects、edge、groups、people、places、things。
- 使用独立照片副本及独立目录库，所有变更、移除、XMP、恢复和权限测试均在 `bench/data/verification-2026-10-14` 中进行。没有执行图库清理。
- 测试结束逐张 SHA256 校验：原始 1000 张与测试副本 1000 张均无内容变化或缺失，见 `integrity.json`。
- Windows，i7-13700K、64 GB RAM、RTX 4090 24 GB；源码基线 `main a62ad789` 加本轮已有修复。服务端为本次构建的 release binary。
- 真实权重：SigLIP2、NIMA、YuNet、MediaPipe、AuraFace、分割/修复模型及 Phi 文本/视觉模型。测试时设置 `UV_NO_SYNC=1`，避免默认 worker 启动移除 CUDA/LLM extras。
- 下文证据相对路径均位于 `bench/data/verification-2026-10-14/`。原始失败日志保留；测试脚本错误与产品问题分开说明。

## 功能覆盖

| 模块 | 本次实际执行与结果 | 证据 |
|---|---|---|
| 导入与目录浏览 | 三个独立目录库各导入/分析 1000 张；分页及不同排序无重复、无遗漏 | `*-analysis.log`、`api/modules.jsonl` |
| 缩略图/预览/原图 | 1000 张缩略图逐张下载并解码；8 类样本分别读取预览/完整图/原图，共 24 次 | `api/modules.jsonl` |
| lite/fast/standard 分析 | 三档均完成 1000/1000；standard 1688 张人脸、18 个默认可见人物、266 场景、692 堆栈，其中 72 个多图堆栈 | `quality-*.txt`、`quality-standard.json` |
| 分析生命周期 | 32/1000 时暂停，重启仍暂停，续跑至 1000；32/1000 时强杀服务，重启自动恢复至 1000；取消任务状态正确；已有图库增量计划为 0 | `lifecycle.json` |
| 评分/旗标/颜色/标签/筛选 | 60 张修改和过滤核对；标签添加移除；1000 张采纳 AI 评分成功 | `api/modules.jsonl`、`additional.json` |
| 收藏/预设 | 自定义集合、预设创建读取删除，内置集合保护 | `api/modules.jsonl` |
| 连拍管理 | 拆分再合并后成员集合一致 | `additional.json` |
| 人物管理 | 改名、隐藏、创建人物、“不是此人”、重新指定、合并、最佳照片列表、人脸裁图均通过 | `additional.json` |
| 以脸搜图 | 3 位参考人物 Top-1 均正确；同人物前 10 张分别为 10/10、10/10、9/10；face_id 查询也通过 | `ai-features.jsonl` |
| 基础编辑 | 8 类样本各执行全局、裁切旋转、局部径向、锐化，共 32 次实际渲染；3 张自适应同步；自动调整可返回参数 | `api/modules.jsonl`、`ai-features.jsonl` |
| LUT | 导入自建 .cube 并渲染，验证像素发生变化 | `additional.json`、`lut-inverted.jpg` |
| AI 蒙版 | subject/sky/person/skin/hair/clothes/background 七类生成 PNG；合照四人蒙版分离 | `ai-features.jsonl`、`ai/` |
| 最佳表情 | 闭眼底片 57 次候选替换成功 50 次，26 组中 23 组替换闭眼对象；侧看底片 28 次成功 22 次，存在拒绝及接缝警告 | `ai-features.jsonl`、`ai/besttake_fix_*` |
| 路人/画笔消除 | 市场、灯笼、手绘蒙版三个实际输出；接口和像素修改通过，视觉质量未通过全面验收 | `ai/inpaint_*` |
| 降噪/人脸修复/美化 | 实际推理和渲染成功；噪声估计 6.37 → 1.85；皮肤与形变分别检查有输出 | `ai-features.jsonl`、`ai/` |
| 导出 | 1000 张 JPEG；PNG/WebP/TIFF 各 8 张，均重新解码和检查尺寸；另 2 张 2 倍超分辨率导出至 2048×3072 | `export-verification/modules.jsonl`、`ai-features.jsonl` |
| 文件监听/缺失/重定位 | 独立 6 张夹具：排除导入、新增自动发现、目录改名后缺失标记、重定位预览/应用、淘汰确认及移入 `_rejected` | `api/modules.jsonl` |
| 备份恢复 | 备份后修改评分，恢复后值还原；人物与分组专项也在备份后运行并恢复 | `api/modules.jsonl`、`additional.py` |
| XMP | 独立 4 张：导入读取、评分/颜色/关键词写侧车、外部修改后读回，通过 | `ai-features.jsonl` |
| 助手/撤销 | 中英文 4 条执行后撤销，评分/旗标/颜色恢复；另 20 条指令分别测试 rules/llm，发现语义错误，详见下文 | `assistant.jsonl`、`assistant-20.json` |
| 图片描述/建议 | 真实视觉模型返回与人像内容相符的描述及对比度/阴影建议 | `assistant.jsonl` |
| 访客/设备权限 | 模拟非本机来源匿名 401；访客可读图库、写操作及人物接口 403；设备不能读目录库；撤销后设备 401 | `auth-remote.json` |
| 远程 AI | 配对、上传 8 类实际照片，standard 8 个推理步骤返回 8 项，无 item error、无 skipped_steps | `remote-analysis.json` |
| 页面/响应式 | 首页、图库、人物、编辑、最佳表情、偏好、设置 × 桌面/手机尺寸，共 14 个页面检查通过，无页面异常/5xx/横向溢出；不是所有按钮的逐项 UI 验收 | `browser/results.json`、`browser/*.png` |
| 偏好学习 | 评分事件计数及页面可读；当前 active=false、pairs=0，仅确认采集/展示，未证明学习模型已生效 | `api/modules.jsonl` |

三档耗时：lite release 11.27 秒（88.7 张/秒）；fast release 约 63 秒（15.9 张/秒）；standard debug 106.96 秒（9.3 张/秒）。standard 同时有 release 编译活动；构建模式和负载不同，**不能作为三档公平性能对比**。

## 算法效果与问题

以下标签来自生成 manifest 的意图，未经逐张人工真值复核；数字用于定位回归和校准，不应当作真实照片准确率。

| 指标 | standard 结果 |
|---|---|
| 闭眼 | TP 51 / FP 7 / FN 1；精确率 87.9%，召回率 98.1% |
| 模糊 | TP 34 / FP 113 / FN 37；精确率 23.1%，召回率 47.9%，误报明显 |
| 过曝 / 欠曝 / 噪声 | 精确率/召回率分别约 100%/91.7%、63.6%/58.3%、47.8%/91.7% |
| 连拍 | 60/60 标注组完全匹配；成对精确率 95.9%，召回率 100%；另有额外成组照片 |
| 场景 | 成对精确率 99.95%，召回率 78.9%；标注 225 场景，实际分为 266，有过度拆分 |
| 单人人物聚类 | 392 张人脸成对 F1 94.8%，纯度 98.0%；仍有 a2/a8、a4/a6 混簇及个别人被拆簇 |
| 合照身份 | 标注预期 1085，找到 1050，wrong=20；不是无误识别 |
| 最佳单帧 | 标注 strict best 的 23 组中 Top-1 命中 7 组；另 12 组排第二。审美/笑容偏好需人工复核 |

需要优先处理：

1. **助手语义执行风险。** 中英文“应用暖调胶片预设”都生成 `apply_profiles`（美颜）计划。中文“增加 0.5 曝光”生成 `auto_adjust(mode=auto)`，回复却称调整 0.5，计划与用户要求不符。以上只生成计划，未执行错误计划。应对工具和参数做语义验证，不能将 HTTP 成功当作指令正确。
2. **助手稳定性。** 20 条 rules 指令中 17 条匹配预期工具；20 条 llm 中 14 条匹配（包括回退结果），2 次失败为客户端 120 秒超时及 HTTP 504。服务日志有 `models.list` 60 秒超时并重启 worker；后续请求恢复。4 次回退中包含 3 次 `llm_unsafe_target`，安全约束有效，但不等同于 LLM 自身正确。工具名匹配只是一层检查，不是完整语义通过率。规则不支持英文 “pick the selected photos” 及精确曝光调整。
3. **模糊评分与选择质量。** 本库 FP 113，多于 TP 34；场景过度拆分、最佳帧排序也需复核。倾斜标签此次为 TP 0 / FP 24 / FN 3，但原库不是专门旋转校准集，不能替代已有倾斜专项评估。
4. **最佳表情并非全部可合成。** 闭眼场景 7 次 `camera_moved` 拒绝，5 次 seam 警告；侧看 4 次 `camera_moved`、2 次 `face_not_aligned` 拒绝，另有接缝/姿态警告。这些保护性拒绝不等同于进程崩溃，但用户仍会遇到无法修复的照片。
5. **消除质量。** 人像 `noon-market-a4.jpg` 原图/编辑图人工对照：右侧路人的脸被移除，但身体与模糊区域仍然可见。API 成功和 patch 外像素不变不能证明消除自然。

测试记录解释：`api/modules.jsonl` 最初 exports 失败是脚本误读 task 的 `state`，接口实际字段为 `status`；修正后完整重跑四种格式通过。`ai-features.jsonl` 的 `besttake`/`besttake_all` 报 fails，是旧脚本要求必须产生 patch；自动选底片时 30 组/108 人均返回 0 个替换选择，这条路径没有覆盖合成，不据此判定合成失效。随后用闭眼/侧看底片显式测试合成，结果如上。人物“不是此人”按契约新建单脸人物，不返回空 person_id；修正断言后人物操作全部通过。

## 仍未验收的范围

- 1000 张全部是 JPEG，不能验收真实 RAW、HEIC/HDR/ICC 全部路径；此前 HEIC 镜像方向失败仍有效，见 [基础检查](verification-2026-10-14.md)。
- SDXL 增强消除包未安装、未推理；本次验证默认修复路径。
- Android 系统相册/权限、真正跨设备网络、设备端接收远程产物、安装/更新程序、macOS/Linux 未实测。远程 API 测试不是 Android 真机验收。
- 偏好模型训练生效与泛化、全部美颜强度/蒙版边界、多语言可访问性、长时间运行和大于 1000 张规模未覆盖。
- 自然语言搜索和 SD 卡导入入口仍禁用，不属于已经正常可用的模块。

## 可复跑入口

使用独立目录库启动服务，并把测试输出根加入允许访问的 roots。不要把会评分、写侧车和操作文件的测试指向生产图库。

```powershell
$env:UV_NO_SYNC = '1'
# --out 应为允许访问的 verification-* 目录；服务已有分析完成的 1000 张 session。
ai-worker/.venv/Scripts/python.exe bench/verify-library-modules.py --base http://127.0.0.1:17914 --out bench/data/verification-2026-10-14/api --session 1
# AI 专项的 --only 列表及真实证据保存在 ai-features-run.log / assistant-run.log。
# 浏览器脚本读取 BASE_URL / OUT_DIR，须从 web 工作目录执行。
cd web
node scripts/e2e-library.mjs
```

文件操作夹具输出需要全新目录；重复运行要换 `--out`，避免已有移动目标或旧导出文件影响断言。本轮新增脚本通过 Ruff F/E9、Node 语法及 ESLint 检查，沿用基础检查中已通过的单元测试结果，没有把历史单元测试冒充此次重新执行。
