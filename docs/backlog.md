# 待办与已知问题（Backlog）

> 每轮集成后更新。按优先级：**P0** 发布前必须修复，**P1** 1.0 前完成，**P2** 之后。

## 校准（需要真实照片评测集）

> 2026-10-07：已用本地 ComfyUI（Qwen-Image 2.1）生成 74 张合成评测图（人像 / 风景 / 合照 / 连拍变体：闭眼、动态模糊、过曝欠曝、路人 / 缺陷样本），脚本 `scripts/comfy-testset.py`，输出在主机 `~/ai/ComfyUI/output/imagepicker-testset/`（带 `manifest.json` 标签），不入库。lite 档位首轮结果与结论见 `bench/testset-lite-2026-10-07.md`（白底 / 雪地过曝误报、低对比判模糊、局部动态模糊漏报、合成连拍 pHash 距离过大需 img2img 派生）。

| 优先级 | 项 | 说明 |
|---|---|---|
| P0 | 评分阈值校准（真实照片复核） | 2026-10-09 已在 1000 张合成图库（`bench/data/qwen-photo-library`，`bench/eval-library.py`）上校准问题标签：过曝/欠曝要求细节丢失（白底人像、霓虹夜景不再误报），噪点阈值 0.60→0.22，连拍内清晰度 < 0.8× 最清晰帧标「模糊」。lite 档位：过曝精确率 0.52→0.92，欠曝 0.56→0.77，噪点召回 0→0.83，模糊精确率 0.28→0.37 / 召回 0.35→0.52。仍待：真实照片复核；背景虚化人像的模糊误报（需人脸区域清晰度）；美学/IQA 换算、分组松紧、星级映射 |
| P1 | 自适应同步参数 | `edit/sync.rs` 的 `K_PER_LN`、`T_PER_LN` 在 FakeRenderer 上调参，需在真实渲染器上复核 |
| P1 | 美颜强度 | 形变/美颜强度在合成人像上标定，真实人脸效果需人工盲评 |

## 功能缺口

| 优先级 | 项 | 来源 |
|---|---|---|
| P1 | HEIC/HEIF 解码路径的实机验证：本机只验证了 libheif（AI 组件自带）与内嵌 JPEG 两层；Windows WIC + HEIF/HEVC 扩展、macOS ImageIO 未实测；真实 iPhone / 安卓样张（网格、HDR、`imir`）与 AVIF 未测 | M1 imaging |
| P2 | Android 端 HEIC 缩略图：Rust 侧只有内嵌 JPEG，需经媒体插件接入 `ImageDecoder` / `loadThumbnail` | M8 |
| P1 | 倾斜检测、构图分 | M2 |
| P1 | RAW 预览在真实相机文件上验证（目前仅合成样本） | M1 imaging |
| P2 | 以脸搜脸「这是新的人」 | M4 web |
| P2 | HEIC/HEIF 色彩余项：HDR（`nclx` 传递函数 PQ / HLG、iPhone 增益图）不做色调映射，按 SDR 原样显示；WIC 路径的色彩转换未实测（本机无 HEIF 编解码器） | M5 core |
| P2 | 导出元数据余项：DNG/CR3/HEIC 文件内部的 XMP、PNG/WebP/TIFF 内嵌 XMP/IPTC 不复制（只用 JPEG 内嵌包或 `.xmp` 侧车）；扩展 XMP（>64 KB）丢弃；导出不写入目录库的评分/关键词；RAW 预览色彩空间只看预览 JPEG 自带 ICC / DCF 标记，RAW EXIF 重建仅在合成样本上验证；原样复制（未编辑、原尺寸）时 `strip_gps` 不生效 | M5 core |
| P2 | 色彩转换前生成的缩略图/预览缓存不会自动重建（清除缓存后重新生成）；导出只输出 sRGB（无 P3 输出），色域外颜色裁切 | M5 core |
| P2 | 生成任务没有取消接口、没有持久化任务记录 | M5 |
| P2 | 最佳表情选底片的系数（`MIN_GAIN` 0.04、残留 ×6、缺席 ×3、切换余量 0.25）只在合成矩阵上验证，需真实连拍复核；不估计相机移动；手动换底片时网页仍用自动底片的计划（可合成性按自动底片的姿态判断、原自动底片不在候选中），可改用 `?base_photo_id=` 重新取计划 | M5 |
| P1 | 大面积消除（超过 512px 的洞）结果偏糊、有残影；默认 LaMa，需 SDXL 增强包实测（真实照片：去除雕像） | M5 worker |
| P1 | 雕像/海报上的人脸会被当作「路人」；界面需确认后才消除，但检测应区分真人（活体/语义） | M5 |
| P2 | SDXL 增强包未用真实权重测试；OpenCV 构建无 AKAZE，仅 ORB 对齐 | M5 worker |
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

