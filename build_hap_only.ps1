$ErrorActionPreference = "Continue"

$env:OHOS_SDK_HOME = "$env:LOCALAPPDATA\OpenHarmony\Sdk\20"
$env:JAVA_HOME = "C:\Program Files\Huawei\DevEco Studio\jbr"
$env:PATH = "$env:JAVA_HOME\bin;$env:PATH"

Set-Location "d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app"
Write-Host "=== Building HAP ===" -ForegroundColor Cyan
D:\tools\command-line-tools\bin\hvigorw.bat assembleHap --mode module -p product=default --no-daemon 2>&1 | Select-Object -Last 30

Write-Host ""
Write-Host "=== Checking output ===" -ForegroundColor Cyan
$hapPath = "entry\build\default\outputs\default\entry-default-signed.hap"
if (Test-Path $hapPath) {
    Write-Host "HAP found: $hapPath" -ForegroundColor Green
    Get-Item $hapPath | Select-Object Name, Length, LastWriteTime
} else {
    Write-Host "HAP NOT FOUND at $hapPath" -ForegroundColor Red
    Get-ChildItem "entry\build\default\outputs" -Recurse -Filter "*.hap" 2>$null | ForEach-Object { Write-Host "  Found: $($_.FullName)" }
}
