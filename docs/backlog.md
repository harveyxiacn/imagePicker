# 待办与已知问题（Backlog）

> 每轮集成后更新。按优先级：**P0** 发布前必须修复，**P1** 1.0 前完成，**P2** 之后。

## 校准（需要真实照片评测集）

| 优先级 | 项 | 说明 |
|---|---|---|
| P0 | 评分阈值校准 | 过曝判定只看整图高光溢出比例，白底图大量误报；美学/IQA 换算、分组松紧、星级映射均为经验初值 |
| P1 | 自适应同步参数 | `edit/sync.rs` 的 `K_PER_LN`、`T_PER_LN` 在 FakeRenderer 上调参，需在真实渲染器上复核 |
| P1 | 美颜强度 | 形变/美颜强度在合成人像上标定，真实人脸效果需人工盲评 |

## 功能缺口

| 优先级 | 项 | 来源 |
|---|---|---|
| P1 | HEIC/HEIF 缩略图（系统解码器：WIC / ImageIO / libheif） | M1 imaging |
| P1 | 倾斜检测、构图分 | M2 |
| P1 | RAW 预览在真实相机文件上验证（目前仅合成样本） | M1 imaging |
| P2 | 以脸搜脸「这是新的人」 | M4 web |
| P1 | RAW/HEIC 导出只合成 EXIF（未复制原始 EXIF）；XMP/IPTC 丢失；非 sRGB 源未做色彩转换 | M5 core |
| P2 | 生成任务没有取消接口、没有持久化任务记录 | M5 |
| P2 | 一键全员最佳总以组内最佳为底片 | M5 core |
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

## 许可审计（商用前）

- NIMA 美学/技术质量模型：权重 Apache-2.0，训练数据 AVA / TID2013 有研究用途条款。
- 天空分割 `skyseg-u2net`：训练数据未公开。
- GFPGAN：FFHQ 训练数据有独立条款；Real-ESRGAN ONNX 来自 facefusion 镜像（上游 BSD-3）。

## 已在集成中修复（记录）

- 拍摄时间按查看者时区显示 → 改为拍摄地当地时间（M1）
- EXIF 浮点噪声、AI 一键数值噪声（M1、M3）
- 网格场景标题 key 重复导致重叠（M2）
- 前端写死 LUT id → `GET /api/luts`（M3）
- 内置 LUT 两套来源 → 统一由 ip-render 提供（M3）
- M7：冷启动 500→32ms、翻页 p95 188→15ms、批量改星 20k 1213→122ms；三平台安装包 CI 试运行成功，Windows 安装包实机验证（启动、鉴权、卸载）
- 导出保留 EXIF + 嵌入 sRGB ICC；Worker 调用超时（504）+ 自动重启；删除会话清理编辑缓存；部分几何在模型安装后刷新；单张人物可被选中；API 浮点噪声统一清理（M5）
