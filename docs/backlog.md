# 待办与已知问题（Backlog）

> 每轮集成后更新。按优先级：**P0** 发布前必须修复，**P1** 1.0 前完成，**P2** 之后。
> 2026-10-09 的完整评审（各需求状态、文档矛盾、本表未列的缺口、验收标准与并行工作包）见 [09 完成度评审与收尾计划](09-完成度评审与收尾计划.md)。

## 校准（需要真实照片评测集）

> 合成评测图库：2026-10-08/09 起使用 1000 张图库 `bench/data/qwen-photo-library`（ComfyUI + Qwen-Image 2.1，`scripts/comfy-photo-library.py`（脚本尚未提交），图片不入库；`manifest.json` 标签是提示词意图，需人工核验），评测脚本 `bench/eval-library.py`。连拍变体改由 Qwen 原生编辑从底片派生，lite 档连拍分组 pairwise 精确率 0.96 / 召回 0.97（63a8ffe3）。2026-10-07 的 74 张首轮记录 `bench/testset-lite-2026-10-07.md` 仅作历史参考（其中「连拍 pHash 距离过大需 img2img 派生」已解决）。

| 优先级 | 项 | 说明 |
|---|---|---|
| P0 | 评分阈值校准（真实照片复核） | 2026-10-09 已在 1000 张合成图库（`bench/data/qwen-photo-library`，`bench/eval-library.py`）上校准问题标签：过曝/欠曝要求细节丢失（白底人像、霓虹夜景不再误报），噪点阈值 0.60→0.22，连拍内清晰度 < 0.8× 最清晰帧标「模糊」。lite 档位：过曝精确率 0.52→0.92，欠曝 0.56→0.77，噪点召回 0→0.83，模糊精确率 0.28→0.37 / 召回 0.35→0.52。仍待：真实照片复核；背景虚化人像的模糊误报（需人脸区域清晰度）；美学/IQA 换算、分组松紧、星级映射 |
| P1 | 倾斜检测阈值（真实照片复核） | 2026-10-09 实现（03 §3.1.1，所有档位同一算法）。只在合成图库的旋转集上标定：置信度 ≥ 0.2 且 2°–10° 时精确率 0.91，但召回低（\|角\| ≥ 3° 时 0.14，有直线的照片 0.21）；原图库 2.4% 被标（部分生成图本身就斜）。仍待：真实照片复核；仰拍建筑（竖线会聚）、车窗/桌面等单侧透视的误报；广角畸变的弯线不计入 |
| P1 | 自动拉直 / 智能裁剪（真实照片复核） | 2026-10-13 实现（P1-11，03 §3.1.1 / §6.6）。拉直沿用存储的 `tilt_deg`，合成倾斜样本最大残余 0.14°（≤ 0.5° 验收，见 03 §6.6）；智能裁剪的三分法/缩放常量（`MIN_SCALE`、`SUBJECT_FILL`、`CENTRE_SHARE`）与显著性代理只在合成图上验证。仍待：真实照片人工盲评；无脸时的显著性代理对杂乱画面较弱；尚无 Android 编辑页入口；尚不考虑裁剪后的主体姿态/视线方向 |
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
| P2 | 事件层（09 P1-1）余项：事件只有「日期 + 时间段」默认名，没有反向地理编码（离线 GeoNames）与 VLM 短标题，用户也无法在界面里给事件命名（`event.title` / `place` 已预留）；事件按场景切分，一个场景内部的大幅移动（高速行驶中的连拍）不会切开；GPS 位置取平均，跨日期变更线的事件位置不准；事件 id 每次重算都会重新分配（界面选中的事件在分组变化后失效并自动取消）；事件过滤（`event_id`）不能存进智能集合；助手（自然语言筛选）还不认识「事件 / 重复」 | P1-1 |
| P2 | 重复检测（09 P1-3）余项：完全重复用 `content_key`（头尾各 64 KiB + 长度）判定，未做整文件哈希，仅改动文件中段而长度不变的两个文件会被误判（概率极低，且只标记、不删除）；近似重复只认「至少一张无 EXIF」且 pHash ≤ 4 位、宽高比相同的对，缩小到很小尺寸或轻度裁剪的副本、两张都带 EXIF 的重新编码副本不识别（合成图库的 `missing-lens` 副本带着同一拍摄时间，按连拍堆叠、不标重复：只靠 pHash 分不出它与连拍相邻帧，09 §7 的 36/36 因此只有 exact-duplicate 与 no-exif 两类 24/24 能达成）；一张无 EXIF 副本会把距离 ≤ 4 位的连拍帧一并并入重复组；界面只有角标、智能集合与 `GET /api/photos/{id}/duplicates`，没有「并排比较 / 保留一张」的清理流程；阈值只在合成图（JPEG 质量 50–98 重新编码、半尺寸缩放，见 `analysis/duplicates.rs`）上验证，真实照片（微信压缩、截图）需复核 | P1-3 |
| P1 | RAW 预览在真实相机文件上验证（目前仅合成样本） | M1 imaging |
| P2 | HEIC/HEIF 色彩余项：HDR（`nclx` 传递函数 PQ / HLG、iPhone 增益图）不做色调映射，按 SDR 原样显示；WIC 路径的色彩转换未实测（本机无 HEIF 编解码器） | M5 core |
| P2 | 导出元数据余项：DNG/CR3/HEIC 文件内部的 XMP、PNG/WebP/TIFF 内嵌 XMP/IPTC 不复制（只用 JPEG 内嵌包或 `.xmp` 侧车）；扩展 XMP（>64 KB）丢弃；导出不写入目录库的评分/关键词；RAW 预览色彩空间只看预览 JPEG 自带 ICC / DCF 标记，RAW EXIF 重建仅在合成样本上验证 | M5 core |
| P2 | 导出格式余项：WebP 只有无损（纯 Rust 编码器无有损模式，文件较大）；TIFF 为 8 位无压缩，无 16 位输出；PNG/WebP 不写 IPTC 块（只在 XMP 中）；TIFF 导出不带 MakerNote；无「打印 300dpi」预设与用户自定义导出预设 | P0-8 |
| P2 | 色彩转换前生成的缩略图/预览缓存不会自动重建（清除缓存后重新生成）；导出只输出 sRGB（无 P3 输出），色域外颜色裁切 | M5 core |
| P2 | 最佳表情选底片的系数（`MIN_GAIN` 0.04、残留 ×6、缺席 ×3、切换余量 0.25）只在合成矩阵上验证，需真实连拍复核；不估计相机移动；手动换底片时网页仍用自动底片的计划（可合成性按自动底片的姿态判断、原自动底片不在候选中），可改用 `?base_photo_id=` 重新取计划 | M5 |
| P1 | 大面积消除（超过 512px 的洞）结果偏糊、有残影；默认 LaMa，需 SDXL 增强包实测（真实照片：去除雕像） | M5 worker |
| P1 | 雕像/海报上的人脸会被当作「路人」；界面需确认后才消除，但检测应区分真人（活体/语义） | M5 |
| P2 | 合影修复（`subject_flags` 第二遍、`besttake/align.py` 分档排除区）只在合成测试上验证；需在 1000 张合成库上重跑 `bench/eval-standard.py` / `bench/besttake-align.py` 并补数字，再用真实合影连拍复核阈值（`EXCLUSION_ZONES`、`MIN_BG_FRAC`、`subject_flags` 的比例常量） | M5 |
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
| P2 | 运维余项（09 P1-13）：①Windows 原生异常（SEH）与 Rust `abort` 不经过 panic 钩子，Rust 侧的原生崩溃不留记录（Python 侧由 `faulthandler` 覆盖）；②日志与崩溃记录的路径 / 密钥遮盖是正则、尽力而为（带空格的 Windows 路径只保证遮盖到图片文件名），分享前仍应让用户检查；③Android 仍只写控制台日志（数据目录在 Tauri `setup` 才确定）；④诊断包的硬件概况没有内存 / 显存之外的系统信息（无 `sysinfo` 依赖），操作系统只有系统名与架构，没有版本号 | P1-13 |
| P2 | lite 档位的 `faces` 步骤不输出关键点 / 闭眼 / 表情（需 MediaPipe 或 ONNX 关键点模型） | M8.1 infer |
| P2 | 分析任务健壮性余项（09 WP-B）：①按步骤增量只覆盖 `embed` / `aesthetic` / `iqa` / `scene`，`faces` / `identity` 与人脸行绑定，装了人脸模型后要整批重新分析（`force`）；②暂停只在批次之间生效（每批最多 32 张，远程 AI 的一批可能要数十秒）；③续跑的真实进程强杀只在测试里用「丢弃运行时」模拟，未在 Windows / macOS / Linux 上做真正的 kill -9 实机验证；④桌面「改用快速分析」不会清掉已有的 fast / standard 结果，混合档位的会话在评分上按各自的分项计算，未做跨档位校准 | M2 / 09 WP-B |
| P2 | 筛选补全余项（09 P1-5）：①`lens` 按 EXIF 原始字符串精确匹配，同一支镜头的不同写法（`RF24-70` / `RF 24-70`）是不同选项，没有镜头名规范化（设备名有，镜头没有）；②`face_ids` 每次查询线性扫描会话内所有有身份特征的人脸（没有向量索引，耗时与人脸数成正比，大会话上未测量；列表总数缓存在目录库无写入期间有效）；③`sharp=unknown` 表示没有清晰度度量（未分析，或该步骤没有输出），界面上叫“未分析”；④`pnpm dev:mock` 的模拟后端没有关键词和人脸特征（关键词筛选恒为空，`face_ids` 被忽略）；⑤自然语言搜索（P1-4）不在此范围 | P1-5 |
| P2 | 可访问性余项（09 WP-O）：①撤销合并人物时按人脸重建人物，原人物 ID 与已保存的美颜档案不会恢复，撤销「并入分组」假定两组在时间上相连；②字体缩放上限 150%（09 验收口径写的是 200%），个别写死像素尺寸的控件（缩略图、手机端 44px 触控目标）不随字号变化；③axe 冒烟脚本只覆盖桌面视口的 mock 数据，未检查手机布局、屏幕阅读器实际朗读与 Tauri 窗口；未接入 CI；④高对比度 / 字号只存本机，不随服务端设置同步到其他设备 | P1-16 |

