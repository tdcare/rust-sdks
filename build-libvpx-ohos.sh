#!/bin/bash
# Cross-compile libvpx for aarch64-linux-ohos (OHOS NDK)
# Step 1: configure only (make is run separately from PowerShell)
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIBVPX_SRC="$SCRIPT_DIR/libvpx"
BUILD_DIR="$SCRIPT_DIR/libvpx-build"

# OHOS NDK paths (Windows -> Git Bash path)
NDK_LLVM="/c/Users/tzw/AppData/Local/OpenHarmony/Sdk/12/native/llvm/bin"
NDK_SYSROOT="/c/Users/tzw/AppData/Local/OpenHarmony/Sdk/12/native/sysroot"

export CC="${NDK_LLVM}/clang --target=aarch64-linux-ohos --sysroot=${NDK_SYSROOT}"
export CXX="${NDK_LLVM}/clang++ --target=aarch64-linux-ohos --sysroot=${NDK_SYSROOT}"
export AR="${NDK_LLVM}/llvm-ar"
export STRIP="${NDK_LLVM}/llvm-strip"
export NM="${NDK_LLVM}/llvm-nm"
export RANLIB="${NDK_LLVM}/llvm-ranlib"
export AS="${NDK_LLVM}/clang --target=aarch64-linux-ohos --sysroot=${NDK_SYSROOT}"

echo "=== Cross-compiling libvpx for aarch64-linux-ohos ==="
echo "CC=$CC"
echo "Source: $LIBVPX_SRC"
echo "Build: $BUILD_DIR"

# Clean & setup build dir
rm -rf "$BUILD_DIR"/*
mkdir -p "$BUILD_DIR"
cd "$BUILD_DIR"

# Configure
echo ""
echo "=== Configuring... ==="
"$LIBVPX_SRC/configure" \
    --target=generic-gnu \
    --enable-vp8 \
    --disable-vp9 \
    --enable-static \
    --disable-shared \
    --enable-pic \
    --disable-examples \
    --disable-tools \
    --disable-docs \
    --disable-unit-tests \
    --disable-install-docs \
    --disable-install-bins \
    --enable-realtime-only \
    --enable-onthefly-bitpacking \
    --disable-multithread \
    --extra-cflags="-fPIC -O2" \
    --disable-runtime-cpu-detect

echo ""
echo "=== Configure done. Now run gmake from PowerShell. ==="
