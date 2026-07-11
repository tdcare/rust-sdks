@echo off
set HDC="C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20\toolchains\hdc.exe"
set SIGNED_HAP=d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app\entry\build\default\outputs\default\entry-default-signed.hap

echo ===== Installing HAP =====
cd /d C:\
%HDC% install -r "%SIGNED_HAP%"
