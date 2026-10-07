# Android 交叉编译（Rust 核心）

Rust 核心（`ip-core` / `ip-imaging` / `ip-render` / `ip-server`、`ip-cli`）可交叉编译到
`aarch64-linux-android`（真机）与 `x86_64-linux-android`（模拟器）。Tauri Android 外壳另见 `apps/`。

## 准备

1. Android SDK + **NDK 27**（`sdkmanager "ndk;27.2.12479018"`）。
2. `rustup target add aarch64-linux-android x86_64-linux-android`
3. 配置脚本（Git Bash / Linux / macOS）会按 `ANDROID_NDK_HOME` → `ANDROID_NDK_ROOT` →
   `$ANDROID_HOME|$ANDROID_SDK_ROOT|%LOCALAPPDATA%/Android/Sdk` 下最新的 `ndk/<版本>` 查找 NDK，并导出
   `CARGO_TARGET_<TRIPLE>_LINKER`、`CC_<triple>`、`CXX_<triple>`、`AR_<triple>`，使 `rusqlite`（bundled SQLite）
   等 C 依赖使用 NDK clang。仓库内不含任何用户绝对路径（`.cargo/config.toml` 只写 PATH 可解析的工具名）。

```bash
source scripts/android-env.sh            # 之后直接用 cargo
cargo build -p ip-server --target aarch64-linux-android
cargo build -p ip-cli --release --target x86_64-linux-android

# 或一步到位（不污染当前 shell）：
scripts/cargo-android.sh build -p ip-server --target aarch64-linux-android
```

`ANDROID_API`（默认 24）可覆盖 clang 的 API 级别。CI（`.github/workflows/ci.yml` 的 `android` 任务）在
ubuntu 运行器上对两个目标执行 `cargo check -p ip-server -p ip-cli`。

## 平台差异（均已按 `cfg(target_os = "android")` / 运行时能力处理）

| 项 | Android 行为 |
|---|---|
| 数据目录 | 无平台默认值：外壳必须通过 `CoreConfig::data_dir` / `ServerConfig::data_dir`（或 `IMAGEPICKER_DATA_DIR`）传入应用私有目录，否则 `Core::open` 报错 |
| AI worker | 不可用：`Core` 使用 `UnavailableWorker`（`/api/system/hardware` → `worker.state = "unavailable"`，`lite: true`）；`ManagedWorker` 的进程派生、`kill_tree`/`pid_alive`（`taskkill`/`kill`）在 Android 上为空操作 / 报错。`IMAGEPICKER_NO_WORKER=1` 可在任意平台强制同样行为 |
| AI 运行时安装 | `can_install` 返回「不支持」；`detect_hardware`（`nvidia-smi`）直接返回无 NVIDIA |
| 端侧分析 | `lite` 档位（纯 Rust，见 `docs/api-contract-m8.md` §B） |
| 路径白名单 | 默认根 `/storage/emulated/0/{DCIM,Pictures,Download}`（+ 外壳经 `extra_roots` 传入的相册路径）；不使用 home |
| 渲染 | 仅启用 wgpu **Vulkan** 后端（未编译 GLES）；无 Vulkan 适配器 / 仅软件光栅（SwiftShader）/ 初始化失败 → 自动回退 CPU 渲染 |
| 线程 | 缩略图线程与 lite 分析线程 ≤ 4 |
| SQLite | 连接池 3；`mmap_size=0`、`cache_size=-8192`、`temp_store=FILE`、`wal_autocheckpoint=500`（桌面：6 连接、256 MiB mmap、32 MiB 缓存） |
| `memmap2` | 在 Android 上可用（只读映射），无需处理 |

## 在模拟器 / 真机上跑 CLI（调试）

```bash
scripts/cargo-android.sh build -p ip-cli --release --target x86_64-linux-android
adb push target/x86_64-linux-android/release/imagepicker /data/local/tmp/
adb push <照片目录> /data/local/tmp/photos
adb shell 'IMAGEPICKER_DATA_DIR=/data/local/tmp/ipdata /data/local/tmp/imagepicker analyze /data/local/tmp/photos --profile lite'
```

## 已知限制

- 未集成 GLES 兜底（wgpu `gles` 特性需要 EGL 窗口系统初始化）；无 Vulkan 的旧设备走 CPU 渲染。
- `ip-infer`（ONNX Runtime，YuNet 人脸检测）目前仅在桌面验证（`ip-core` 的 `infer` 特性，默认关闭）；`ort` 尚未链接到 Android：Android 构建不启用该特性，也没有 Android 版 ONNX Runtime 库与 NNAPI/XNNPACK 执行提供器的接线。因此端侧人脸 / 嵌入 / 美学步骤在手机上仍不可用，评分按可用分项重新归一。
