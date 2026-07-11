# OHOS (OpenHarmony) Build Guide

## Prerequisites

- Rust toolchain with OHOS target support
- OpenHarmony SDK (version 18 or 20 recommended)
- OHOS NDK toolchain (LLVM bundled with the SDK)

## Environment Setup

### 1. Install Rust OHOS Target

```bash
rustup target add aarch64-unknown-linux-ohos
# Optional 32-bit target
rustup target add armv7-unknown-linux-ohos
```

### 2. Configure environment variables

The OHOS build uses pure-Rust dependencies that still require a C
compiler (`ring`, `getrandom`, etc.). The variables below tell `cc-rs`
to invoke the OHOS SDK's `clang` with the right `--target` and
`--sysroot` so cross-compilation succeeds.

#### Windows (PowerShell)

```powershell
# Adjust to your installed SDK version (10..20).
$env:OHOS_SDK_HOME = "$env:LOCALAPPDATA\OpenHarmony\Sdk\20"

$llvm    = "$env:OHOS_SDK_HOME\native\llvm\bin"
$sysroot = "$env:OHOS_SDK_HOME\native\sysroot"

$env:CC_aarch64_unknown_linux_ohos  = "$llvm\clang.exe --target=aarch64-unknown-linux-ohos --sysroot=$sysroot"
$env:CXX_aarch64_unknown_linux_ohos = "$llvm\clang++.exe --target=aarch64-unknown-linux-ohos --sysroot=$sysroot"
$env:AR_aarch64_unknown_linux_ohos     = "$llvm\llvm-ar.exe"
$env:RANLIB_aarch64_unknown_linux_ohos = "$llvm\llvm-ranlib.exe"
$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER = "$llvm\clang.exe"
```

A convenience batch script is provided for `cmd.exe`:

```cmd
set OHOS_SDK_HOME=%LOCALAPPDATA%\OpenHarmony\Sdk\20
call ohos-env.bat
```

#### Linux / macOS (bash / zsh)

```bash
export OHOS_SDK_HOME=/path/to/ohos-sdk/20
LLVM=$OHOS_SDK_HOME/native/llvm/bin
SYSROOT=$OHOS_SDK_HOME/native/sysroot

export CC_aarch64_unknown_linux_ohos="$LLVM/clang --target=aarch64-unknown-linux-ohos --sysroot=$SYSROOT"
export CXX_aarch64_unknown_linux_ohos="$LLVM/clang++ --target=aarch64-unknown-linux-ohos --sysroot=$SYSROOT"
export AR_aarch64_unknown_linux_ohos="$LLVM/llvm-ar"
export RANLIB_aarch64_unknown_linux_ohos="$LLVM/llvm-ranlib"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="$LLVM/clang"
export PATH="$LLVM:$PATH"
```

> The variable names use underscores (`aarch64_unknown_linux_ohos`),
> matching the casing that `cc-rs` looks up first.

### 3. Build libvpx for OHOS (if needed)

Pre-built libraries are provided in `libvpx-build/`. To rebuild from source:

```bash
./build-libvpx-ohos.sh
```

## Building

### Build libwebrtc (OHOS backend)

```bash
cargo check --target aarch64-unknown-linux-ohos -p libwebrtc
```

### Build the full livekit SDK for OHOS

```bash
cargo check --target aarch64-unknown-linux-ohos -p livekit
```

`cargo build` works the same way; `check` is recommended for quick
type-checking iterations.

## Architecture

The OHOS WebRTC implementation uses:

- **webrtc-rs/rtc**: Pure Rust WebRTC protocol stack (ICE, DTLS, SRTP, SCTP)
- **OH_AVCodec**: OHOS native hardware video encoder/decoder
- **libvpx**: Software VP8 codec (fallback)

## Notes

- The `webrtc-sys` crate is **not** compiled for OHOS targets; the
  `libwebrtc` crate switches to its `ohos/` backend via
  `#[cfg(target_env = "ohos")]`.
- The `glib` dependency (used for the GDBus / XDG portal integration on
  desktop Linux) is excluded on OHOS — the screen-capture / portal code
  paths are not exposed there.
- `libwebrtc::native::create_random_uuid()` is reimplemented in pure
  Rust on OHOS (no FFI), so existing callers in the public layer keep
  working without target-specific branches.
- Hardware codec availability depends on the device. VP8 software
  encoding is always available as a fallback.
- `ring` (transitive via `reqwest -> rustls` and `rtc -> rcgen`)
  requires a working C cross-compiler. Without the env vars from
  step 2, `cargo` fails with `failed to find tool "cc"`.