## 助手

| 优先级 | 项 | 说明 |
|---|---|---|
| P1 | 助手 20 条指令基准（docs/09 M6）的 LLM 计划实测 | 目标绑定、`fallback` 与大范围修改确认（api-contract-m6 A.4）已有单元/接口测试，但未用真实本地模型重跑 20 条基准；前端尚未展示 `criteria` / `confirm_text` / `fallback` |

## 许可审计（商用前）

- NIMA 美学/技术质量模型：权重 Apache-2.0，训练数据 AVA / TID2013 有研究用途条款。
- 天空分割 `skyseg-u2net`：训练数据未公开。
- GFPGAN：FFHQ 训练数据有独立条款；Real-ESRGAN ONNX 来自 facefusion 镜像（上游 BSD-3）。
- 以上条目已在 `registry.yaml` 的 `license_info` 中结构化记录（`usage: unknown`，附训练数据条款与备注），应用内以徽标提示，`docs/07` §5 由注册表生成（P1-17）。`unknown` 只表示尚未确认，**是否可商用仍待许可决定**：NIMA ×2（AVA / TID2013 研究条款）、`skyseg-u2net`（训练数据未公开）、`gfpgan-v1.4`（FFHQ）、`realesrgan-*` ×4（第三方镜像无许可元数据）。其余 20 项仅按权重许可标为 `commercial-ok`，训练数据条款多为 `not-recorded`，发布前审计时需逐项补全。
- ModelScope 下载源尚无实现（注册表无 `source.modelscope`），选它时按「自动」处理。

