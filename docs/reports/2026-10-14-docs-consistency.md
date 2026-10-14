# 文档一致性核对报告（09 P0-13，WP-M）

日期：2026-10-14　分支：`docs/consistency-pass`（已合并 `origin/main` 至 #51）　原则：以 `main` 上的代码为准。

范围与方法：逐文档对照代码；路由表对 `crates/ip-server/src/lib.rs` 的 `api_router` 与 `auth::routes()`、`remote::routes()` 逐条核对；用户指南里的界面描述对 `web/src/i18n/zh-CN.ts` 与组件逐项 grep。未修改 `docs/09` §5 的状态单元格与 `CHANGELOG.md`；`docs/09` 只新增 §5.5。`docs/07` 的生成表（BEGIN/END GENERATED 标记之间）未手改，相对 `main` 无差异。

结果：核对 38 项；31 项有修改（含前一轮中断会话留下的未提交修改，已逐项复核后保留，并在合并 main 后补齐新功能），7 项核对后无需修改（第 7、9、12、13、15、20、32 项）；遗留问题 7 条见第四节。其中第 8–13、16–18、22、24–28 项的修改来自前一轮，本轮对照 diff 与代码位置做了抽查，而不是逐行重新验证。

## 一、逐项核对

| # | 文档与位置 | 代码证据 | 动作 |
|---|---|---|---|
| 1 | README 状态节（中/英） | 版本 `Cargo.toml` `[workspace.package] version = "0.1.0"`；搜索框 `web/src/components/TopBar.tsx:74`（`disabled`）；匹配参考图 `web/src/components/edit/BasicPanels.tsx:49` | 改写：预发布、各项限制逐条列出；HEIC 写明依赖系统 / AI 组件 libheif / 内嵌 JPEG；Android 写明从未在真机运行 |
| 2 | README 便携模式、存储卡导入、拼图 | `crates/ip-core/src/paths.rs:8`（`imagepicker.portable`）、`card.rs`、`collage.rs`；`lib.rs:135-136,163` | 合并 main 后修正：便携模式不再列为「未提供」；亮点表加存储卡导入、便携模式、拼图 / 联系表 |
| 3 | README 硬件档位表 | `ai-worker/imagepicker_ai/hw.py:101-161`（T3 ≥16 GB、T2 ≥8 GB、T1 ≥4 GB、统一内存映射、RAM<6 GB 降 T0） | 助手与 SDXL 的档位表述对齐；确认 |
| 4 | README 隐私节 | `privacy.allow_network` 设置；明文 HTTP | 补「局域网与远程 AI 为明文 HTTP」；与 PRD / 09 §5.5 D3、D7 一致 |
| 5 | README ModelScope | `registry.yaml` 中 `source.modelscope` 均为空 | 标明目前等同「自动」 |
| 6 | docs/README 索引 | `docs/` 实际文件清单 | 补 api-contract-m8（无 m7 契约）、android-build、release、reports/、bench；版本表加 v0.4；报告链接改到本文件；一页纸摘要第 7 条把便携模式、存储卡导入、拼图移出「尚未提供」 |
| 7 | docs/README 05 行「迁移 v14」 | `crates/ip-core/src/db.rs:17-347`，`MIGRATIONS` 共 14 项 | 确认 v14 |
| 8 | 02 §4 仓库结构 | `Cargo.toml` workspace members；`crates/*/src` 文件清单 | 已按实际重写；本次补 ip-core 的 card / collage / export_formats / paths / portable，ip-server 的 card / fs |
| 9 | 02 §4 ip-lite、ip-infer | `crates/ip-lite/src/{phash,quality,tilt,imgops}.rs`；`crates/ip-infer/src/yunet.rs` | 确认；说明 ip-infer 仅 YuNet、特性默认关闭 |
| 10 | 02 §5.3 渲染管线 | `crates/ip-render/src/{prep,cpu,gpu}.rs`、`Op` 顺序 | 已按实现改写（补丁在裁剪前、无 RAW 显影、仅 sRGB 输出）；补拼图由 `collage.rs` 合成 |
| 11 | 02 §7 数据目录布局 | `paths.rs`、`logging.rs`、`backup.rs`、`settings.rs` | 合并时取 main 的「解析顺序与便携模式」小节（含盘符重写）；目录树保持 |
| 12 | 02 §8 备份 / 诊断 / 日志 | `backup.rs`、`diagnostics.rs`、`logging.rs` | 确认 |
| 13 | 02 HEIF 分层降级 | `crates/ip-imaging/src/imp/heif.rs` | 确认（README 的 HEIC 表述以此为准） |
| 14 | 03 §1 分析档位 | `ai-worker/imagepicker_ai/steps/__init__.py` `PROFILES`（fast / standard）；`ip-core` `Profile`（lite） | 加入 lite；删除「深度」档的现行表述（标 1.x） |
| 15 | 03 §8.1 档位表与判定 | `hw.py:101-161` | 与 hw.py 逐项对照，一致；补手动覆盖说明 |
| 16 | 03 自动调色 / 匹配参考图 | `ip-render/src/auto.rs`；无 `color.match_reference` | 标明规则实现与禁用占位 |
| 17 | 03 多设备时间对齐 | 仅 `device.time_offset_ms / offset_source` 两列 | 标「未实现」；并行 PR 落地后需更新 |
| 18 | 04 UI/UX | `TopBar.tsx:74`、`BasicPanels.tsx:49` | 标注搜索框与匹配参考图为占位 |
| 19 | 05 路由总表 | `lib.rs:132-258`（`.route(` 共 98 处，含多方法路径）+ `auth.rs:716-720` + `remote.rs:213-224` | 逐条核对；补 `/import/sources`、`/import/card`（#47）、`/export/collage`（#51）；移除不存在的接口并单列 |
| 20 | 05 迁移与表结构 | `db.rs` v1–v14；`root_folder.portable` 自 v1 | 确认；v13 / v14 派生表已记入 |
| 21 | 05 编辑栈示例 | `ip-render/src/lib.rs`（`Op`、`Crop.rect` 必填、`Warp::Face/Body`、`Patch`、`Beauty`、`Lut`）；`crates/ip-render/tests/schema.rs:4` | 示例补 `rect` 等必填字段，字段名与枚举逐个对照类型；以人工对照为准，未在本环境运行 cargo 测试 |
| 22 | 05 事件 / Worker 方法表 | `crates/ip-server/src/ws.rs`；`ip-worker-client/src/protocol*.rs` | 已按实现改写；确认 |
| 23 | 07 生成表 | `ai-worker/scripts/gen_model_table.py` | 标记内相对 main 无改动；本机无 PyYAML，未能运行 `--check`，CI 的 `test_model_table.py` 会校验 |
| 24 | 07 §3 / §4 默认组合、下载源 | `models/registry.yaml`、`download.py` | 默认组合与镜像顺序已更新；字体（§6）确认 |
| 25 | 08 能力表 | `apps/desktop/plugins/imagepicker-media`、`ip-lite`、`remote.rs` | 增「状态」列，端侧闭眼 / 人物 / 美颜、MediaPipe 等标为未实现；已实现项标「🧪 未在真机验证」 |
| 26 | android-build.md CI job 名 | `.github/workflows/ci.yml` 的 `android-check` | 已修 |
| 27 | bench/testset-lite-2026-10-07.md | 被 c9fef3b1、63a8ffe3 取代 | 文首加「已过时」 |
| 28 | 用户指南 Lite 档 | `i18n/zh-CN.ts:503`（`profile_standard_remote`）及 lite 档文案 | 已写入第 6 节与首页说明 |
| 29 | 用户指南移动端 | `web/src/lib/layout.test.ts:5`（768 / 1024）；`i18n/zh-CN.ts:1596,1605,1634-1644` | 第 20 节所述（底栏、挑片、快速挑片、抽屉编辑、相册权限）逐项 grep 均存在 |
| 30 | 用户指南远程 AI | `i18n/zh-CN.ts:1656-1685`；`remote.rs:213-224` | 配对、吊销、断开、离线提示均存在；写明无摄像头扫码、不含本地助手 |
| 31 | 用户指南快捷键 / Ctrl+K | `web/src/lib/shortcuts.ts`、`CommandPalette.tsx`、`settings/ShortcutsSection.tsx`（#50） | 合并冲突取 main：设置页快捷键为可编辑表；第 5 节「自定义快捷键」 |
| 32 | 用户指南便携模式 | `paths.rs:8,114`（#48） | main 已有第 2 节「Windows 便携版」，确认与代码一致 |
| 33 | 用户指南 LAN / 远程 AI 明文 HTTP | 无 TLS 代码 | 第 15、20 节开头加粗体警示框（D7），中英文 |
| 34 | 用户指南「匹配参考图」「自然语言搜索」 | `BasicPanels.tsx:49`、`TopBar.tsx:74` | 已写成占位（NL 搜索 PR 合并后需删除） |
| 35 | 用户指南设置页清单 | 设置分区（含 remote、devices、shortcuts） | 表格已含远程 AI、远程 AI 设备、快捷键 |
| 36 | PRD F10.6、§4 隐私 | 联网只用于模型下载 | D3：改为「无用户操作不联网」 |
| 37 | PRD F2.1 / F2.9 / F2.11 / F4.4 / F3.7 / F10.3、§4 平台 | 见 09 §5.5 | D4 / D5 / D7 / D8 默认写入 PRD |
| 38 | 09 §5.5 | — | 新增（唯一的 09 改动） |

