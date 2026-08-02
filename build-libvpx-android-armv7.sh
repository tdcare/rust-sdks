#!/bin/bash
# Cross-compile libvpx for armv7-android (Android NDK 25.1.8937393)
# 与 build-libvpx-ohos.sh 同策略：--target=generic-gnu 纯 C 实现 + 交叉编译器指定目标架构
# 用法: git-bash 中执行 ./build-libvpx-android-armv7.sh
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# 注意：NDK make.exe 是 Windows 原生程序，不识别 MSYS 路径（/d/...），
# 因此传给 configure 的源码路径必须用 Windows 风格（D:/...），否则 Makefile 中的
# SRC_PATH 会变成 /d/... 导致 make 找不到 libs.mk。
LIBVPX_SRC="D:/tdcare/td-zt9/smartward/rust-sdks/libvpx"
BUILD_DIR="$SCRIPT_DIR/libvpx-build-armv7"

# Android NDK paths (Windows -> Git Bash path)
NDK_LLVM="/c/Users/tzw/AppData/Local/Android/Sdk/ndk/25.1.8937393/toolchains/llvm/prebuilt/windows-x86_64/bin"

export CC="${NDK_LLVM}/clang --target=armv7a-linux-androideabi21"
export CXX="${NDK_LLVM}/clang++ --target=armv7a-linux-androideabi21"
export AR="${NDK_LLVM}/llvm-ar"
export STRIP="${NDK_LLVM}/llvm-strip"
export NM="${NDK_LLVM}/llvm-nm"
export RANLIB="${NDK_LLVM}/llvm-ranlib"
export AS="${NDK_LLVM}/clang --target=armv7a-linux-androideabi21"

echo "=== Cross-compiling libvpx for armv7-android ==="
echo "CC=$CC"
echo "Source: $LIBVPX_SRC"
echo "Build: $BUILD_DIR"

# Clean & setup build dir
rm -rf "$BUILD_DIR"
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
echo "=== Building (make) ==="
MAKE_EXE="/c/Users/tzw/AppData/Local/Android/Sdk/ndk/25.1.8937393/prebuilt/windows-x86_64/bin/make.exe"
"$MAKE_EXE" -j8

echo ""
echo "=== Done: $BUILD_DIR/libvpx.a ==="
ls -la "$BUILD_DIR/libvpx.a"