## 已在集成中修复（记录）

- Best Take 同人校验（09 §5.1 P0-2，WP-I）：`POST /api/besttake` 对源脸与底片脸不在同一连拍、不在同一轨迹/人物、或身份嵌入余弦低于 `MIN_IDENTITY_COS`（0.30）时返回 422（`besttake_cross_burst` / `besttake_person_mismatch` / `besttake_low_similarity`），界面显示对应的本地化提示；缺身份嵌入（lite）时只做连拍与轨迹/人物检查；自动路径只在同一轨迹内选脸（`tests_m5.rs`、`ip-server/tests/m5.rs`、`web/src/lib/gen.test.ts`）。契约见 `api-contract-m5.md` C 节
- 可访问性与交互（09 §5.2 P1-16，WP-O）：①设置 → 语言与外观新增「高对比度」（`data-contrast="high"`，深色 / 浅色均有 AA 色板与 3px 焦点环）和「界面字号」90%–150%（`--font-scale`，rem 化的字号 / 控件，任意像素字号类同步缩放），两项只存本机（`imagepicker.ui`）；②`Ctrl+K` 命令面板（`CommandPalette.tsx`）对键位表动作与页面跳转模糊搜索，键位表仍是唯一来源；③撤销覆盖组拆分 / 合并与人物重命名 / 隐藏 / 合并 / 移动人脸（`lib/structureUndo.ts`，新增只读接口 `GET /api/people/{id}/faces` 供撤销合并）；④`pnpm a11y`（Playwright + axe-core，对 mock 后端）；⑤全局焦点环与输入框聚焦环。余项见「功能缺口」
- 目录与文件（09 §5.1 P0-7、§5.2 P1-6，WP-F1）：①同目录同主文件名的 RAW+JPEG 导入后是一个网格项（JPEG 显示 / 分析，RAW 为 companion，`pair_id`），评分 / 旗标 / 色标同步到两者及各自的 XMP 侧车，分组与分析只算一次，导出可选 `pair_member`；②`POST /api/import` 支持 `exclude`（通配符 / 子文件夹 / 名字，重新扫描沿用）；③桌面端文件监听，增删文件约 2–3 秒内进网格；④缺失文件显示断链，`POST /api/sessions/{id}/relocate` 按相对路径 + 大小重新定位文件夹；⑤已淘汰照片经确认后可移入回收站（`trash`）或 `_rejected`，目录库跟随，绝不硬删（`tests_files.rs`、`ip-server/tests/files.rs`）

