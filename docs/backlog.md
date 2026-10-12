# 待办与已知问题（Backlog）

> 每轮集成后更新。按优先级：**P0** 发布前必须修复，**P1** 1.0 前完成，**P2** 之后。
> 2026-10-09 的完整评审（各需求状态、文档矛盾、本表未列的缺口、验收标准与并行工作包）见 [09 完成度评审与收尾计划](09-完成度评审与收尾计划.md)。

## 校准（需要真实照片评测集）

> 合成评测图库：2026-10-08/09 起使用 1000 张图库 `bench/data/qwen-photo-library`（ComfyUI + Qwen-Image 2.1，`scripts/comfy-photo-library.py`（脚本尚未提交），图片不入库；`manifest.json` 标签是提示词意图，需人工核验），评测脚本 `bench/eval-library.py`。连拍变体改由 Qwen 原生编辑从底片派生，lite 档连拍分组 pairwise 精确率 0.96 / 召回 0.97（63a8ffe3）。2026-10-07 的 74 张首轮记录 `bench/testset-lite-2026-10-07.md` 仅作历史参考（其中「连拍 pHash 距离过大需 img2img 派生」已解决）。

| 优先级 | 项 | 说明 |
|---|---|---|
| P0 | 评分阈值校准（真实照片复核） | 2026-10-09 已在 1000 张合成图库（`bench/data/qwen-photo-library`，`bench/eval-library.py`）上校准问题标签：过曝/欠曝要求细节丢失（白底人像、霓虹夜景不再误报），噪点阈值 0.60→0.22，连拍内清晰度 < 0.8× 最清晰帧标「模糊」。lite 档位：过曝精确率 0.52→0.92，欠曝 0.56→0.77，噪点召回 0→0.83，模糊精确率 0.28→0.37 / 召回 0.35→0.52。仍待：真实照片复核；背景虚化人像的模糊误报（需人脸区域清晰度）；美学/IQA 换算、分组松紧、星级映射 |
| P1 | 倾斜检测阈值（真实照片复核） | 2026-10-09 实现（03 §3.1.1，所有档位同一算法）。只在合成图库的旋转集上标定：置信度 ≥ 0.2 且 2°–10° 时精确率 0.91，但召回低（\|角\| ≥ 3° 时 0.14，有直线的照片 0.21）；原图库 2.4% 被标（部分生成图本身就斜）。仍待：真实照片复核；仰拍建筑（竖线会聚）、车窗/桌面等单侧透视的误报；广角畸变的弯线不计入 |
| P1 | 自适应同步参数 | `edit/sync.rs` 的 `K_PER_LN`、`T_PER_LN` 在 FakeRenderer 上调参，需在真实渲染器上复核 |
| P1 | 美颜强度 | 形变/美颜强度在合成人像上标定，真实人脸效果需人工盲评 |

## 功能缺口

