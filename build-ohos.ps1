$ErrorActionPreference = 'Stop'

# ---- Environment ----
$env:OHOS_SDK_HOME = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\12"
$env:OHOS_NDK = "$env:OHOS_SDK_HOME\native"
$env:OHOS_SYSROOT = "$env:OHOS_NDK\sysroot"
$env:OHOS_LLVM = "$env:OHOS_NDK\llvm\bin"
$env:CC_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\clang.exe --target=aarch64-unknown-linux-ohos --sysroot=$env:OHOS_SYSROOT"
$env:CXX_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\clang++.exe --target=aarch64-unknown-linux-ohos --sysroot=$env:OHOS_SYSROOT"
$env:AR_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\llvm-ar.exe"
$env:RANLIB_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\llvm-ranlib.exe"
$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER = "$env:OHOS_LLVM\clang.exe"
$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_RUSTFLAGS = "-C link-arg=-fuse-ld=lld -C link-arg=--target=aarch64-unknown-linux-ohos -C link-arg=--sysroot=$env:OHOS_SYSROOT"
$env:CMAKE_TOOLCHAIN_FILE_aarch64_unknown_linux_ohos = "$env:OHOS_NDK\build\cmake\ohos.toolchain.cmake"
$env:CMAKE_GENERATOR = "Ninja"
$env:PATH = "$env:OHOS_LLVM;$env:OHOS_NDK\build-tools\cmake\bin;$env:PATH"

$rustSdks = "d:\tdcare\td-zt9\smartward\rust-sdks"
$ohosDir = "d:\tdcare\td-zt9\smartward\ohos"

# ---- Step 1: Build Rust native library (cargo build instead of ohrs build) ----
Write-Host "===== Step 1: Build livekit-napi-ohos (Rust -> .so) =====" -ForegroundColor Cyan
Set-Location $rustSdks
cargo build -p livekit-napi-ohos --target aarch64-unknown-linux-ohos --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# ---- Step 2: Copy liblivekit.so to all OHOS apps ----
Write-Host "`n===== Step 2: Copy liblivekit.so to OHOS apps =====" -ForegroundColor Cyan
$soSrc = "$rustSdks\target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so"
if (-not (Test-Path $soSrc)) { throw "liblivekit_napi_ohos.so not found at $soSrc" }

$soSize = "{0:F1} MB" -f ((Get-Item $soSrc).Length / 1MB)
Write-Host "Source: $soSrc ($soSize)"

$apps = @("bedside", "gate", "nursestation", "corridor")
foreach ($app in $apps) {
    $dst = "$ohosDir\$app\entry\libs\arm64-v8a\liblivekit.so"
    Copy-Item $soSrc $dst -Force
    Write-Host "  -> $app"
}

# Clean up stale libsmartward_call.so (now integrated into p2p crate, linked statically)
foreach ($app in $apps) {
    $stale = "$ohosDir\$app\entry\libs\arm64-v8a\libsmartward_call.so"
    if (Test-Path $stale) {
        Remove-Item $stale -Force
        Write-Host "  Removed stale: $app/libsmartward_call.so"
    }
}

Write-Host "`n===== Rust build complete =====" -ForegroundColor Green
Write-Host "To build HAPs, run individual build scripts in each OHOS app."
