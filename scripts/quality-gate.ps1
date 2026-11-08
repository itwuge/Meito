# ===== Meito 质量门禁 =====
# 用途：串接经验库门禁、Rust 格式/静态检查/测试与架构检查。
# 说明：存在 Cargo.toml 但找不到 cargo 时直接失败；M0 无 workspace 时才标记 SKIP，不伪造通过。

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    Write-Host "FAIL: CARGO_TARGET_DIR 未设置；构建产物不得落仓库内 target/，拒绝回退默认路径。" -ForegroundColor Red
    exit 126
}
if (-not [System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
    Write-Host "FAIL: CARGO_TARGET_DIR 必须是绝对路径，当前为 '$env:CARGO_TARGET_DIR'。" -ForegroundColor Red
    exit 126
}
Write-Host "使用 CARGO_TARGET_DIR：$env:CARGO_TARGET_DIR"

function Run-Command {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$Arguments
    )
    Write-Host "==> $Name"
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAIL: $Name (exit $LASTEXITCODE)" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

function Resolve-Cargo {
    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $fallback = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path -LiteralPath $fallback) { return $fallback }
    return $null
}

$ps = (Get-Command powershell).Source

Run-Command "gate.ps1 -SelfTest" $ps @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $root "scripts\gate.ps1"), "-SelfTest")
Run-Command "gate.ps1 全量" $ps @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $root "scripts\gate.ps1"), "-BaseRef", "main")

if (Test-Path -LiteralPath (Join-Path $root "Cargo.toml")) {
    $cargo = Resolve-Cargo
    if (-not $cargo) {
        Write-Host "FAIL: 未找到 cargo（已检查 PATH 与 $env:USERPROFILE\.cargo\bin\cargo.exe），不伪造 SKIP。" -ForegroundColor Red
        exit 127
    }
    Write-Host "使用 cargo：$cargo"
    Run-Command "cargo fmt --check" $cargo @("fmt", "--all", "--", "--check")
    Run-Command "cargo check" $cargo @("check", "--workspace")
    Run-Command "cargo clippy -D warnings" $cargo @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings")
    Run-Command "cargo test" $cargo @("test", "--workspace")
} else {
    Write-Host "SKIP: Rust workspace 未建立（M0），cargo fmt/check/clippy/test 未启用。" -ForegroundColor Yellow
}

Run-Command "arch-check.ps1" $ps @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $root "scripts\arch-check.ps1"))
Run-Command "target-cleanup.ps1" $ps @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $root "scripts\target-cleanup.ps1"))

Write-Host "PASS: quality-gate 完成" -ForegroundColor Green
exit 0
