# ============================================================
# livekit-jni-android 交叉编译脚本
# ============================================================
# 用途：将 livekit-jni-android 编译为 Android 动态链接库（.so）
#
# 前提条件：
#   1. 已安装 Android NDK（推荐 25.x+）
#   2. 已通过 rustup 安装 aarch64-linux-android target
#   3. 已安装 cargo-ndk: cargo install cargo-ndk
#
# 用法：
#   .\build_android.ps1              # 完整 release 构建
#   .\build_android.ps1 -CheckOnly   # 仅做 cargo check（快速验证）
#
# 产物：
#   rust-sdks/android-libs/arm64-v8a/liblivekit_jni_android.so
# ============================================================

param(
    [switch]$CheckOnly
)

$ErrorActionPreference = "Stop"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " livekit-jni-android 交叉编译" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ----------------------------------------------------------
# 步骤 1：检查环境
# ----------------------------------------------------------

Write-Host "[1/4] 检查环境..." -ForegroundColor Yellow

# NDK 路径
$ndk = $env:ANDROID_NDK_HOME
if ([string]::IsNullOrEmpty($ndk)) {
    $ndk = "$env:LOCALAPPDATA\Android\Sdk\ndk\25.1.8937393"
}
if (-not (Test-Path $ndk)) {
    Write-Host "错误：Android NDK 未找到: $ndk" -ForegroundColor Red
    Write-Host '请设置 $env:ANDROID_NDK_HOME 指向 NDK 目录' -ForegroundColor Red
    exit 1
}
Write-Host "  NDK: $ndk" -ForegroundColor Gray

$tc  = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"
$cm  = "$env:LOCALAPPDATA\Android\Sdk\cmake\3.22.1\bin"
$clang = "$tc\aarch64-linux-android21-clang.cmd"

if (-not (Test-Path $clang)) {
    Write-Host "错误：clang 未找到: $clang" -ForegroundColor Red
    exit 1
}

# 设置交叉编译环境变量
$env:CC_aarch64_linux_android  = $clang
$env:CXX_aarch64_linux_android = "$tc\aarch64-linux-android21-clang++.cmd"
$env:AR_aarch64_linux_android  = "$tc\llvm-ar.exe"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $clang

# CMake 配置（用于 C 依赖如 opus/libvpx）
# 使用 cmake 包装脚本解决 cmake-rs 0.1.58 不传递 ANDROID_ABI 的问题
$env:CMAKE = "$PSScriptRoot\cmake-android.cmd"
$env:ANDROID_NDK_HOME = $ndk
Remove-Item Env:\CMAKE_TOOLCHAIN_FILE -ErrorAction SilentlyContinue

$env:PATH = "$tc;$cm;$env:PATH"
Remove-Item Env:\RING_PREGENERATE_ASM -ErrorAction SilentlyContinue

Write-Host "  CC: $clang" -ForegroundColor Gray
Write-Host ""

# ----------------------------------------------------------
# 步骤 2：编译
# ----------------------------------------------------------

Set-Location "$PSScriptRoot"

if ($CheckOnly) {
    Write-Host "[2/4] cargo check (验证模式)..." -ForegroundColor Yellow
    cargo check -p livekit-jni-android --target aarch64-linux-android -j 1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "cargo check 失败" -ForegroundColor Red
        exit $LASTEXITCODE
    }
    Write-Host "cargo check 通过 ✓" -ForegroundColor Green
    exit 0
}

Write-Host "[2/4] cargo build --release..." -ForegroundColor Yellow
Write-Host "  这可能需要 5-15 分钟（首次编译）..." -ForegroundColor Gray

# 使用 cargo-ndk 简化构建（自动处理 linker 和 sysroot）
$hasCargoNdk = Get-Command cargo-ndk -ErrorAction SilentlyContinue
if ($hasCargoNdk) {
    cargo ndk -t arm64-v8a -o android-libs build --release -p livekit-jni-android
} else {
    # Fallback: 直接 cargo build
    cargo build --release -p livekit-jni-android --target aarch64-linux-android
}

if ($LASTEXITCODE -ne 0) {
    Write-Host "构建失败" -ForegroundColor Red
    exit $LASTEXITCODE
}

Write-Host "构建成功 ✓" -ForegroundColor Green
Write-Host ""

# ----------------------------------------------------------
# 步骤 3：收集产物
# ----------------------------------------------------------

Write-Host "[3/4] 收集产物..." -ForegroundColor Yellow

$outDir = "$PSScriptRoot\android-libs\arm64-v8a"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

# cargo-ndk 输出到 android-libs/，直接 cargo build 输出到 target/
$soSources = @(
    "$PSScriptRoot\android-libs\liblivekit_jni_android.so",
    "$PSScriptRoot\target\aarch64-linux-android\release\liblivekit_jni_android.so"
)

$copied = $false
foreach ($src in $soSources) {
    if (Test-Path $src) {
        Copy-Item $src "$outDir\liblivekit_jni_android.so" -Force
        Write-Host "  复制: $src" -ForegroundColor Gray
        $copied = $true
        break
    }
}

if (-not $copied) {
    Write-Host "警告：未找到 .so 产物" -ForegroundColor Yellow
}

# ----------------------------------------------------------
# 步骤 4：部署到 Android 模块（可选）
# ----------------------------------------------------------

Write-Host "[4/4] 部署到 Android 模块..." -ForegroundColor Yellow

$androidJniDir = "$PSScriptRoot\..\android\smartward-rust-bridge\src\main\jniLibs\arm64-v8a"
if (Test-Path "$outDir\liblivekit_jni_android.so") {
    New-Item -ItemType Directory -Force -Path $androidJniDir | Out-Null
    Copy-Item "$outDir\liblivekit_jni_android.so" $androidJniDir -Force
    Write-Host "  部署到: $androidJniDir" -ForegroundColor Gray
} else {
    Write-Host "  跳过（无产物）" -ForegroundColor Gray
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 完成！" -ForegroundColor Green
Write-Host " 产物: $outDir\liblivekit_jni_android.so" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
