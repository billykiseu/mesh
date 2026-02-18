#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "=== Mesh Android Build ==="

# 1. Ensure Rust Android targets are installed
echo "[1/3] Checking Rust Android targets..."
rustup target add aarch64-linux-android x86_64-linux-android

# 2. Build native library with cargo-ndk
echo "[2/3] Building mesh-ffi for Android..."
cargo ndk \
  -t arm64-v8a \
  -t x86_64 \
  -o mesh-android/app/src/main/jniLibs \
  build --release -p mesh-ffi

# 3. Build the APK
echo "[3/3] Building Android APK..."
cd mesh-android
if [ -f "./gradlew" ]; then
  ./gradlew assembleDebug
else
  echo "Error: gradlew not found in mesh-android/. Run this from the workspace root."
  exit 1
fi

APK_PATH="app/build/outputs/apk/debug/app-debug.apk"
if [ -f "$APK_PATH" ]; then
  echo ""
  echo "=== Build successful ==="
  echo "APK: mesh-android/$APK_PATH"
  ls -lh "$APK_PATH"
else
  echo "Build finished but APK not found at expected path."
  exit 1
fi
