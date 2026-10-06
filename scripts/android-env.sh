#!/usr/bin/env bash
# Source this file to cross-compile the Rust core for Android:
#   source scripts/android-env.sh
#   cargo build -p ip-server --target aarch64-linux-android
# Finds the NDK (ANDROID_NDK_HOME / ANDROID_NDK_ROOT, else <SDK>/ndk/<newest>) and exports the
# per-target linker / CC / AR variables (see docs/android-build.md). Nothing user-specific is
# committed: every path is discovered from the environment.

_ip_ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [ -z "$_ip_ndk" ]; then
  for _sdk in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "${LOCALAPPDATA:-}/Android/Sdk" "$HOME/Android/Sdk" "$HOME/Library/Android/sdk"; do
    if [ -n "$_sdk" ] && [ -d "$_sdk/ndk" ]; then
      _ip_ndk="$(ls -d "$_sdk"/ndk/*/ 2>/dev/null | sort -V | tail -1)"
      [ -n "$_ip_ndk" ] && break
    fi
  done
fi
if [ -z "$_ip_ndk" ] || [ ! -d "$_ip_ndk" ]; then
  echo "android-env: NDK not found; set ANDROID_NDK_HOME" >&2
  return 1 2>/dev/null || exit 1
fi
if command -v cygpath >/dev/null 2>&1; then _ip_ndk="$(cygpath -u "$_ip_ndk")"; fi
_ip_ndk="${_ip_ndk%/}"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) _ip_host=windows-x86_64; _ip_ext=.cmd; _ip_exe=.exe ;;
  Darwin) _ip_host=darwin-x86_64; _ip_ext=; _ip_exe= ;;
  *) _ip_host=linux-x86_64; _ip_ext=; _ip_exe= ;;
esac
_ip_bin="$_ip_ndk/toolchains/llvm/prebuilt/$_ip_host/bin"
_ip_api="${ANDROID_API:-24}"
export ANDROID_NDK_HOME="$_ip_ndk"
export PATH="$_ip_bin:$PATH"

for _triple in aarch64-linux-android x86_64-linux-android; do
  _under="${_triple//-/_}"
  _upper="$(echo "$_under" | tr a-z A-Z)"
  _clang="$_ip_bin/${_triple}${_ip_api}-clang${_ip_ext}"
  export "CC_${_under}=$_clang"
  export "CXX_${_under}=$_ip_bin/${_triple}${_ip_api}-clang++${_ip_ext}"
  export "AR_${_under}=$_ip_bin/llvm-ar${_ip_exe}"
  export "CARGO_TARGET_${_upper}_LINKER=$_clang"
  export "CARGO_TARGET_${_upper}_AR=$_ip_bin/llvm-ar${_ip_exe}"
done
echo "android-env: NDK $_ip_ndk (API $_ip_api)"
unset _ip_ndk _sdk _ip_host _ip_ext _ip_exe _ip_bin _ip_api _triple _under _upper _clang