- 数据安全 P0（09 §5.1 P0-1 / P0-3 / P0-4）：①原图只读成为硬约束——「侧车 + 内嵌」改为需二次确认的高级模式「修改原图（写入内嵌 XMP）」（`xmp_mode = modify_originals`，服务端要求 `confirm_modify_originals`，旧值降为 `sidecar`），其余模式下导入、分析、评分 / XMP 同步、编辑、导出后原图 SHA-256 不变（`tests_safety.rs`）；②目录库启动后后台 `quick_check`、每日 `VACUUM INTO` 备份留 7 份、打不开时移到 `backups/corrupt-*.db` 并以空目录库启动、可从备份恢复（`GET/POST /api/catalog*`，界面在启动弹窗与「设置 → 目录库备份」）；③人脸识别首次说明与同意（`faces.consented`），同意前不检测人脸、不计算人脸特征（lite 档同样），拒绝即 `faces.enabled = false`。同意文案待产品确认（09 §5.4 / P0-4）
- 运维（09 §5.2 P1-13，WP-A）：①日志——服务端 / 桌面端写滚动日志到 `<数据目录>/logs`（按日或 5 MiB 换文件，14 天、40 MiB 封顶）；②崩溃记录——Rust panic 在默认钩子之前写 `crashes/crash-*.txt`（时间、版本、系统、线程、信息、回溯），AI worker 的未处理异常（`worker-crash-*.txt`）与原生崩溃（`faulthandler`，`worker-native-*.log`）进同一目录，worker 意外退出时核心保存其 stderr 末尾；③诊断包——设置页「诊断」→「导出诊断信息」（`POST /api/system/diagnostics`，仅所有者）生成 zip：版本、系统与硬件概况、脱敏设置、最近日志与崩溃记录（路径与密钥遮盖）、目录库统计（数量、架构版本、完整性检查）；不含照片、缩略图、人脸裁剪与特征向量、目录库本身、照片路径（`logging.rs`、`diagnostics.rs`、`tests_diag.rs`、`ip-server/tests/security.rs`、`ai-worker/tests/test_crash.py`，api-contract-m6 §G）
- 分析可靠性（09 §5.1 P0-5、§5.2 P1-18）：①**续跑**——运行参数存在 `task.params`，应用重启后上次未结束的分析自动续跑，只发送剩余照片（已入库的批次不重算，`force` 运行同样），同一运行最多自动续跑 3 次；②**暂停 / 继续**——`POST /api/analysis/pause|continue`，批次之间停下，状态 `paused` 经 `analysis.progress` / `task.progress` 事件推送，重启后仍为暂停；桌面状态栏与手机分析面板有对应按钮；③**lite 兜底**——桌面分析档位新增「快速（本机）」；模型缺失、AI 组件未安装或 worker 不可用时，`fallback_lite` 运行改用 `ip-lite` 完成同一批照片，失败的运行带 `error_code`，界面（模型同意框、安装提示、状态栏）提供「改用快速分析」；④**按步骤增量**——由 `analysis.model_versions` 判断，安装新模型后只补算对应步骤（`GET /api/analysis/plan` 预告），不重算嵌入 / 人脸 / 人物（api-contract-m2 C.5）
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
- 筛选补全（09 P1-5）：`GET /api/photos` 新增 `lens`、`taken_from`/`taken_to`、`tags`/`tag_mode`、`sharp`、`face_ids`/`face_similarity`（未聚类的脸按身份特征近邻找照片，阈值可调，默认 0.38），`GET /api/sessions/{id}/filter-options` 给出镜头 / 关键词 / 时间跨度；`POST /api/faces/search` 新增 `min_similarity`；网页筛选栏有镜头、关键词、日期弹窗与清晰度芯片，右键人脸可“有相似人脸的照片”（见 api-contract-m1「筛选补全」）
- 以脸搜脸「这是新的人」（#5）：结果里展示相似人脸（含当前所属人物）供勾选，`POST /api/people` 把它们（以库内人脸搜索时含查询脸）移到新人物并锁定，重新聚类/分析保持；人脸菜单新增「查找相似的脸」（M4 web）
- 一键全员最佳不再总以组内最佳为底片（#3）：逐帧估算需替换的脸（按脸尺寸加权）、替换后仍低于本人最佳的表情、缺席人物，排除模糊/曝光问题帧，组内最佳只在别的帧明显更省事时让位；计划返回 `base_choice` 说明原因（编辑器底片按钮悬停可见），可用 `base_photo_id` 手动指定底片；表情分提升不足 0.04 的脸不再替换（M5 core）
- P0-8 导出（已完成）：可选 JPEG / PNG / TIFF / WebP，各格式携带的 EXIF/XMP/IPTC/ICC 见 `api-contract-m5.md` F 节；对话框有「去除 GPS 位置」「去除全部元数据」，对原样复制的原图同样生效（JPEG/PNG/WebP 按段/块清理，图像数据不变；RAW/HEIF/TIFF 改为全尺寸重新编码）；导出预设只有服务端一张表（`GET /api/export/presets`，对话框与 AI 助手共用，含 3:4、4:5 裁切）；命名模板支持 `{event}` `{scene}` `{camera}` `{rating}`；导出失败时只删除本次创建的文件（规划文件名后才出现的同名文件不再被删除，也从不覆盖）
- 生成任务（最佳表情 / 消除 / 增强）、导出与分析可按 id 取消（#6）（`POST /api/tasks/{id}/cancel`，取消传到 worker 的进行中请求，取消后不提交补丁）并记录在 `task` 表（`GET /api/tasks`，保留 200 条，重启时把残留的进行中任务记为「已中断」）；界面有取消按钮和「设置 → 任务记录」（M5，见 api-contract-m5 G）
- 导出保留 EXIF + 嵌入 sRGB ICC；Worker 调用超时（504）+ 自动重启；删除会话清理编辑缓存；部分几何在模型安装后刷新；单张人物可被选中；API 浮点噪声统一清理（M5）
