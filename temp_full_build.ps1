$ErrorActionPreference = "Continue"

$env:OHOS_SDK_HOME = "$env:LOCALAPPDATA\OpenHarmony\Sdk\20"
$env:OHOS_NDK_HOME = "$env:OHOS_SDK_HOME\native"
$env:OHOS_NDK = "$env:OHOS_SDK_HOME\native"
$env:OHOS_SYSROOT = "$env:OHOS_NDK\sysroot"
$env:OHOS_LLVM = "$env:OHOS_NDK\llvm\bin"
$env:CC_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\clang.exe"
$env:CXX_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\clang++.exe"
$env:CFLAGS_aarch64_unknown_linux_ohos = "--target=aarch64-unknown-linux-ohos --sysroot=$env:OHOS_SYSROOT"
$env:CXXFLAGS_aarch64_unknown_linux_ohos = "--target=aarch64-unknown-linux-ohos --sysroot=$env:OHOS_SYSROOT"
$env:AR_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\llvm-ar.exe"
$env:RANLIB_aarch64_unknown_linux_ohos = "$env:OHOS_LLVM\llvm-ranlib.exe"
$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER = "$env:OHOS_LLVM\clang.exe"
$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_RUSTFLAGS = "-C link-arg=-fuse-ld=lld -C link-arg=--target=aarch64-unknown-linux-ohos -C link-arg=--sysroot=$env:OHOS_SYSROOT"
$env:CMAKE_C_COMPILER = "$env:OHOS_LLVM\clang.exe"
$env:CMAKE_CXX_COMPILER = "$env:OHOS_LLVM\clang++.exe"
$env:CMAKE_GENERATOR = "Ninja"
$env:PATH = "$env:OHOS_LLVM;$env:OHOS_NDK\build-tools\cmake\bin;$env:PATH"

Set-Location d:\tdcare\livekit\rust-sdks
Write-Host "=== cargo build ===" -ForegroundColor Cyan
cargo build -p livekit-napi-ohos --target aarch64-unknown-linux-ohos --release 2>&1 | Select-String "error|Compiling livekit-napi|Finished" | Select-Object -Last 10
if ($LASTEXITCODE -ne 0) { Write-Host "RUST BUILD FAILED" -ForegroundColor Red; exit 1 }

Write-Host "=== copy .so ===" -ForegroundColor Cyan
Copy-Item "target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so" "examples\ohos-livekit-app\entry\libs\arm64-v8a\liblivekit.so" -Force
Copy-Item "target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so" "examples\ohos-livekit-app\libs\arm64-v8a\liblivekit.so" -Force
Write-Host "Copied .so files"

Write-Host "=== build HAP ===" -ForegroundColor Cyan
$env:JAVA_HOME = "C:\Program Files\Huawei\DevEco Studio\jbr"
$env:PATH = "$env:JAVA_HOME\bin;$env:PATH"
Set-Location "d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app"
D:\tools\command-line-tools\bin\hvigorw.bat assembleHap --mode module -p product=default --no-daemon 2>&1 | Select-Object -Last 5

Write-Host "=== install ===" -ForegroundColor Cyan
Set-Location "entry\build\default\outputs\default"
$hdc = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20\toolchains\hdc.exe"
& $hdc uninstall com.livekit.ohos.demo 2>&1
& $hdc install entry-default-signed.hap 2>&1
& $hdc shell aa start -a EntryAbility -b com.livekit.ohos.demo 2>&1
