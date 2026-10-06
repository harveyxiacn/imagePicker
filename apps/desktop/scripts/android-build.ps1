# Build the Android APK on Windows machines where `tauri android build` fails at the final
# "create symbolic link" step (no Developer Mode / SeCreateSymbolicLinkPrivilege).
# The Tauri CLI still compiles the Rust library first; we then copy the .so into jniLibs
# (instead of symlinking it) and run Gradle without the Rust task.
#
#   pwsh apps/desktop/scripts/android-build.ps1 [-Target x86_64|aarch64] [-Release]
#
# Needs: JDK 17+ (JAVA_HOME), Android SDK (ANDROID_HOME), NDK 27 (NDK_HOME), rustup targets
# aarch64-linux-android / x86_64-linux-android, pnpm. On Linux/macOS/CI use
# `pnpm tauri android build --apk --target <t>` directly.
param(
    [ValidateSet('x86_64', 'aarch64')][string]$Target = 'x86_64',
    [switch]$Release
)
$ErrorActionPreference = 'Continue'
$desktop = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $desktop
$profile_ = if ($Release) { 'release' } else { 'debug' }
$triple = "$Target-linux-android"
$abi = if ($Target -eq 'aarch64') { 'arm64-v8a' } else { 'x86_64' }
$cap = (Get-Culture).TextInfo.ToTitleCase($Target)
if ($Target -eq 'x86_64') { $cap = 'X86_64' }
$cap2 = if ($Target -eq 'aarch64') { 'Arm64' } else { 'X86_64' }

$args_ = @('tauri', 'android', 'build', '--apk', '--target', $Target)
if (-not $Release) { $args_ += '--debug' }
pnpm @args_
# On a machine with symlink rights the command above is already complete.

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$so = Join-Path $desktop "src-tauri/$targetDir/$triple/$profile_/libip_desktop_lib.so"
if (-not (Test-Path $so)) { $so = Join-Path $desktop "../../$targetDir/$triple/$profile_/libip_desktop_lib.so" }
if (-not (Test-Path $so)) { throw "libip_desktop_lib.so not found ($so)" }
$jni = Join-Path $desktop "src-tauri/gen/android/app/src/main/jniLibs/$abi"
New-Item -ItemType Directory -Force $jni | Out-Null
Copy-Item $so $jni -Force

Set-Location (Join-Path $desktop 'src-tauri/gen/android')
$flavor = $cap2
$task = "assemble$flavor" + (Get-Culture).TextInfo.ToTitleCase($profile_)
& .\gradlew.bat $task "-x" "rustBuild$flavor$((Get-Culture).TextInfo.ToTitleCase($profile_))"
Get-ChildItem -Recurse -Filter *.apk app/build/outputs | ForEach-Object { "{0}  {1:N1} MB" -f $_.FullName, ($_.Length / 1MB) }
