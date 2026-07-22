$ndk = "C:\Users\tzw\AppData\Local\Android\Sdk\ndk\25.1.8937393"
$tc = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"
$cm = "$env:LOCALAPPDATA\Android\Sdk\cmake\3.22.1\bin"
$env:PATH = "$tc;$cm;$env:PATH"
$env:ANDROID_NDK_HOME = $ndk
# 使用 cmake 包装脚本，自动注入 ANDROID_ABI=arm64-v8a
$env:CMAKE = "d:\tdcare\td-zt9\smartward\rust-sdks\cmake-android.cmd"
Remove-Item Env:\CMAKE_TOOLCHAIN_FILE -ErrorAction SilentlyContinue
Remove-Item Env:\RING_PREGENERATE_ASM -ErrorAction SilentlyContinue

Set-Location "d:\tdcare\td-zt9\smartward\rust-sdks"
$env:CC_aarch64_linux_android = "$tc\aarch64-linux-android21-clang.cmd"
$env:CXX_aarch64_linux_android = "$tc\aarch64-linux-android21-clang++.cmd"
$env:AR_aarch64_linux_android = "$tc\llvm-ar.exe"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$tc\aarch64-linux-android21-clang.cmd"
cargo check -p livekit-jni-android --target aarch64-linux-android 2>&1
