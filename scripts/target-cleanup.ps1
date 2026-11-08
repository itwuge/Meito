# ===== Meito 构建产物清理 =====
# 用途：每次任务收尾删除统一 CARGO_TARGET_DIR 下的 debug 中间产物，并报告释放量。
# 输入：环境变量 CARGO_TARGET_DIR（必须已设置，缺失即失败，不回退默认路径）。
# 退出码：0=清理完成；2=环境变量缺失或路径非法。
# 契约：gate.config.json 的 build 段与 docs/工程规范索引.md 的构建产物治理条目。
# 未实测：释放量按删除前后同一分区的可用空间差值计算，受并发写入影响可能略有偏差。

$ErrorActionPreference = "Stop"

$target = $env:CARGO_TARGET_DIR
if ([string]::IsNullOrWhiteSpace($target)) {
    Write-Host "FAIL: CARGO_TARGET_DIR 未设置；拒绝回退到仓库内 target/。" -ForegroundColor Red
    exit 2
}
if (-not [System.IO.Path]::IsPathRooted($target)) {
    Write-Host "FAIL: CARGO_TARGET_DIR 必须是绝对路径，当前为 '$target'。" -ForegroundColor Red
    exit 2
}

$targetFull = [System.IO.Path]::GetFullPath($target)
$debug = Join-Path $targetFull "debug"
$tmp = Join-Path $targetFull "tmp"
$root = [System.IO.Path]::GetPathRoot($targetFull)
$driveLetter = $root.TrimEnd('\', '/')
if ($driveLetter.Length -ne 2 -or -not $driveLetter.EndsWith(":")) {
    Write-Host "WARN: 无法从 '$root' 解析盘符，跳过释放量对比。" -ForegroundColor Yellow
    $before = 0
} else {
    $before = (Get-PSDrive -Name $driveLetter.Substring(0, 1)).Free
}

$debugBytes = 0
if (Test-Path -LiteralPath $debug) {
    $debugBytes = (Get-ChildItem -LiteralPath $debug -Recurse -File -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $debugBytes) { $debugBytes = 0 }
    Remove-Item -LiteralPath $debug -Recurse -Force
}
$tmpBytes = 0
if (Test-Path -LiteralPath $tmp) {
    $tmpBytes = (Get-ChildItem -LiteralPath $tmp -Recurse -File -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $tmpBytes) { $tmpBytes = 0 }
    Remove-Item -LiteralPath $tmp -Recurse -Force
}

if ($driveLetter.Length -eq 2 -and $driveLetter.EndsWith(":")) {
    $after = (Get-PSDrive -Name $driveLetter.Substring(0, 1)).Free
    $freed = $after - $before
} else {
    $freed = $debugBytes + $tmpBytes
}

Write-Host ("target=" + $targetFull)
Write-Host ("debugBytes=" + $debugBytes)
Write-Host ("tmpBytes=" + $tmpBytes)
Write-Host ("freedBytes=" + $freed)
if ($driveLetter.Length -eq 2 -and $driveLetter.EndsWith(":")) {
    Write-Host ("freeBytes=" + $after)
}
Write-Host "PASS: target-cleanup 完成" -ForegroundColor Green
exit 0
