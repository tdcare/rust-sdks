# rust-sdks OHOS 编译安装启动指南

## 一键构建 (完整管道)

执行 `d:\tdcare\livekit\rust-sdks\temp_full_build.ps1` 自动完成以下 4 个阶段。

### 构建管道四阶段

```
┌─────────────────────────────────────────────────────────────┐
│ 1. 设置 OHOS 交叉编译环境                                    │
│    OHOS_SDK_HOME=.../Sdk/20  →  CC/CXX/AR/RANLIB/LINKER   │
├─────────────────────────────────────────────────────────────┤
│ 2. Cargo 交叉编译                                            │
│    cargo build -p livekit-napi-ohos --target aarch64-...   │
│    → target/aarch64-unknown-linux-ohos/release/              │
│      liblivekit_napi_ohos.so                                 │
├─────────────────────────────────────────────────────────────┤
│ 3. .so 部署 + HAP 打包                                       │
│    copy .so → entry/libs/arm64-v8a/liblivekit.so            │
│    hvigorw.bat assembleHap → entry-default-signed.hap        │
├─────────────────────────────────────────────────────────────┤
│ 4. 安装启动                                                  │
│    hdc uninstall → hdc install → hdc shell aa start         │
└─────────────────────────────────────────────────────────────┘
```

### 第一阶段：环境变量

关键变量（基于 `OHOS_SDK_HOME = %LOCALAPPDATA%\OpenHarmony\Sdk\20`）：

```
CC_aarch64_unknown_linux_ohos  = clang.exe --target=aarch64-unknown-linux-ohos --sysroot=...
CXX_aarch64_unknown_linux_ohos = clang++.exe --target=aarch64-unknown-linux-ohos --sysroot=...
AR_aarch64_unknown_linux_ohos  = llvm-ar.exe
RANLIB_aarch64_unknown_linux_ohos = llvm-ranlib.exe
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER = clang.exe
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_RUSTFLAGS = -C link-arg=-fuse-ld=lld -C link-arg=--target=... -C link-arg=--sysroot=...
PATH 注入 OHOS LLVM bin 目录
```

注意：`CC_*` 将 `--target` 和 `--sysroot` 内嵌到命令行字符串中传给 clang；`RUSTFLAGS` 强制指定 lld linker 避免 GNU ld 无法处理 OHOS ELF。

### 第二阶段：Cargo 编译命令

```powershell
cargo build -p livekit-napi-ohos --target aarch64-unknown-linux-ohos --release
```

- `-p livekit-napi-ohos` — 只编译这一个 crate（cdylib），自动拉齐所有 workspace 依赖
- `--target aarch64-unknown-linux-ohos` — 交叉编译到 OHOS arm64
- `--release` — 优化编译（opt-level="z", lto=true, panic="abort", strip="symbols"）
- 产物：`target/aarch64-unknown-linux-ohos/release/liblivekit_napi_ohos.so`

### 第三阶段：.so 部署与 HAP 打包

**.so 部署**（Windows PowerShell）：
```powershell
Copy-Item `
  "target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so" `
  "examples\ohos-livekit-app\entry\libs\arm64-v8a\liblivekit.so" -Force
Copy-Item `
  "target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so" `
  "examples\ohos-livekit-app\libs\arm64-v8a\liblivekit.so" -Force
```

两个目标目录：
1. `entry/libs/arm64-v8a/` — entry 模块级 native 库（hvigor 打包自动包含）
2. `libs/arm64-v8a/` — 项目级 native 库（fallback）

重命名为 `liblivekit.so`（去掉 `_napi_ohos` 后缀），匹配 ArkTS 的 `import { ... } from 'liblivekit.so'` 约定。

**HAP 打包**：
```powershell
$env:JAVA_HOME = "C:\Program Files\Huawei\DevEco Studio\jbr"
$env:PATH = "$env:JAVA_HOME\bin;$env:PATH"
cd examples\ohos-livekit-app
D:\tools\command-line-tools\bin\hvigorw.bat assembleHap --mode module -p product=default --no-daemon
```

