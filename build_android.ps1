$ndk = "$env:LOCALAPPDATA\Android\Sdk\ndk\25.1.8937393"
$tc  = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"
$cm  = "$env:LOCALAPPDATA\Android\Sdk\cmake\3.22.1\bin"
$clang = "$tc\aarch64-linux-android21-clang.cmd"

$env:CC_aarch64_linux_android  = $clang
$env:CXX_aarch64_linux_android = "$tc\aarch64-linux-android21-clang++.cmd"
$env:AR_aarch64_linux_android  = "$tc\llvm-ar.exe"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $clang

# Custom toolchain: prevents cmake-rs auto-detecting NDK's broken armv7 default
$env:CMAKE_TOOLCHAIN_FILE = "$PSScriptRoot\android_toolchain.cmake"
$env:ANDROID_NDK_HOME = $ndk

# NO CMAKE_TOOLCHAIN_FILE — let cmake-rs use its own cross-compile logic
$env:CMAKE_C_COMPILER_aarch64_linux_android = $clang
$env:CMAKE_CXX_COMPILER_aarch64_linux_android = "$tc\aarch64-linux-android21-clang++.cmd"
$env:CMAKE_GENERATOR = "Ninja"
$env:CMAKE_MAKE_PROGRAM = "$cm\ninja.exe"

# Bypass audiopus_sys CMake build for cargo check
$env:LIBOPUS_LIB_DIR = "$PSScriptRoot\target\opus-stub"
New-Item -ItemType Directory -Force -Path $env:LIBOPUS_LIB_DIR | Out-Null

$env:PATH = "$tc;$cm;$env:PATH"
Remove-Item Env:\RING_PREGENERATE_ASM -ErrorAction SilentlyContinue

Set-Location "$PSScriptRoot"
Write-Host "CC: $clang" -ForegroundColor Cyan
Write-Host "CMAKE: $cm" -ForegroundColor Cyan
Write-Host ""
cargo check -p libwebrtc --target aarch64-linux-android -j 1
exit $LASTEXITCODE
