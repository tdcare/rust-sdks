@echo off
REM cmake wrapper: injects ANDROID_ABI only for configure step (not --build)
set NDK=C:\Users\tzw\AppData\Local\Android\Sdk\ndk\25.1.8937393
set CMAKE_REAL=C:\Users\tzw\AppData\Local\Android\Sdk\cmake\3.22.1\bin\cmake.exe
set NINJA=C:\Users\tzw\AppData\Local\Android\Sdk\cmake\3.22.1\bin\ninja.exe

REM Read ANDROID_ABI from environment, default to arm64-v8a
if "%ANDROID_ABI%"=="" set ANDROID_ABI=arm64-v8a

REM Check if this is a --build invocation
if "%1"=="--build" (
    %CMAKE_REAL% %*
) else (
    %CMAKE_REAL% -G Ninja -DANDROID_ABI=%ANDROID_ABI% -DANDROID_PLATFORM=android-21 -DCMAKE_TOOLCHAIN_FILE=%NDK%\build\cmake\android.toolchain.cmake -DCMAKE_MAKE_PROGRAM=%NINJA% %*
)