## 二、合并 main 时的冲突

`docs/02` 便携模式小节、`docs/05` 路由表、两份用户指南的设置表各有一处冲突。处理：02 取 main（#48 的实现说明）；05 保留本分支的完整路由表并补入 main 新增的三条路由；用户指南取 main 的「可编辑快捷键」行并保留本分支的「远程 AI」两行。

## 三、产品默认决定（09 §5.5）

D3 无用户操作不联网；D4 NL 搜索与多设备对齐由并行 PR 实现、深度档与虚拟副本移出 1.0；D5 macOS 最低 13；D7 明文 HTTP 仅限可信局域网（已加警示框）；D8 保留「修改原图」为二次确认的高级选项；D2 指向 #26 的标注。D1、D6 保持未决。

## 四、剩余问题

1. **D1 产品名、D6 签名与分发账号**：需要产品负责人。
2. **D2 是否商用**：默认只覆盖个人非商用；商用前要替换 NIMA 并做许可审计。
3. **并行 PR 合并后需回头改文档**：NL 搜索（README 状态、用户指南第 1 节与 FAQ、05 的「无 q 参数」、docs/README 摘要第 7 条）、多设备对齐（03 的「未实现」）、macOS 13（`tauri.conf.json:63` 现为 11.0，09 §3 与 06 的描述）。
4. **docs/07 生成表**：本环境无法运行生成脚本的 `--check`，依赖 CI。
5. **编辑栈示例**：靠人工对照类型，建议在 `schema.rs` 增加直接读取 docs/05 示例的测试，避免再次漂移。
6. **08 的 Android 能力**：全部来自代码与 CI，没有真机数据。
7. **CHANGELOG.md** 仍缺大量提交（按要求未改；发布前 `git cliff` 重新生成）。
