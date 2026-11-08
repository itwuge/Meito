# ===== Meito 质量门禁 =====
# 用途：串接经验库门禁、Rust 格式/静态检查/测试与架构检查。
# 说明：M0 尚无 Rust workspace 时，Rust 项明确标记 SKIP，不伪造通过。

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Run-Step {
    param(
        [string]$Name,
        [scriptblock]$Action
    )
    Write-Host "==> $Name"
    & $Action
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAIL: $Name (exit $LASTEXITCODE)" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

Write-Host "==> gate.ps1 -SelfTest"
& "$root\scripts\gate.ps1" -SelfTest
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Run-Step "gate.ps1 全量" { & "$root\scripts\gate.ps1" }

if (Test-Path "$root\Cargo.toml") {
    Run-Step "cargo fmt --check" { cargo fmt --all -- --check }
    Run-Step "cargo clippy -D warnings" { cargo clippy --workspace --all-targets -- -D warnings }
    Run-Step "cargo test" { cargo test --workspace }
} else {
    Write-Host "SKIP: Rust workspace 未建立（M0），cargo fmt/clippy/test 未启用。" -ForegroundColor Yellow
}

Run-Step "arch-check.ps1" { & "$root\scripts\arch-check.ps1" }

Write-Host "PASS: quality-gate 完成" -ForegroundColor Green
exit 0
