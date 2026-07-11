cp "d:\tdcare\livekit\rust-sdks\target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so" "d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app\entry\libs\arm64-v8a\liblivekit.so"
cp "d:\tdcare\livekit\rust-sdks\target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so" "d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app\libs\arm64-v8a\liblivekit.so"

$env:JAVA_HOME = "C:\Program Files\Huawei\DevEco Studio\jbr"
$env:PATH = "$env:JAVA_HOME\bin;$env:PATH"
Set-Location "d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app"
D:\tools\command-line-tools\bin\hvigorw.bat assembleHap --mode module -p product=default --no-daemon 2>&1 | Select-Object -Last 3

Set-Location "entry\build\default\outputs\default"
$hdc = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20\toolchains\hdc.exe"
& $hdc uninstall com.livekit.ohos.demo 2>&1
& $hdc install entry-default-signed.hap 2>&1
& $hdc shell aa start -a EntryAbility -b com.livekit.ohos.demo 2>&1