| 优先级 | 项 | 来源 |
|---|---|---|
| P1 | HEIC/HEIF 解码路径的实机验证：本机只验证了 libheif（AI 组件自带）与内嵌 JPEG 两层；Windows WIC + HEIF/HEVC 扩展、macOS ImageIO 未实测；真实 iPhone / 安卓样张（网格、HDR、`imir`）与 AVIF 未测 | M1 imaging |
| P1 | RAW+JPEG 配对（09 P0-7）已实现并在合成文件上测试；仍待：真实相机的 RAW+JPEG 样张（Canon CR3、Sony ARW、Nikon NEF、Fuji RAF）验证；配对只认同目录同主文件名，不认相机的 `_DSC1234` / `DSC1234` 变体命名；JPEG 在别处被删除后 RAW 仍作为它的 companion（显示为缺失，不会自动升格为网格项）；多个 JPEG 与同一 RAW 同名时 RAW 只跟第一张 | P0-7 |
| P1 | 文件监听（09 P1-6）：整个根目录变化后全量重新扫描（10 万张级别的根目录每次扫描会有秒级 IO），网络盘 / NAS 上的变更通知不一定可靠（需手动「重新扫描」）；安卓与 iOS 不监听；回收站只在桌面版（Android 无回收站，仅 `_rejected` 文件夹） | P1-6 |
| P2 | Android 端 HEIC 缩略图：Rust 侧只有内嵌 JPEG，需经媒体插件接入 `ImageDecoder` / `loadThumbnail` | M8 |
| P1 | 构图分（倾斜检测已实现，阈值复核见「校准」） | M2 |
| P1 | RAW 预览在真实相机文件上验证（目前仅合成样本） | M1 imaging |
| P2 | HEIC/HEIF 色彩余项：HDR（`nclx` 传递函数 PQ / HLG、iPhone 增益图）不做色调映射，按 SDR 原样显示；WIC 路径的色彩转换未实测（本机无 HEIF 编解码器） | M5 core |
| P2 | 导出元数据余项：DNG/CR3/HEIC 文件内部的 XMP、PNG/WebP/TIFF 内嵌 XMP/IPTC 不复制（只用 JPEG 内嵌包或 `.xmp` 侧车）；扩展 XMP（>64 KB）丢弃；导出不写入目录库的评分/关键词；RAW 预览色彩空间只看预览 JPEG 自带 ICC / DCF 标记，RAW EXIF 重建仅在合成样本上验证 | M5 core |
| P2 | 导出格式余项：WebP 只有无损（纯 Rust 编码器无有损模式，文件较大）；TIFF 为 8 位无压缩，无 16 位输出；PNG/WebP 不写 IPTC 块（只在 XMP 中）；TIFF 导出不带 MakerNote；无「打印 300dpi」预设与用户自定义导出预设 | P0-8 |
| P2 | 色彩转换前生成的缩略图/预览缓存不会自动重建（清除缓存后重新生成）；导出只输出 sRGB（无 P3 输出），色域外颜色裁切 | M5 core |
| P2 | 最佳表情选底片的系数（`MIN_GAIN` 0.04、残留 ×6、缺席 ×3、切换余量 0.25）只在合成矩阵上验证，需真实连拍复核；不估计相机移动；手动换底片时网页仍用自动底片的计划（可合成性按自动底片的姿态判断、原自动底片不在候选中），可改用 `?base_photo_id=` 重新取计划 | M5 |
| P1 | 大面积消除（超过 512px 的洞）结果偏糊、有残影；默认 LaMa，需 SDXL 增强包实测（真实照片：去除雕像） | M5 worker |
| P1 | 雕像/海报上的人脸会被当作「路人」；界面需确认后才消除，但检测应区分真人（活体/语义） | M5 |
| P2 | SDXL 增强包未用真实权重测试；OpenCV 构建无 AKAZE，仅 ORB 对齐 | M5 worker |
| P2 | 取消生成任务时 worker 立即以 `-32800` 结束请求，但 `inpaint.run` / `enhance.run` / `besttake.compose` 在线程池里的计算不会中断，跑完才释放（紧接着的生成请求会排在它后面）；需在推理循环里检查取消标志 | M5 worker |
| P1 | 网格 60fps 需在真实 GPU 浏览器上复测（无头软件光栅下仅 4× 降频 CPU 快速滚动时掉帧） | M7 perf |
| P2 | 单会话 10 万张时每页约 10ms、`-taken_at` 约 28ms（需会话排序键表）；`GET /api/sessions` 10 万行约 13ms（需计数缓存） | M7 perf |
| P1 | macOS / Linux 安装后的 AI 组件安装流程未实测；国内镜像对 `uv sync --frozen` 可能无效 | M7 runtime |
| P1 | 应用内自动更新（tauri-plugin-updater + 签名密钥） | M7 release |
| P2 | AUR PKGBUILD 未在 Arch 上 makepkg 实测 | M7 release |
| P2 | 身体形变后的生成式背景补全 | M4 render |
| P2 | 皮肤蒙版回退方案未扣除眉毛 | M4 render |
| P1 | `ip-infer`（`infer` 特性）尚未在 Android 上链接 ONNX Runtime（NNAPI/XNNPACK）；端侧 YuNet 目前仅桌面可用，检测器为单实例互斥（分析线程间串行） | M8.1 infer |
| P1 | Release 的 Android APK 任务（签名 keystore、`apksigner` 校验、产物路径）未在 CI 实跑；需配置 `ANDROID_KEYSTORE_*` 四个 secrets 后验证 | M8 release |
| P2 | lite 档位的 `faces` 步骤不输出关键点 / 闭眼 / 表情（需 MediaPipe 或 ONNX 关键点模型） | M8.1 infer |

