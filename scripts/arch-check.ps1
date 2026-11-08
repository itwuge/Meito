# ===== Meito 架构检查 =====
# 用途：在本地与 CI 检查“一套内核、两个外壳”没有被复制成两套逻辑。
# 说明：M0 尚无 Rust 源码时，相关检查标记 SKIP；不伪造通过。

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$failures = New-Object System.Collections.Generic.List[string]
$checks = New-Object System.Collections.Generic.List[string]

function Add-Failure([string]$Code, [string]$Message) {
    $script:failures.Add("${Code}: $Message")
}

function Add-Check([string]$Message) {
    $script:checks.Add($Message)
}

# 检查 0：冻结契约（contracts/ 是后续并行开工的唯一依据）
& powershell -NoProfile -ExecutionPolicy Bypass -File "$root\scripts\contract-check.ps1"
if ($LASTEXITCODE -ne 0) {
    Add-Failure "ARCH_CONTRACT" "contract-check 未通过（退出码 $LASTEXITCODE）"
} else {
    Add-Check "冻结契约"
}

# 检查 1：禁止 TURN / 中继配置出现在网页外壳
$webPaths = @("$root\web", "$root\crates\meito-web")
foreach ($p in $webPaths) {
    if (Test-Path $p) {
        Get-ChildItem -Path $p -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Extension -in @(".js", ".ts", ".rs", ".json", ".html") } |
            ForEach-Object {
                $hits = Select-String -Path $_.FullName -Pattern "turn:" -SimpleMatch -ErrorAction SilentlyContinue
                if ($hits) { Add-Failure "ARCH_TURN_FORBIDDEN" "$($_.FullName) 含 turn: URL" }
            }
    }
}
Add-Check "无 TURN URL（web/meito-web）"

# 检查 2：禁止 SPA 框架
$pkg = "$root\web\package.json"
if (Test-Path $pkg) {
    $content = Get-Content $pkg -Raw
    foreach ($fw in @("react", "vue", "angular", "@sveltejs", "svelte")) {
        if ($content -match [regex]::Escape($fw)) {
            Add-Failure "ARCH_SPA_FORBIDDEN" "web/package.json 引用了 $fw"
        }
    }
}
Add-Check "无 SPA 框架"

# 检查 3：会话状态枚举只在 meito-core
$duplicate = Get-ChildItem -Path "$root\crates" -Recurse -File -Filter *.rs -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch "\\meito-core\\" } |
    Select-String -Pattern "enum\s+SessionState" -ErrorAction SilentlyContinue
if ($duplicate) {
    Add-Failure "ARCH_MODEL_DUPLICATE" "SessionState 在 meito-core 之外定义"
}
Add-Check "单一 Model"

# 检查 4：非 CLI 展示层禁止 println!
$printHits = Get-ChildItem -Path "$root\crates" -Recurse -File -Filter *.rs -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch "\\meito-cli\\" -and $_.FullName -notmatch "\\tests\\" -and $_.FullName -notmatch "\\benches\\" } |
    Select-String -Pattern "println!\(" -ErrorAction SilentlyContinue
if ($printHits) {
    Add-Failure "ARCH_LOG_BYPASS" "非 CLI 展示层出现 println!（应使用 tracing）"
}
Add-Check "日志不绕过"

# 检查 5：日志路径约定
$coreFiles = Get-ChildItem -Path "$root\crates\meito-core" -Recurse -File -Filter *.rs -ErrorAction SilentlyContinue
if ($coreFiles) {
    $hasPath = $coreFiles | Select-String -Pattern "logs/session-" -SimpleMatch -ErrorAction SilentlyContinue
    if (-not $hasPath) {
        Add-Failure "ARCH_LOG_PATH" "meito-core 未出现 logs/session- 路径约定"
    }
}
Add-Check "日志路径约定"

# 检查 6：核心文件头注释
if ($coreFiles) {
    foreach ($f in $coreFiles) {
        $head = Get-Content $f.FullName -TotalCount 25 -ErrorAction SilentlyContinue
        if (-not ($head | Select-String -Pattern "//!" -SimpleMatch)) {
            Add-Failure "ARCH_NOTE" "$($f.FullName) 首 25 行缺少 //! 文件头"
        }
    }
}
Add-Check "Rust 文件头"

if ($failures.Count -gt 0) {
    Write-Host "FAIL: arch-check" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host " - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "PASS: arch-check（$($checks -join '；')）" -ForegroundColor Green
exit 0
