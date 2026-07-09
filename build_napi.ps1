$env:OHOS_SDK_HOME = "$env:LOCALAPPDATA\OpenHarmony\Sdk\20"
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

Write-Host "===== Build livekit-napi-ohos (SDK 20) =====" -ForegroundColor Cyan
Set-Location $rustSdks
cargo build -p livekit-napi-ohos --target aarch64-unknown-linux-ohos --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Copy liblivekit.so to all OHOS apps
$soSrc = "$rustSdks\target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so"
if (-not (Test-Path $soSrc)) { throw "liblivekit_napi_ohos.so not found at $soSrc" }

Write-Host "Copying liblivekit.so to OHOS apps..." -ForegroundColor Cyan
$apps = @("bedside", "gate", "nursestation", "corridor")
foreach ($app in $apps) {
    $dst = "$ohosDir\$app\entry\libs\arm64-v8a\liblivekit.so"
    Copy-Item $soSrc $dst -Force
    Write-Host "  -> $app"
}

# Clean stale libsmartward_call.so
foreach ($app in $apps) {
    $stale = "$ohosDir\$app\entry\libs\arm64-v8a\libsmartward_call.so"
    if (Test-Path $stale) {
        Remove-Item $stale -Force
        Write-Host "  Removed stale: $app/libsmartward_call.so"
    }
}

Write-Host "===== Done =====" -ForegroundColor Green
$env:OHOS_SDK_HOME = "$env:LOCALAPPDATA\OpenHarmony\Sdk\20"
$env:OHOS_NDK = "$env:OHOS_SDK_HOME\native"
$env:OHOS_SYSROOT = "$env:OHOS_NDK\sysroot"
$env:OHOS_LLVM = "$env:OHOS_NDK\llvm\bin"
$env:CC_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\clang.exe --target=aarch64-unknown-linux-ohos --sysroot=$env:OHOS_SYSROOT"
$env:CXX_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\clang++.exe --target=aarch64-unknown-linux-ohos --sysroot=$env:OHOS_SYSROOT"
$env:AR_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\llvm-ar.exe"
$env:RANLIB_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\llvm-ranlib.exe"
$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER = "$env:OHOS_LLVM\clang.exe"
$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_RUSTFLAGS = "-C link-arg=-fuse-ld=lld -C link-arg=--target=aarch64-unknown-linux-ohos -C link-arg=--sysroot=$env:OHOS_SYSROOT"
$env:PATH = "$env:OHOS_LLVM;$env:PATH"

Set-Location d:\tdcare\td-zt9\smartward\rust-sdks
cargo build -p livekit-napi-ohos --target aarch64-unknown-linux-ohos --release
