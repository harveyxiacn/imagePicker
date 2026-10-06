#!/usr/bin/env bash
# Downloads the pinned `uv` release for the target triple into src-tauri/binaries/ as
# `uv-<triple>[.exe]` (the name Tauri's `bundle.externalBin: ["binaries/uv"]` expects), after
# verifying its sha256 against scripts/uv.sha256. Binaries are git-ignored, never committed.
#
#   fetch-uv.sh                         host triple (rustc -vV)
#   FETCH_UV_TARGET=<triple> fetch-uv.sh   e.g. aarch64-apple-darwin
#   FETCH_UV_TARGET=universal-apple-darwin  both mac arches + `lipo -create`
#
# Idempotent (skips when the pinned version is already there); exits non-zero on any failure,
# in particular on a checksum mismatch. Runs on Linux, macOS and Git Bash (Windows).
set -euo pipefail

UV_VERSION="0.12.23"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$HERE/../src-tauri/binaries"
SUMS="$HERE/uv.sha256"
BASE="${UV_DOWNLOAD_BASE:-https://github.com/astral-sh/uv/releases/download/$UV_VERSION}"

host_triple() {
  if command -v rustc >/dev/null 2>&1; then
    rustc -vV | sed -n 's/^host: //p'
    return
  fi
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
    Linux-aarch64) echo aarch64-unknown-linux-gnu ;;
    Darwin-arm64) echo aarch64-apple-darwin ;;
    Darwin-x86_64) echo x86_64-apple-darwin ;;
    MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) echo x86_64-pc-windows-msvc ;;
    *) echo "fetch-uv: cannot detect the host triple (install rustc or set FETCH_UV_TARGET)" >&2; exit 1 ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# fetch_one <triple>: leaves $OUT/uv-<triple>[.exe]
fetch_one() {
  local triple="$1" ext="tar.gz" exe="" bin dest archive expected got tmp
  case "$triple" in *windows*) ext="zip"; exe=".exe" ;; esac
  dest="$OUT/uv-$triple$exe"
  if [ -f "$dest" ] && [ "$(cat "$OUT/.uv-$triple.version" 2>/dev/null || true)" = "$UV_VERSION" ]; then
    echo "fetch-uv: $dest is up to date (uv $UV_VERSION)"
    return
  fi
  archive="uv-$triple.$ext"
  expected="$(awk -v f="$archive" '$2 == f { print $1 }' "$SUMS")"
  if [ -z "$expected" ]; then
    echo "fetch-uv: no pinned checksum for $archive in $SUMS" >&2
    exit 1
  fi
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  echo "fetch-uv: downloading $BASE/$archive"
  curl -fsSL --retry 3 -o "$tmp/$archive" "$BASE/$archive"
  got="$(sha256_of "$tmp/$archive")"
  if [ "$got" != "$expected" ]; then
    echo "fetch-uv: CHECKSUM MISMATCH for $archive" >&2
    echo "  expected $expected" >&2
    echo "  got      $got" >&2
    exit 1
  fi
  mkdir -p "$tmp/x"
  if [ "$ext" = "zip" ]; then
    if command -v unzip >/dev/null 2>&1; then unzip -oq "$tmp/$archive" -d "$tmp/x"
    else powershell -NoProfile -Command "Expand-Archive -Force -LiteralPath '$(cygpath -w "$tmp/$archive")' -DestinationPath '$(cygpath -w "$tmp/x")'"; fi
  else
    tar -xzf "$tmp/$archive" -C "$tmp/x"
  fi
  bin="$(find "$tmp/x" -type f -name "uv$exe" | head -n 1)"
  if [ -z "$bin" ]; then
    echo "fetch-uv: uv$exe not found in $archive" >&2
    exit 1
  fi
  mkdir -p "$OUT"
  cp "$bin" "$dest"
  chmod +x "$dest"
  echo "$UV_VERSION" > "$OUT/.uv-$triple.version"
  echo "fetch-uv: installed $dest"
}

TARGET="${FETCH_UV_TARGET:-$(host_triple)}"
mkdir -p "$OUT"
if [ "$TARGET" = "universal-apple-darwin" ]; then
  fetch_one aarch64-apple-darwin
  fetch_one x86_64-apple-darwin
  dest="$OUT/uv-universal-apple-darwin"
  if [ -f "$dest" ] && [ "$(cat "$OUT/.uv-universal-apple-darwin.version" 2>/dev/null || true)" = "$UV_VERSION" ]; then
    echo "fetch-uv: $dest is up to date (uv $UV_VERSION)"
  else
    lipo -create -output "$dest" "$OUT/uv-aarch64-apple-darwin" "$OUT/uv-x86_64-apple-darwin"
    chmod +x "$dest"
    echo "$UV_VERSION" > "$OUT/.uv-universal-apple-darwin.version"
    echo "fetch-uv: created $dest"
  fi
else
  fetch_one "$TARGET"
fi
