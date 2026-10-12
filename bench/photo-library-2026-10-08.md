# Qwen Image 2.1 本地合成照片库

目标：1,000 张可导入 JPEG。用途是功能回归与人工挑片测试；不是完整的真实摄影基准，也不证明所有测试已经覆盖。

| 类别 | 张数 | 重点 |
| --- | ---: | --- |
| 虚构人物参考 | 8 | 跨场景身份一致性参考 |
| 单人 | 200 | 8 人 × 25 场景，肤色、年龄、光线、构图 |
| 多人合照 | 120 | 30 场景 × 4 次，2–8 人组合 |
| 连拍 | 344 | 60 组，单人 30 组、多人 30 组；0.4 秒帧间隔 |
| 风景、街道、建筑 | 120 | 40 场景 × 3 光照 |
| 物品、美食、宠物、文档 | 100 | 50 场景 × 2 光照 |
| 缺陷意图样本 | 60 | 12 场景 × 模糊、欠曝、过曝、噪声、抖动 |
| 数据边界 | 48 | 12 张各派生完全重复、无 EXIF、无镜头、另一设备 +120 秒 |

## 文件与运行

- 脚本：`scripts/comfy-photo-library.py`；复用 `comfy-testset-v2.py` 的场景清单。
- 输出：`bench/data/qwen-photo-library/jpeg/`，可直接导入 imagePicker。
- `png/` 保存原图；`manifest.json` 保存完成记录；`plan.json` 保存完整计划。
- `workflows/` 保存逐图 ComfyUI API 工作流；`inflight.json` 保存正在执行的 prompt ID。
- 图像和执行记录在已被 Git 忽略的 `bench/data/` 下，不提交大文件。
- 本机服务为 `http://127.0.0.1:8191`，原图同时保存在 ComfyUI 的 `output/imagepicker-library-20261008/`。

```powershell
python scripts/comfy-photo-library.py --plan
python scripts/comfy-photo-library.py --pilot
python scripts/comfy-photo-library.py
python scripts/test_comfy_photo_library.py
python scripts/comfy-photo-library-audit.py
```

默认 20 步，euler/simple，CFG 1，约 1.5MP；本机模型为 Qwen Image 2.1 INT8 convrot，Qwen3VL 8B W4A8 和 Qwen 2.1 BF16 VAE。无需下载模型。首张执行先验证兼容性；长任务串行提交，发现外部队列任务时等待，不清队列、不重启服务。若需停止，在输出根目录创建 `STOP` 文件，当前图完成后不再提交下一张；恢复前删除该文件。

`progress.json` 记录已保存张数和最新耗时；`generation.log` / `generation.err.log` 记录后台运行输出（采用后台启动时）。每轮结束自动执行文件哈希、EXIF 回读、项目原生 pHash 连拍距离检查，输出 `audit.json` 和 `contact-sheet.jpg`。`complete` 仅表示文件数量和完整性完成，不表示人物身份、表情或算法准确率通过人工验收。运行需要本机已有的 Pillow、NumPy、OpenCV。

## 元数据与质量边界

14 种机身／镜头配置（Sony、Canon、Nikon、Fujifilm、OM、Panasonic、Ricoh、Apple、小米、Samsung）。同场景设备一致。镜头焦距、光圈、ISO、快门、原始时间、毫秒时间、+08:00 时区、尺寸和合成来源写入 EXIF，并在保存后读回验证。参数是模拟测试配置，并非真实拍摄测量；不据此评测镜头成像风格。

身份、人数、闭眼、模糊等标签是**提示词意图，待人工核验**。参考图只能提高人物一致性，不能保证身份和人数正确。连拍采用 Qwen 原生参考图编辑：基础帧连接编码器与 VAE，使用编码器输出的对应尺寸 latent，denoise 1.0，并明确指定被编辑人物的位置和动作。首轮低 denoise 0.30 试验虽然保持构图，但多人闭眼未生效，因此不用于完整批次。原生编辑对照图明确实现最左侧人物闭眼，pHash 与基础帧相差 2。仍须测量其余组内 pHash 并检查表情变化，不能把“生成成功”当作连拍或 Best Take 测试通过。

无 EXIF 与镜头缺失样本、重复字节和 +120 秒设备时差属于可确定的元数据测试样本；其图像内容仍来源于 Qwen。48 张派生样本不会被计为新的独立摄影场景。

尚需单独准备：真实相机 RAW/HEIC、RAW+JPEG 配对、损坏文件和色彩配置文件，以及 10,000+ 文件性能压力集。不要用 JPEG 改后缀冒充 RAW，也不要用合成照片的评分校准直接代表真实用户素材。

## 2026-10-08 启动验收

完整批次启动前已保存并校验 16 张 JPEG，文件哈希与 EXIF 均无错误；3 项脚本测试通过。单人、四人合照的两组连拍各抽取 3 帧，组内最大 pHash 距离均为 4。视线偏离效果肉眼可见。多人闭眼最终试验帧中最左侧女性和戴眼镜男性都闭眼，超出了“只改变 a1”的意图，因此不得用 victim 标签直接作为逐人状态真值；可作为两人闭眼的合照样本。独立原生编辑对照图仅最左侧人物闭眼。

这只是小批工作流验收。完整生成进度以输出目录的 `progress.json`、`manifest.json` 为准；启动后台进程不表示 1,000 张已经完成。旧的 4 张低 denoise 试验变体保存在 `pilot-v1/`，不计入正式库。
