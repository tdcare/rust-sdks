# WebRTC-rs/rtc OpenHarmony 补丁

本目录维护对上游 [webrtc-rs/rtc](https://github.com/webrtc-rs/rtc) 的 OpenHarmony 兼容性补丁。

采用 **patch 增量增强** 方式，而非直接 fork 复制源码。

## 目录结构

```
webrtc/patches/
├── 0001-ohos-support.patch   # OpenHarmony 兼容性补丁
├── apply-patches.ps1          # 自动化应用脚本
├── rtc-patched/               # [git clone + patch 生成] 上游仓库 + 补丁应用后的结果
└── README.md
```

## 快速使用

```powershell
# 首次使用：自动克隆上游仓库 + 应用补丁
.\webrtc\patches\apply-patches.ps1

# 重新应用：重置仓库到干净状态，重新打补丁
.\webrtc\patches\apply-patches.ps1 -Force
```

脚本会：
1. 从 GitHub 浅克隆 `webrtc-rs/rtc` master 分支到 `rtc-patched/`
2. 依次应用 `patches/` 下的 `.patch` 文件
3. 自动验证补丁结果（文件存在性 + 内容校验）

脚本具备**幂等性**：已打过补丁时直接跳过，不重复操作。

## Cargo.toml 引用

`webrtc_client` 通过本地路径引用打过补丁的 rtc：

```toml
# webrtc/webrtc_client/Cargo.toml
rtc = { path = "../patches/rtc-patched/rtc" }
```

## 补丁详情

### 0001-ohos-support.patch

| 修改文件 | 变更内容 | 原因 |
|----------|----------|------|
| `rtc-mdns/src/proto/mod.rs` | `Ipv4Addr::from_octets(a.a)` → `Ipv4Addr::from(a.a)` | `from_octets` 是 unstable API，Rust stable 不可用 |
| `rtc-shared/Cargo.toml` | nix 依赖条件增加 `not(target_env = "ohos")` | nix crate 依赖 libc 类型，与 OHOS 的 libc 不兼容（`u32` vs `u8` 等） |
| `rtc-shared/src/ifaces/ffi/mod.rs` | 增加 `#[cfg(target_env = "ohos")]` 条件编译分支 | 将 OHOS 从 unix 分支中分离出来 |
| `rtc-shared/src/ifaces/ffi/ohos/mod.rs` | **新增** - `ifaces()` 返回空列表 | Sans-I/O 架构下网络地址由上层应用提供，无需系统查询 |

### 问题背景

上游 rtc 库在非 Windows 平台依赖 `nix` crate 获取网络接口列表。`nix` 内部使用的 libc 类型定义与 OpenHarmony NDK 的 musl libc 不兼容：

```
error[E0308]: mismatched types
  nix::ifaddrs 期望 libc::c_char = i8
  OHOS musl libc 提供 libc::c_char = u8
```

rtc 采用 Sans-I/O 架构，协议逻辑与网络 I/O 完全分离，应用层通过 `handle_read()` / `poll_write()` 手动传入数据包。因此网络接口发现不是必需的，使用空实现即可。

## 如何新增补丁

1. 在 `rtc-patched/` 中修改代码
2. 生成 patch：
   ```powershell
   cd webrtc/patches/rtc-patched
   # 已跟踪文件的修改
   git diff > ../0002-your-patch-name.patch
   # 新增文件需先 stage
   git add <新文件>
   git diff --cached >> ../0002-your-patch-name.patch
   git reset HEAD <新文件>
   ```
3. 在 `apply-patches.ps1` 的 `$PATCHES` 数组中追加新补丁文件名：
   ```powershell
   $PATCHES = @(
       "0001-ohos-support.patch"
       "0002-your-patch-name.patch"
   )
   ```
4. 运行 `-Force` 验证：
   ```powershell
   .\apply-patches.ps1 -Force
   ```

## 注意事项

- `rtc-patched/` 目录 **不应提交到 Git**（已在 `.gitignore` 中排除），它是构建时动态生成的
- 补丁基于上游 master 分支最新 commit，若上游更新导致冲突，需手动调整 patch 文件
- 补丁应用使用 `git apply`，冲突时自动回退到 `--3way` 模式尝试合并