## 助手

| 优先级 | 项 | 说明 |
|---|---|---|
| P1 | 助手 20 条指令基准（docs/09 M6）的 LLM 计划实测 | 目标绑定、`fallback` 与大范围修改确认（api-contract-m6 A.4）已有单元/接口测试，但未用真实本地模型重跑 20 条基准；前端尚未展示 `criteria` / `confirm_text` / `fallback` |

## 许可审计（商用前）

- NIMA 美学/技术质量模型：权重 Apache-2.0，训练数据 AVA / TID2013 有研究用途条款。
- 天空分割 `skyseg-u2net`：训练数据未公开。
- GFPGAN：FFHQ 训练数据有独立条款；Real-ESRGAN ONNX 来自 facefusion 镜像（上游 BSD-3）。

## 已在集成中修复（记录）

- 目录与文件（09 §5.1 P0-7、§5.2 P1-6，WP-F1）：①同目录同主文件名的 RAW+JPEG 导入后是一个网格项（JPEG 显示 / 分析，RAW 为 companion，`pair_id`），评分 / 旗标 / 色标同步到两者及各自的 XMP 侧车，分组与分析只算一次，导出可选 `pair_member`；②`POST /api/import` 支持 `exclude`（通配符 / 子文件夹 / 名字，重新扫描沿用）；③桌面端文件监听，增删文件约 2–3 秒内进网格；④缺失文件显示断链，`POST /api/sessions/{id}/relocate` 按相对路径 + 大小重新定位文件夹；⑤已淘汰照片经确认后可移入回收站（`trash`）或 `_rejected`，目录库跟随，绝不硬删（`tests_files.rs`、`ip-server/tests/files.rs`）

