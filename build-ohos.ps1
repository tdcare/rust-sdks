$ErrorActionPreference = 'Stop'

# ---- Environment ----
$env:PATH = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\12\native\llvm\bin;C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\12\native\build-tools\cmake\bin;" + $env:PATH
$env:OHOS_SDK_HOME = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\12"
$env:CMAKE_GENERATOR = "Ninja"
$env:CC_aarch64_unknown_linux_ohos = "aarch64-unknown-linux-ohos-clang.cmd"
$env:CXX_aarch64_unknown_linux_ohos = "aarch64-unknown-linux-ohos-clang++.cmd"
$env:AR_aarch64_unknown_linux_ohos = "llvm-ar"
$env:DEVECO_SDK_HOME = "C:\Program Files\Huawei\DevEco Studio\sdk"

$root = "d:\tdcare\td-zt9\smartward\rust-sdks"
$appDir = "$root\examples\ohos-livekit-app"
$hdc = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20\toolchains\hdc.exe"
$hvigorw = "C:\Program Files\Huawei\DevEco Studio\tools\hvigor\bin\hvigorw.bat"

# ---- Step 1: Build Rust native library ----
Write-Host "===== Step 1: Build Rust native library =====" -ForegroundColor Cyan
Set-Location "$root\livekit-napi-ohos"
ohrs build
if ($LASTEXITCODE -ne 0) { throw "ohrs build failed" }

# ---- Step 2: Copy .so to app libs ----
Write-Host "`n===== Step 2: Copy .so to app libs =====" -ForegroundColor Cyan
$soSrc = "$root\livekit-napi-ohos\dist\arm64-v8a\liblivekit_napi_ohos.so"
$soDst = "$appDir\libs\arm64-v8a\liblivekit.so"
if (Test-Path $soSrc) {
    Copy-Item $soSrc $soDst -Force
    Write-Host "Copied liblivekit.so ($(((Get-Item $soSrc).Length / 1MB).ToString('F1')) MB)"
} else {
    throw "liblivekit_napi_ohos.so not found at $soSrc"
}

# ---- Step 3: Build HAP with hvigorw ----
Write-Host "`n===== Step 3: Build HAP =====" -ForegroundColor Cyan
Set-Location $appDir
& $hvigorw --mode module -p module=entry@default -p product=default assembleHap --no-daemon 2>&1 | ForEach-Object { Write-Host $_ }
if ($LASTEXITCODE -ne 0) { throw "hvigorw build failed" }

# ---- Step 4: Install ----
Write-Host "`n===== Step 4: Install =====" -ForegroundColor Cyan
$signedHap = "$appDir\entry\build\default\outputs\default\entry-default-signed.hap"
if (-not (Test-Path $signedHap)) { throw "Signed HAP not found: $signedHap" }

Set-Location "C:\"
& $hdc install -r $signedHap 2>&1 | ForEach-Object { Write-Host $_ }

Write-Host "`n===== Done =====" -ForegroundColor Green
