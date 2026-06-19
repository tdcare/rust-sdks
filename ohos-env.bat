@echo off
REM ============================================================================
REM OHOS cross-compilation environment setup for Windows (cmd / PowerShell)
REM ============================================================================
REM Usage:
REM   1) Set OHOS_SDK_HOME to your OpenHarmony SDK version root.
REM      Example (SDK 20):
REM        set OHOS_SDK_HOME=%LOCALAPPDATA%\OpenHarmony\Sdk\20
REM   2) Run this script before invoking cargo:
REM        call ohos-env.bat
REM   3) Build:
REM        cargo build --target aarch64-unknown-linux-ohos -p livekit
REM
REM What this script does:
REM   - Wires CC / CXX / AR / RANLIB env vars that ring (and other -sys crates)
REM     consume via the cc-rs build helper.
REM   - Adds the LLVM toolchain to PATH so cargo can locate clang.
REM ============================================================================

if "%OHOS_SDK_HOME%"=="" (
    echo [ohos-env] ERROR: OHOS_SDK_HOME is not set.
    echo [ohos-env] Example: set OHOS_SDK_HOME=%%LOCALAPPDATA%%\OpenHarmony\Sdk\20
    exit /b 1
)

if not exist "%OHOS_SDK_HOME%\native\llvm\bin\clang.exe" (
    echo [ohos-env] ERROR: clang.exe not found under "%OHOS_SDK_HOME%\native\llvm\bin".
    echo [ohos-env] Verify OHOS_SDK_HOME points at a valid OpenHarmony SDK version root.
    exit /b 1
)

set "OHOS_NDK=%OHOS_SDK_HOME%\native"
set "OHOS_SYSROOT=%OHOS_NDK%\sysroot"
set "OHOS_LLVM=%OHOS_NDK%\llvm\bin"

REM --- aarch64 ---------------------------------------------------------------
set "CC_aarch64_unknown_linux_ohos=%OHOS_LLVM%\clang.exe --target=aarch64-unknown-linux-ohos --sysroot=%OHOS_SYSROOT%"
set "CXX_aarch64_unknown_linux_ohos=%OHOS_LLVM%\clang++.exe --target=aarch64-unknown-linux-ohos --sysroot=%OHOS_SYSROOT%"
set "AR_aarch64_unknown_linux_ohos=%OHOS_LLVM%\llvm-ar.exe"
set "RANLIB_aarch64_unknown_linux_ohos=%OHOS_LLVM%\llvm-ranlib.exe"
set "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER=%OHOS_LLVM%\clang.exe"

REM --- armv7 (32-bit) --------------------------------------------------------
set "CC_armv7_unknown_linux_ohos=%OHOS_LLVM%\clang.exe --target=armv7-unknown-linux-ohos --sysroot=%OHOS_SYSROOT%"
set "CXX_armv7_unknown_linux_ohos=%OHOS_LLVM%\clang++.exe --target=armv7-unknown-linux-ohos --sysroot=%OHOS_SYSROOT%"
set "AR_armv7_unknown_linux_ohos=%OHOS_LLVM%\llvm-ar.exe"
set "RANLIB_armv7_unknown_linux_ohos=%OHOS_LLVM%\llvm-ranlib.exe"
set "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_OHOS_LINKER=%OHOS_LLVM%\clang.exe"

REM Make the toolchain itself discoverable.
set "PATH=%OHOS_LLVM%;%PATH%"

echo [ohos-env] OHOS cross-compilation environment configured.
echo [ohos-env]   OHOS_SDK_HOME = %OHOS_SDK_HOME%
echo [ohos-env]   CC (aarch64)  = %CC_aarch64_unknown_linux_ohos%
echo.
echo Run: cargo check --target aarch64-unknown-linux-ohos -p livekit
