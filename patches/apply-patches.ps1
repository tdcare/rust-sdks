<#
.SYNOPSIS
    从 upstream 克隆 webrtc-rs/rtc 并应用 OpenHarmony 补丁

.DESCRIPTION
    此脚本实现 patch-based 工作流：
    1. 从 GitHub 浅克隆 webrtc-rs/rtc (如不存在)
    2. 应用 OpenHarmony 兼容性补丁 (0001-ohos-support.patch)
    3. 生成可直接被 Cargo.toml path 引用的本地目录

    补丁内容 (0001-ohos-support.patch):
    - rtc-mdns:   Ipv4Addr::from_octets(a.a) -> Ipv4Addr::from(a.a)  (unstable API fix)
    - rtc-shared:  排除 nix crate 对 target_env="ohos" 目标的编译
    - rtc-shared:  添加 ohos 平台 ifaces stub 空实现 (Sans-I/O, 地址由上层提供)

.PARAMETER Force
    重置已有仓库到干净状态，重新应用补丁

.EXAMPLE
    .\apply-patches.ps1
    .\apply-patches.ps1 -Force
#>

param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"

# ============================================================
# 配置
# ============================================================
$SCRIPT_DIR    = Split-Path -Parent $MyInvocation.MyCommand.Path
$RTC_REPO      = "https://github.com/webrtc-rs/rtc.git"
$TARGET_DIR    = Join-Path $SCRIPT_DIR "rtc-patched"
$PATCH_DIR     = $SCRIPT_DIR
$PATCHES       = @(
    "0001-ohos-support.patch"
)

# 补丁标志: 若此文件存在则认为补丁已应用
$PATCH_MARKER  = Join-Path $TARGET_DIR "rtc-shared\src\ifaces\ffi\ohos\mod.rs"

# ============================================================
# 辅助函数
# ============================================================
function Write-Step($msg) {
    Write-Host "`n==> $msg" -ForegroundColor Cyan
}

function Write-Ok($msg) {
    Write-Host "    [OK] $msg" -ForegroundColor Green
}

function Write-Err($msg) {
    Write-Host "    [ERROR] $msg" -ForegroundColor Red
}

function Apply-Patches {
    Write-Step "应用 OpenHarmony 兼容性补丁"
    foreach ($patch in $PATCHES) {
        $patchFile = Join-Path $PATCH_DIR $patch
        if (-not (Test-Path $patchFile)) {
            Write-Err "补丁文件不存在: $patchFile"
            exit 1
        }
        Write-Host "    应用 $patch ..." -ForegroundColor Gray
        # git apply --verbose 输出到 stderr, 临时切换 ErrorActionPreference
        $prevEAP = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $output = & git -C $TARGET_DIR apply --verbose $patchFile 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $prevEAP
        $output | ForEach-Object { Write-Host "      $_" -ForegroundColor Gray }
        if ($exitCode -ne 0) {
            Write-Host "    git apply 失败, 尝试 --3way ..." -ForegroundColor Yellow
            $ErrorActionPreference = "Continue"
            $output2 = & git -C $TARGET_DIR apply --3way $patchFile 2>&1
            $exitCode2 = $LASTEXITCODE
            $ErrorActionPreference = $prevEAP
            $output2 | ForEach-Object { Write-Host "      $_" -ForegroundColor Gray }
            if ($exitCode2 -ne 0) {
                Write-Err "补丁应用失败: $patch"
                exit 1
            }
        }
        Write-Ok "$patch"
    }
}

