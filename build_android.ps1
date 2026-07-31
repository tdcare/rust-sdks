# ============================================================
# livekit-jni-android 交叉编译脚本
# ============================================================
# 用途：将 livekit-jni-android 编译为 Android 动态链接库（.so）
#
# 前提条件：
#   1. 已安装 Android NDK（推荐 25.x+）
#   2. 已通过 rustup 安装所需 target（aarch64-linux-android / x86_64-linux-android）
#   3. 已安装 cargo-ndk: cargo install cargo-ndk
#
# 用法：
#   .\build_android.ps1                        # 编译所有 target (arm64-v8a + armeabi-v7a + x86_64)
#   .\build_android.ps1 -Target arm64-v8a      # 仅编译 arm64-v8a
#   .\build_android.ps1 -Target armeabi-v7a    # 仅编译 armeabi-v7a (32位 ARM)
#   .\build_android.ps1 -Target x86_64         # 仅编译 x86_64
#   .\build_android.ps1 -CheckOnly             # 仅做 cargo check（快速验证）
#
# 产物：
#   rust-sdks/android-libs/arm64-v8a/liblivekit_jni_android.so
#   rust-sdks/android-libs/x86_64/liblivekit_jni_android.so
# ============================================================

param(
    [ValidateSet("arm64-v8a", "armeabi-v7a", "x86_64", "all")]
    [string]$Target = "all",
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

# 验证所需 clang 存在
$requiredClangs = @()
if ($Target -eq "all" -or $Target -eq "arm64-v8a") {
    $requiredClangs += "$tc\aarch64-linux-android21-clang.cmd"
}
if ($Target -eq "all" -or $Target -eq "armeabi-v7a") {
    $requiredClangs += "$tc\armv7a-linux-androideabi21-clang.cmd"
}
if ($Target -eq "all" -or $Target -eq "x86_64") {
    $requiredClangs += "$tc\x86_64-linux-android21-clang.cmd"
}

foreach ($c in $requiredClangs) {
    if (-not (Test-Path $c)) {
        Write-Host "错误：clang 未找到: $c" -ForegroundColor Red
        exit 1
    }
}

# 设置所有 target 的交叉编译环境变量
# aarch64 (arm64-v8a)
$env:CC_aarch64_linux_android  = "$tc\aarch64-linux-android21-clang.cmd"
$env:CXX_aarch64_linux_android = "$tc\aarch64-linux-android21-clang++.cmd"
$env:AR_aarch64_linux_android  = "$tc\llvm-ar.exe"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$tc\aarch64-linux-android21-clang.cmd"

# armv7 (armeabi-v7a)
$env:CC_armv7_linux_androideabi  = "$tc\armv7a-linux-androideabi21-clang.cmd"
$env:CXX_armv7_linux_androideabi = "$tc\armv7a-linux-androideabi21-clang++.cmd"
$env:AR_armv7_linux_androideabi  = "$tc\llvm-ar.exe"
$env:CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER = "$tc\armv7a-linux-androideabi21-clang.cmd"

# x86_64
$env:CC_x86_64_linux_android  = "$tc\x86_64-linux-android21-clang.cmd"
$env:CXX_x86_64_linux_android = "$tc\x86_64-linux-android21-clang++.cmd"
$env:AR_x86_64_linux_android  = "$tc\llvm-ar.exe"
$env:CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER = "$tc\x86_64-linux-android21-clang.cmd"

# CMake 配置（用于 C 依赖如 opus/libvpx）
# 使用 cmake 包装脚本解决 cmake-rs 0.1.58 不传递 ANDROID_ABI 的问题
$env:CMAKE = "$PSScriptRoot\cmake-android.cmd"
$env:ANDROID_NDK_HOME = $ndk
Remove-Item Env:\CMAKE_TOOLCHAIN_FILE -ErrorAction SilentlyContinue

$env:PATH = "$tc;$cm;$env:PATH"
Remove-Item Env:\RING_PREGENERATE_ASM -ErrorAction SilentlyContinue

Write-Host "  CC (aarch64): $($env:CC_aarch64_linux_android)" -ForegroundColor Gray
Write-Host "  CC (armv7):   $($env:CC_armv7_linux_androideabi)" -ForegroundColor Gray
Write-Host "  CC (x86_64):  $($env:CC_x86_64_linux_android)" -ForegroundColor Gray
Write-Host ""

# ----------------------------------------------------------
# 构建目标列表
# ----------------------------------------------------------

$buildTargets = @()
if ($Target -eq "all" -or $Target -eq "arm64-v8a") {
    $buildTargets += @{
        Abi         = "arm64-v8a"
        RustTarget  = "aarch64-linux-android"
        NdkAbi      = "arm64-v8a"
        JniLibsDir  = "arm64-v8a"
    }
}
if ($Target -eq "all" -or $Target -eq "armeabi-v7a") {
    $buildTargets += @{
        Abi         = "armeabi-v7a"
        RustTarget  = "armv7-linux-androideabi"
        NdkAbi      = "armeabi-v7a"
        JniLibsDir  = "armeabi-v7a"
    }
}
if ($Target -eq "all" -or $Target -eq "x86_64") {
    $buildTargets += @{
        Abi         = "x86_64"
        RustTarget  = "x86_64-linux-android"
        NdkAbi      = "x86_64"
        JniLibsDir  = "x86_64"
    }
}

# ----------------------------------------------------------
# 步骤 2：编译
# ----------------------------------------------------------

Set-Location "$PSScriptRoot"

$hasCargoNdk = $null -ne (Get-Command cargo-ndk -ErrorAction SilentlyContinue)

foreach ($t in $buildTargets) {
    $abi        = $t.Abi
    $rustTarget = $t.RustTarget
    $ndkAbi     = $t.NdkAbi

    Write-Host "" -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host " 编译 target: $abi ($rustTarget)" -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan

    # 设置 ANDROID_ABI 供 cmake-android.cmd 使用
    $env:ANDROID_ABI = $ndkAbi

    if ($CheckOnly) {
        Write-Host "[2/4] cargo check (验证模式) [$abi]..." -ForegroundColor Yellow
        cargo check -p livekit-jni-android --target $rustTarget -j 1
        if ($LASTEXITCODE -ne 0) {
            Write-Host "cargo check 失败 [$abi]" -ForegroundColor Red
            exit $LASTEXITCODE
        }
        Write-Host "cargo check 通过 [$abi] ✓" -ForegroundColor Green
        continue
    }

    Write-Host "[2/4] cargo build --release [$abi]..." -ForegroundColor Yellow
    Write-Host "  这可能需要 5-15 分钟（首次编译）..." -ForegroundColor Gray

    if ($hasCargoNdk) {
        $featureFlags = ""
        if ($abi -eq "armeabi-v7a") {
            # armv7: disable sonora AEC (cpufeatures 0.3 unsupported)
            $featureFlags = "--no-default-features"
        }
        cargo ndk -t $ndkAbi -o android-libs build --release -p livekit-jni-android $featureFlags.Split(" ", [System.StringSplitOptions]::RemoveEmptyEntries)
    } else {
        $featureFlags = ""
        if ($abi -eq "armeabi-v7a") {
            $featureFlags = "--no-default-features"
        }
        cargo build --release -p livekit-jni-android --target $rustTarget $featureFlags.Split(" ", [System.StringSplitOptions]::RemoveEmptyEntries)
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Host "构建失败 [$abi]" -ForegroundColor Red
        exit $LASTEXITCODE
    }

    Write-Host "构建成功 [$abi] ✓" -ForegroundColor Green

    # ----------------------------------------------------------
    # 步骤 3：收集产物
    # ----------------------------------------------------------

    Write-Host "[3/4] 收集产物 [$abi]..." -ForegroundColor Yellow

    $outDir = "$PSScriptRoot\android-libs\$($t.JniLibsDir)"
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null

    $soSources = @(
        "$PSScriptRoot\android-libs\liblivekit_jni_android.so",
        "$PSScriptRoot\target\$rustTarget\release\liblivekit_jni_android.so"
    )

    $copied = $false
    foreach ($src in $soSources) {
        if (Test-Path $src) {
            Copy-Item $src "$outDir\liblivekit_jni_android.so" -Force
            Write-Host "  复制: $src -> $outDir" -ForegroundColor Gray
            $copied = $true
            break
        }
    }

    if (-not $copied) {
        Write-Host "警告：未找到 .so 产物 [$abi]" -ForegroundColor Yellow
    }

    # ----------------------------------------------------------
    # 步骤 4：部署到 Android 模块
    # ----------------------------------------------------------

    Write-Host "[4/4] 部署到 Android 模块 [$abi]..." -ForegroundColor Yellow

    $androidJniDir = "$PSScriptRoot\..\android\smartward-rust-bridge\src\main\jniLibs\$($t.JniLibsDir)"
    if (Test-Path "$outDir\liblivekit_jni_android.so") {
        New-Item -ItemType Directory -Force -Path $androidJniDir | Out-Null
        Copy-Item "$outDir\liblivekit_jni_android.so" $androidJniDir -Force
        Write-Host "  部署到: $androidJniDir" -ForegroundColor Gray
    } else {
        Write-Host "  跳过（无产物）" -ForegroundColor Gray
    }
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 完成！" -ForegroundColor Green
foreach ($t in $buildTargets) {
    Write-Host " 产物: android-libs\$($t.JniLibsDir)\liblivekit_jni_android.so" -ForegroundColor Cyan
}
Write-Host "========================================" -ForegroundColor Cyan