## 许可审计（商用前）

- NIMA 美学/技术质量模型：权重 Apache-2.0，训练数据 AVA / TID2013 有研究用途条款。
- 天空分割 `skyseg-u2net`：训练数据未公开。
- GFPGAN：FFHQ 训练数据有独立条款；Real-ESRGAN ONNX 来自 facefusion 镜像（上游 BSD-3）。

## 已在集成中修复（记录）

- 数据安全 P0（09 §5.1 P0-1 / P0-3 / P0-4）：①原图只读成为硬约束——「侧车 + 内嵌」改为需二次确认的高级模式「修改原图（写入内嵌 XMP）」（`xmp_mode = modify_originals`，服务端要求 `confirm_modify_originals`，旧值降为 `sidecar`），其余模式下导入、分析、评分 / XMP 同步、编辑、导出后原图 SHA-256 不变（`tests_safety.rs`）；②目录库启动后后台 `quick_check`、每日 `VACUUM INTO` 备份留 7 份、打不开时移到 `backups/corrupt-*.db` 并以空目录库启动、可从备份恢复（`GET/POST /api/catalog*`，界面在启动弹窗与「设置 → 目录库备份」）；③人脸识别首次说明与同意（`faces.consented`），同意前不检测人脸、不计算人脸特征（lite 档同样），拒绝即 `faces.enabled = false`。同意文案待产品确认（09 §5.4 / P0-4）
- 拍摄时间按查看者时区显示 → 改为拍摄地当地时间（M1）
- HEIC/HEIF 缩略图：内嵌 JPEG → WIC / ImageIO → 运行时加载 libheif（含 AI 组件自带）分层降级，方向以 `irot`/`imir` 为准（M1，见 02 §5.1）
- HEIC/HEIF 色彩与导出：libheif / WIC 输出按主图 `colr`（ICC 或 `nclx` P3 / BT.2020）转换为 sRGB，ImageIO 输出本就是 sRGB；有解码器时 HEIC 可预览、修图与导出（M5 core）
- EXIF 浮点噪声、AI 一键数值噪声（M1、M3）
- 网格场景标题 key 重复导致重叠（M2）
- 前端写死 LUT id → `GET /api/luts`（M3）
- 内置 LUT 两套来源 → 统一由 ip-render 提供（M3）
- M7：冷启动 500→32ms、翻页 p95 188→15ms、批量改星 20k 1213→122ms；三平台安装包 CI 试运行成功，Windows 安装包实机验证（启动、鉴权、卸载）
- 导出色彩：带非 sRGB ICC（Display P3、Adobe RGB…）或 DCF Adobe RGB 标记的来源在解码时转换为 sRGB（moxcms，纯 Rust），缩略图、预览、渲染与导出一致；导出保留 XMP（去掉朝向、尺寸、冲印参数、人脸区域、动态照片/景深/增益图引用，`strip_gps` 时去掉所有 `GPS*`）与 IPTC（APP13 只留 IPTC 记录）；RAW/TIFF/CR3 导出复制原始 EXIF（重建 IFD0/Exif/GPS，不含 MakerNote），RAF/HEIF 原样复制（M5 core）
- 一键全员最佳不再总以组内最佳为底片：逐帧估算需替换的脸（按脸尺寸加权）、替换后仍低于本人最佳的表情、缺席人物，排除模糊/曝光问题帧，组内最佳只在别的帧明显更省事时让位；计划返回 `base_choice` 说明原因（编辑器底片按钮悬停可见），可用 `base_photo_id` 手动指定底片；表情分提升不足 0.04 的脸不再替换（M5 core）
- 导出保留 EXIF + 嵌入 sRGB ICC；Worker 调用超时（504）+ 自动重启；删除会话清理编辑缓存；部分几何在模型安装后刷新；单张人物可被选中；API 浮点噪声统一清理（M5）