- `JAVA_HOME` 指向 DevEco Studio 自带的 JBR（JetBrains Runtime）
- `hvigorw.bat` 是 HarmonyOS 构建入口（类比 Android 的 gradlew）
- `--mode module` — 模块模式，编译单个 HAP 模块
- `-p product=default` — 选择 build-profile.json5 中的 default product（compatibleSdkVersion: 5.0.0(12)）
- `--no-daemon` — 不启动后台守护进程，保证脚本化执行的干净状态
- 产物：`entry/build/default/outputs/default/entry-default-signed.hap`

### 第四阶段：安装与启动

```powershell
$hdc = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20\toolchains\hdc.exe"
& $hdc uninstall com.livekit.ohos.demo     # 卸载旧版本
& $hdc install entry-default-signed.hap    # 安装新 HAP
& $hdc shell aa start -a EntryAbility -b com.livekit.ohos.demo  # 启动
```

- `hdc` — OpenHarmony Device Connector（类 adb）
- `bundleName`: `com.livekit.ohos.demo`
- `EntryAbility` — 页面入口（EntryAbility.ets）

## 签名（首次/证书过期时需要）

如果 HAP 未签名，运行 `sign-hap.ps1`：
```powershell
cd examples\ohos-livekit-app
.\sign-hap.ps1
```

签名流程（8 步）：
1. `hdc shell bm get --udid` — 获取设备 UDID
2. 从 `OpenHarmony.p12` 导出 root/CA 证书
3. `hap-sign-tool.jar generate-keypair` — 生成 ECC NIST-P-256 密钥对
4. `generate-app-cert` — 签发应用证书链（由 OpenHarmony Application CA 签发）
5. `generate-profile-cert` — 签发 Profile 证书链
6. 构建 profile JSON（注入 bundleName/UDID/development-certificate）
7. `sign-profile` — 签名 profile → `.p7b`
8. `sign-app` — 签名 HAP（signCode:1）

## 标准 PC 平台编译

```bash
# 编译主 SDK
cargo build -p livekit
cargo build --release -p livekit

# 测试
cargo test --release -- --nocapture

# E2E 测试（需本地 livekit-server）
cargo test --release --features "__lk-e2e-test" -- --nocapture --test-threads=1

# Docker 交叉编译
cd docker
make sdk-x86_64      # manylinux_2_28 x86_64 容器
make sdk-aarch64     # manylinux_2_28 aarch64 容器
```

## 重要路径清单

| 路径 | 说明 |
|------|------|
| `d:\tdcare\livekit\rust-sdks\temp_full_build.ps1` | 一键构建脚本 |
| `d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app\sign-hap.ps1` | HAP 签名脚本 |
| `d:\tdcare\livekit\rust-sdks\.cargo\config.toml` | 平台级 linker 配置 |
| `d:\tdcare\livekit\rust-sdks\docs\OHOS_BUILD.md` | OHOS 编译文档 |
| `C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20` | OHOS SDK 根目录 |
| `C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20\toolchains\hdc.exe` | hdc 工具 |
| `C:\Program Files\Huawei\DevEco Studio\jbr` | DevEco Studio JBR (Java) |
| `D:\tools\command-line-tools\bin\hvigorw.bat` | hvigor 命令行工具 |
| `d:\tdcare\livekit\rust-sdks\livekit-napi-ohos\` | OHOS NAPI 绑定 crate |
| `d:\tdcare\livekit\rust-sdks\target\aarch64-unknown-linux-ohos\release\liblivekit_napi_ohos.so` | Rust 编译产物 |
| `d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app\entry\libs\arm64-v8a\liblivekit.so` | 部署目标位置 1 |
| `d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app\libs\arm64-v8a\liblivekit.so` | 部署目标位置 2 |
| `d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app\entry\build\default\outputs\default\` | HAP 编译产物目录 |