- 数据安全 P0（09 §5.1 P0-1 / P0-3 / P0-4）：①原图只读成为硬约束——「侧车 + 内嵌」改为需二次确认的高级模式「修改原图（写入内嵌 XMP）」（`xmp_mode = modify_originals`，服务端要求 `confirm_modify_originals`，旧值降为 `sidecar`），其余模式下导入、分析、评分 / XMP 同步、编辑、导出后原图 SHA-256 不变（`tests_safety.rs`）；②目录库启动后后台 `quick_check`、每日 `VACUUM INTO` 备份留 7 份、打不开时移到 `backups/corrupt-*.db` 并以空目录库启动、可从备份恢复（`GET/POST /api/catalog*`，界面在启动弹窗与「设置 → 目录库备份」）；③人脸识别首次说明与同意（`faces.consented`），同意前不检测人脸、不计算人脸特征（lite 档同样），拒绝即 `faces.enabled = false`。同意文案待产品确认（09 §5.4 / P0-4）
- 拍摄时间按查看者时区显示 → 改为拍摄地当地时间（M1）
- HEIC/HEIF 缩略图：内嵌 JPEG → WIC / ImageIO → 运行时加载 libheif（含 AI 组件自带）分层降级，方向以 `irot`/`imir` 为准（M1，见 02 §5.1）
- HEIC/HEIF 色彩与导出：libheif / WIC 输出按主图 `colr`（ICC 或 `nclx` P3 / BT.2020）转换为 sRGB，ImageIO 输出本就是 sRGB；有解码器时 HEIC 可预览、修图与导出（M5 core）
- EXIF 浮点噪声、AI 一键数值噪声（M1、M3）
- 网格场景标题 key 重复导致重叠（M2）
- 前端写死 LUT id → `GET /api/luts`（M3）
- 内置 LUT 两套来源 → 统一由 ip-render 提供（M3）
- M7：冷启动 500→32ms、翻页 p95 188→15ms、批量改星 20k 1213→122ms；三平台安装包 CI 试运行成功，Windows 安装包实机验证（启动、鉴权、卸载）
- Loupe「100%」不是原始像素（P0-6）→ 放大超过 4096 预览所含像素时按需加载全分辨率图 `GET /api/full/{id}`（与预览同一解码路径：转正、sRGB；已编辑的照片为全尺寸渲染；`cache/full` 按最近使用裁剪到 1 GB）；100% 改为一个图像像素对应一个屏幕物理像素并对齐物理像素；「适应」与翻页仍只用预读的 2048 预览。RAW 的 100% 是内嵌 JPEG 的分辨率（不做 RAW 解马赛克）（WP-F2）
- 对比只有 2/4 宫格且格子总取光标之后的照片（P1-7）→ 2/3/4 宫格（3 宫格并排一行）；网格里选中两张及以上时对比格取所选照片（按网格顺序，多于格数时 `←` `→` 在所选中翻页），否则维持「A + 光标起的后续照片」（WP-F2）
- 导出色彩：带非 sRGB ICC（Display P3、Adobe RGB…）或 DCF Adobe RGB 标记的来源在解码时转换为 sRGB（moxcms，纯 Rust），缩略图、预览、渲染与导出一致；导出保留 XMP（去掉朝向、尺寸、冲印参数、人脸区域、动态照片/景深/增益图引用，`strip_gps` 时去掉所有 `GPS*`）与 IPTC（APP13 只留 IPTC 记录）；RAW/TIFF/CR3 导出复制原始 EXIF（重建 IFD0/Exif/GPS，不含 MakerNote），RAF/HEIF 原样复制（M5 core）
- 以脸搜脸「这是新的人」：结果里展示相似人脸（含当前所属人物）供勾选，`POST /api/people` 把它们（以库内人脸搜索时含查询脸）移到新人物并锁定，重新聚类/分析保持；人脸菜单新增「查找相似的脸」（M4 web）
- 一键全员最佳不再总以组内最佳为底片：逐帧估算需替换的脸（按脸尺寸加权）、替换后仍低于本人最佳的表情、缺席人物，排除模糊/曝光问题帧，组内最佳只在别的帧明显更省事时让位；计划返回 `base_choice` 说明原因（编辑器底片按钮悬停可见），可用 `base_photo_id` 手动指定底片；表情分提升不足 0.04 的脸不再替换（M5 core）
- P0-8 导出（已完成）：可选 JPEG / PNG / TIFF / WebP，各格式携带的 EXIF/XMP/IPTC/ICC 见 `api-contract-m5.md` F 节；对话框有「去除 GPS 位置」「去除全部元数据」，对原样复制的原图同样生效（JPEG/PNG/WebP 按段/块清理，图像数据不变；RAW/HEIF/TIFF 改为全尺寸重新编码）；导出预设只有服务端一张表（`GET /api/export/presets`，对话框与 AI 助手共用，含 3:4、4:5 裁切）；命名模板支持 `{event}` `{scene}` `{camera}` `{rating}`；导出失败时只删除本次创建的文件（规划文件名后才出现的同名文件不再被删除，也从不覆盖）
- 生成任务（最佳表情 / 消除 / 增强）、导出与分析可按 id 取消（`POST /api/tasks/{id}/cancel`，取消传到 worker 的进行中请求，取消后不提交补丁）并记录在 `task` 表（`GET /api/tasks`，保留 200 条，重启时把残留的进行中任务记为「已中断」）；界面有取消按钮和「设置 → 任务记录」（M5，见 api-contract-m5 G）
- 导出保留 EXIF + 嵌入 sRGB ICC；Worker 调用超时（504）+ 自动重启；删除会话清理编辑缓存；部分几何在模型安装后刷新；单张人物可被选中；API 浮点噪声统一清理（M5）
