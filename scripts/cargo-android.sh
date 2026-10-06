#!/usr/bin/env bash
# Runs cargo with the Android NDK environment of scripts/android-env.sh:
#   scripts/cargo-android.sh build -p ip-server --target aarch64-linux-android
set -e
# shellcheck source=android-env.sh
. "$(dirname "$0")/android-env.sh"
exec cargo "$@"
