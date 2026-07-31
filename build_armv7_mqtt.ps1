# Build rumqttc + rumqttd for armeabi-v7a
$ErrorActionPreference = "Stop"
$ndk = "$env:LOCALAPPDATA\Android\Sdk\ndk\25.1.8937393"
$tc = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"

$env:CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER = "$tc\armv7a-linux-androideabi21-clang.cmd"
$env:CC_armv7_linux_androideabi = "$tc\armv7a-linux-androideabi21-clang.cmd"
$env:CXX_armv7_linux_androideabi = "$tc\armv7a-linux-androideabi21-clang++.cmd"
$env:AR_armv7_linux_androideabi = "$tc\llvm-ar.exe"
$env:PATH = "$tc;$env:PATH"

$target = "armv7-linux-androideabi"
$dest = "D:\tdcare\td-zt9\smartward\android\smartward-rust-bridge\src\main\jniLibs\armeabi-v7a"
New-Item -ItemType Directory -Force -Path $dest | Out-Null

# 1. rumqttc-android
Write-Host "=== Building rumqttc-android (armeabi-v7a) ===" -ForegroundColor Cyan
Set-Location "D:\tdcare\td-zt9\smartward\rumqtt\rumqttc\src\android"
cargo build --release --target $target
if ($LASTEXITCODE -ne 0) { Write-Host "rumqttc-android FAILED" -ForegroundColor Red; exit 1 }
Copy-Item "target\$target\release\librumqttc_android.so" $dest -Force
Write-Host "rumqttc-android OK -> $dest" -ForegroundColor Green

# 2. rumqttd
Write-Host "=== Building rumqttd (armeabi-v7a) ===" -ForegroundColor Cyan
Set-Location "D:\tdcare\td-zt9\smartward\rumqtt"
cargo build --release -p rumqttd --target $target
if ($LASTEXITCODE -ne 0) { Write-Host "rumqttd FAILED" -ForegroundColor Red; exit 1 }
Copy-Item "target\$target\release\librumqttd.so" $dest -Force
Write-Host "rumqttd OK -> $dest" -ForegroundColor Green

Write-Host "`n=== All done! ===" -ForegroundColor Green
Get-ChildItem $dest | Format-Table Name, Length