function Verify-Patches {
    Write-Step "验证补丁结果"
    $checks = @(
        @{ Path = "rtc-shared\src\ifaces\ffi\ohos\mod.rs";  Desc = "ohos ifaces stub" },
        @{ Path = "rtc-shared\Cargo.toml";                   Desc = "nix 条件编译" },
        @{ Path = "rtc-shared\src\ifaces\ffi\mod.rs";        Desc = "ffi mod ohos 分支" },
        @{ Path = "rtc-mdns\src\proto\mod.rs";               Desc = "Ipv4Addr::from 修复" }
    )
    $allOk = $true
    foreach ($check in $checks) {
        $filePath = Join-Path $TARGET_DIR $check.Path
        if (Test-Path $filePath) {
            Write-Ok "$($check.Desc)"
        } else {
            Write-Err "$($check.Desc) - 文件缺失"
            $allOk = $false
        }
    }
    # 内容校验
    $cargoContent = Get-Content (Join-Path $TARGET_DIR "rtc-shared\Cargo.toml") -Raw
    if ($cargoContent -match 'not\(target_env = "ohos"\)') {
        Write-Ok "Cargo.toml ohos nix 排除"
    } else {
        Write-Err "Cargo.toml 缺少 ohos 排除"; $allOk = $false
    }
    $modContent = Get-Content (Join-Path $TARGET_DIR "rtc-shared\src\ifaces\ffi\mod.rs") -Raw
    if ($modContent -match 'mod ohos') {
        Write-Ok "mod.rs ohos 模块声明"
    } else {
        Write-Err "mod.rs 缺少 ohos 模块"; $allOk = $false
    }
    if (-not $allOk) { Write-Err "验证失败"; exit 1 }
}

# ============================================================
# 主流程
# ============================================================

Write-Host "================================================" -ForegroundColor Yellow
Write-Host "  WebRTC-rs/rtc OpenHarmony Patch Applicator"     -ForegroundColor Yellow
Write-Host "================================================" -ForegroundColor Yellow
Write-Host "  Upstream : $RTC_REPO"
Write-Host "  Target   : $TARGET_DIR"

$hasGit = Test-Path (Join-Path $TARGET_DIR ".git")

if ($hasGit -and $Force) {
    # -Force: 用 git reset 还原到干净上游状态, 再重新打 patch
    Write-Step "重置仓库到干净状态 (-Force)"
    $null = & git -C $TARGET_DIR checkout -- . 2>&1
    $null = & git -C $TARGET_DIR clean -fd 2>&1
    Write-Ok "仓库已重置"
    Apply-Patches
    Verify-Patches
} elseif ($hasGit -and (Test-Path $PATCH_MARKER)) {
    # 已克隆且已打 patch
    Write-Host ""
    Write-Ok "rtc-patched 已就绪 (补丁已应用)"
    Write-Host "    如需重新应用: .\apply-patches.ps1 -Force" -ForegroundColor Gray
    exit 0
} elseif ($hasGit) {
    # 已克隆但未打 patch
    Write-Host ""
    Write-Host "    仓库存在但补丁未应用, 正在应用..." -ForegroundColor Yellow
    Apply-Patches
    Verify-Patches
} else {
    # 需要克隆
    Write-Step "克隆 webrtc-rs/rtc (shallow clone)"
    # 如果存在空目录, 先尝试删除
    if (Test-Path $TARGET_DIR) {
        Remove-Item -LiteralPath $TARGET_DIR -Force -ErrorAction SilentlyContinue
    }
    Write-Host "    git clone --depth 1 --branch master ..." -ForegroundColor Gray
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & git clone --depth 1 --branch master $RTC_REPO $TARGET_DIR 2>&1 | ForEach-Object { Write-Host "    $_" -ForegroundColor Gray }
    $cloneExit = $LASTEXITCODE
    $ErrorActionPreference = $prevEAP
    if ($cloneExit -ne 0) { Write-Err "git clone 失败"; exit 1 }
    $commit = & git -C $TARGET_DIR rev-parse --short HEAD
    Write-Ok "克隆成功 (commit: $commit)"
    Apply-Patches
    Verify-Patches
}

# 完成
Write-Host ""
Write-Host "================================================" -ForegroundColor Green
Write-Host "  完成! 补丁已成功应用" -ForegroundColor Green
Write-Host "================================================" -ForegroundColor Green
Write-Host ""
Write-Host "  Cargo.toml 引用:" -ForegroundColor Gray
Write-Host '    rtc = { path = "../patches/rtc-patched/rtc" }' -ForegroundColor White
Write-Host ""
