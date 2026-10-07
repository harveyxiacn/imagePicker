#!/usr/bin/env bash
# Downloads the YuNet face detector (OpenCV Zoo, ~230 KB) used by the AI worker's `faces`
# step and by the optional `ip-infer` crate.
# Usage: scripts/fetch-yunet.sh [target-dir]   (default: ./models)
set -euo pipefail

DIR="${1:-models}"
FILE="face_detection_yunet_2023mar.onnx"
URL="https://huggingface.co/opencv/face_detection_yunet/resolve/main/${FILE}"
SHA256="8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4"

mkdir -p "$DIR"
OUT="$(cd "$DIR" && pwd)/$FILE"
if [ ! -f "$OUT" ]; then
  curl -fsSL --retry 3 -o "$OUT.part" "$URL"
  mv "$OUT.part" "$OUT"
fi
echo "$SHA256  $OUT" | sha256sum -c - >/dev/null || { echo "checksum mismatch: $OUT" >&2; exit 1; }
echo "YuNet ready: $OUT"
echo "export IMAGEPICKER_YUNET_MODEL=\"$OUT\""
