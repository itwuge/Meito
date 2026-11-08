# ===== Meito 冻结契约检查 =====
# 用途：校验 contracts/ 下的机器可读定义，并阻止 CLI/日志/workflow/状态机契约漂移。
# 说明：契约是后续并行开工的唯一依据；失败即阻断 arch-check。

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$failures = New-Object System.Collections.Generic.List[string]

function Fail([string]$Code, [string]$Message) {
    $script:failures.Add("${Code}: $Message")
}

function Load-Json([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        Fail "CONTRACT_MISSING" $Path
        return $null
    }
    try {
        return (Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json)
    } catch {
        Fail "CONTRACT_JSON" "$Path 不是合法 JSON：$($_.Exception.Message)"
        return $null
    }
}

# ===== 版本与文件清单 =====
$versionPath = "contracts/contract-version.json"
$version = Load-Json $versionPath
$contractVersion = ""
if ($null -ne $version) {
    $contractVersion = [string]$version.contractVersion
    if ($version.status -ne "frozen") { Fail "CONTRACT_STATUS" "contract-version.status 必须为 frozen" }
    if ($version.byoTurn.enabled -ne $false) { Fail "CONTRACT_BYO_TURN" "BYO TURN 默认必须关闭" }
    if ($version.byoTurn.status -ne "reserved") { Fail "CONTRACT_BYO_TURN" "BYO TURN 状态必须为 reserved" }
    foreach ($f in $version.files) {
        if (-not (Test-Path -LiteralPath (Join-Path "contracts" $f))) {
            Fail "CONTRACT_FILE_LIST" "contract-version.files 声明了不存在的文件：$f"
        }
    }
}

$contractFiles = @("cli.json", "log.json", "workflow.json", "state-machine.json", "error-codes.json")
foreach ($f in $contractFiles) {
    $doc = Load-Json (Join-Path "contracts" $f)
    if ($null -ne $doc -and [string]$doc.contractVersion -ne $contractVersion) {
        Fail "CONTRACT_VERSION" "$f 的 contractVersion 与 contract-version.json 不一致"
    }
}

# ===== 错误码与退出码 =====
$errors = Load-Json "contracts/error-codes.json"
$codeMap = @{}
$exitMap = @{}
if ($null -ne $errors) {
    foreach ($e in $errors.exitCodes) {
        $n = [int]$e.code
        if ($exitMap.ContainsKey($n)) { Fail "CONTRACT_EXIT" "退出码重复：$n" }
        $exitMap[$n] = $true
    }
    foreach ($c in $errors.codes) {
        $name = [string]$c.code
        if ($name -notmatch "^E_[A-Z0-9_]+$") { Fail "CONTRACT_ERROR_NAME" "非法错误码：$name" }
        if ($codeMap.ContainsKey($name)) { Fail "CONTRACT_ERROR_DUP" "错误码重复：$name" }
        $codeMap[$name] = $true
        $exit = [int]$c.exitCode
        if (-not $exitMap.ContainsKey($exit)) {
            Fail "CONTRACT_ERROR_EXIT" "$name 的退出码 $exit 未在 exitCodes 中声明"
        }
    }
}

# ===== CLI 契约 =====
$cli = Load-Json "contracts/cli.json"
if ($null -ne $cli) {
    if ($cli.byoTurn.enabled -ne $false) { Fail "CONTRACT_BYO_TURN" "cli.json 的 BYO TURN 默认必须关闭" }
    $commands = @{}
    foreach ($cmd in $cli.commands) {
        $name = [string]$cmd.name
        if ($commands.ContainsKey($name)) { Fail "CONTRACT_CLI_CMD" "子命令重复：$name" }
        $commands[$name] = $true
        $argNames = @{}
        foreach ($a in $cmd.args) {
            $an = [string]$a.name
            if ($argNames.ContainsKey($an)) { Fail "CONTRACT_CLI_ARG" "$name 参数重复：$an" }
            $argNames[$an] = $true
        }
    }
    foreach ($required in @("init", "doctor", "up", "web", "share", "status", "connect", "entry", "pull", "kconfig", "down", "logs", "env", "clean", "p2p", "config")) {
        if (-not $commands.ContainsKey($required)) { Fail "CONTRACT_CLI_CMD" "缺少冻结子命令：$required" }
    }
    $envNames = @{}
    foreach ($envItem in $cli.environment) {
        $en = [string]$envItem.name
        if ($envNames.ContainsKey($en)) { Fail "CONTRACT_ENV" "环境变量重复：$en" }
        $envNames[$en] = $true
        if ($envItem.secret -eq $true -and $envItem.redact -ne $true) {
            Fail "CONTRACT_ENV_REDACT" "$en 标记为 secret 但未标记 redact"
        }
    }
}

# ===== 日志契约 =====
$log = Load-Json "contracts/log.json"
if ($null -ne $log) {
    $requiredNames = @($log.requiredFields | ForEach-Object { [string]$_.name })
    $expected = @("ts", "level", "session_id", "stage", "event", "error_code")
    if (($requiredNames -join ",") -ne ($expected -join ",")) {
        Fail "CONTRACT_LOG_FIELDS" "固定字段必须依次为 ts,level,session_id,stage,event,error_code"
    }
    if ([string]$log.path -ne "logs/session-<id>.jsonl") { Fail "CONTRACT_LOG_PATH" "日志路径必须为 logs/session-<id>.jsonl" }
    foreach ($level in @("info", "debug", "trace", "warn", "error")) {
        if (-not ($log.requiredFields | Where-Object { $_.name -eq "level" }).enum.Contains($level)) {
            Fail "CONTRACT_LOG_LEVEL" "缺少日志级别：$level"
        }
    }
    foreach ($key in @("token", "authorization", "private_key", "secret", "password", "session_connect_string")) {
        if (-not $log.redaction.keys.Contains($key)) { Fail "CONTRACT_LOG_REDACT" "缺少脱敏 key：$key" }
    }
}

# ===== 状态机契约 =====
$sm = Load-Json "contracts/state-machine.json"
$stateNames = @{}
if ($null -ne $sm) {
    foreach ($s in $sm.states) {
        $name = [string]$s.name
        if ($stateNames.ContainsKey($name)) { Fail "CONTRACT_STATE_DUP" "状态重复：$name" }
        $stateNames[$name] = $true
    }
    if (-not $stateNames.ContainsKey([string]$sm.initial)) { Fail "CONTRACT_STATE_INIT" "initial 状态不存在" }
    $eventNames = @{}
    foreach ($ev in $sm.events) { $eventNames[[string]$ev] = $true }
    foreach ($t in $sm.transitions) {
        if (-not $stateNames.ContainsKey([string]$t.from)) { Fail "CONTRACT_TRANSITION" "from 状态不存在：$($t.from)" }
        if (-not $stateNames.ContainsKey([string]$t.to)) { Fail "CONTRACT_TRANSITION" "to 状态不存在：$($t.to)" }
        if (-not $eventNames.ContainsKey([string]$t.event)) { Fail "CONTRACT_TRANSITION" "事件未声明：$($t.event)" }
    }
    foreach ($ac in $sm.autoCancel) {
        if (-not $stateNames.ContainsKey([string]$ac.when)) { Fail "CONTRACT_AUTOCANCEL" "autoCancel 状态不存在：$($ac.when)" }
        if (-not $eventNames.ContainsKey([string]$ac.event)) { Fail "CONTRACT_AUTOCANCEL" "autoCancel 事件未声明：$($ac.event)" }
        if (-not $codeMap.ContainsKey([string]$ac.errorCode)) { Fail "CONTRACT_AUTOCANCEL" "autoCancel 错误码未声明：$($ac.errorCode)" }
    }
    if ([int]$sm.timeouts.jobHardLimitMinutes -ne 360) { Fail "CONTRACT_TIMEOUT" "job 硬上限必须为 360" }
    if ([int]$sm.timeouts.sessionMaxMinutes -gt 300) { Fail "CONTRACT_TIMEOUT" "会话上限不得超过 300" }
}

# 日志阶段必须与状态机状态完全一致
if ($null -ne $log -and $null -ne $sm) {
    $logStages = @($log.requiredFields | Where-Object { $_.name -eq "stage" } | ForEach-Object { $_.enum })
    foreach ($s in $sm.states) {
        if (-not $logStages.Contains([string]$s.name)) { Fail "CONTRACT_STAGE_DRIFT" "日志 stage 缺少状态：$($s.name)" }
    }
    foreach ($s in $logStages) {
        if (-not $stateNames.ContainsKey([string]$s)) { Fail "CONTRACT_STAGE_DRIFT" "日志 stage 多出状态：$s" }
    }
}

# ===== Workflow 契约 =====
$wf = Load-Json "contracts/workflow.json"
if ($null -ne $wf) {
    if ($wf.byoTurn.enabled -ne $false) { Fail "CONTRACT_BYO_TURN" "workflow.json 的 BYO TURN 默认必须关闭" }
    if ([int]$wf.timeoutMinutes -ne 360) { Fail "CONTRACT_WF_TIMEOUT" "workflow timeoutMinutes 必须为 360" }
    if ([int]$wf.sessionMaxMinutes -gt 300) { Fail "CONTRACT_WF_TIMEOUT" "workflow sessionMaxMinutes 不得超过 300" }
    if (@($wf.inputs).Count -gt 10) { Fail "CONTRACT_WF_INPUTS" "workflow 输入不得超过 10 个" }
    $inputNames = @{}
    foreach ($i in $wf.inputs) {
        $n = [string]$i.name
        if ($inputNames.ContainsKey($n)) { Fail "CONTRACT_WF_INPUTS" "workflow 输入重复：$n" }
        $inputNames[$n] = $true
    }
    foreach ($required in @("session_id", "ref", "max_minutes", "idle_minutes", "p2p_provider", "entry", "channel", "artifact_spec", "flags", "meito_version")) {
        if (-not $inputNames.ContainsKey($required)) { Fail "CONTRACT_WF_INPUTS" "缺少冻结输入：$required" }
    }
    $template = [string]$wf.template
    if (-not (Test-Path -LiteralPath $template)) {
        Fail "CONTRACT_WF_TEMPLATE" "模板不存在：$template"
    } else {
        $templateText = Get-Content -LiteralPath $template -Raw -Encoding UTF8
        foreach ($n in $inputNames.Keys) {
            if ($templateText -notmatch ("(?m)^\s{6}" + [regex]::Escape($n) + ":")) {
                Fail "CONTRACT_WF_TEMPLATE" "模板缺少输入：$n"
            }
        }
    }
    foreach ($out in @("connectionPackage", "readLink", "writeLink", "jobSummary", "artifactManifest")) {
        if ($wf.outputs.PSObject.Properties.Name -notcontains $out) { Fail "CONTRACT_WF_OUTPUT" "缺少输出：$out" }
    }
    $summaryFields = @($wf.outputs.jobSummary.fields)
    foreach ($f in @("session_id", "status", "entry", "read_link_present", "write_link_present", "expires_at", "artifact_name")) {
        if ($summaryFields -notcontains $f) { Fail "CONTRACT_WF_SUMMARY" "Job Summary 缺少字段：$f" }
    }
    if (@($wf.outputs.jobSummary.forbidden) -notcontains "connection_package") {
        Fail "CONTRACT_WF_SUMMARY" "Job Summary 必须显式禁止 connection_package"
    }
    if ($wf.outputs.connectionPackage.inJobSummary -ne $false) {
        Fail "CONTRACT_WF_SECRET" "连接包不得写入 Job Summary"
    }
    if ($wf.outputs.connectionPackage.inArtifact -ne $false -or $wf.outputs.connectionPackage.inLog -ne $false) {
        Fail "CONTRACT_WF_SECRET" "连接包不得写入 artifact 或日志"
    }
}

# ===== Artifact manifest schema =====
$schema = Load-Json "contracts/artifact-manifest.schema.json"
if ($null -ne $schema) {
    $required = @($schema.required)
    foreach ($r in @("manifestVersion", "sessionId", "runId", "baseCommit", "createdAt", "files", "checksums", "guardPassed")) {
        if ($required -notcontains $r) { Fail "CONTRACT_MANIFEST" "manifest schema 缺少 required：$r" }
    }
    if ($schema.properties.files.items.properties.sha256.pattern -notmatch "0-9a-f") {
        Fail "CONTRACT_MANIFEST" "files.sha256 必须是 64 位小写十六进制"
    }
    if ($schema.properties.checksums.required -notcontains "filesSha256" -or $schema.properties.checksums.required -notcontains "archiveSha256") {
        Fail "CONTRACT_MANIFEST" "checksums 必须包含 filesSha256 与 archiveSha256"
    }
}

# ===== 文档与实现对齐 =====
$freezeDoc = "docs/接口冻结.md"
if (-not (Test-Path -LiteralPath $freezeDoc)) {
    Fail "CONTRACT_DOC" "缺少 docs/接口冻结.md"
} else {
    $docText = Get-Content -LiteralPath $freezeDoc -Raw -Encoding UTF8
    foreach ($needle in @($contractVersion, "BYO TURN", "默认关闭", "contracts/")) {
        if ($docText -notmatch [regex]::Escape($needle)) {
            Fail "CONTRACT_DOC" "接口冻结文档缺少：$needle"
        }
    }
}

$coreLib = "crates/meito-core/src/lib.rs"
if (-not (Test-Path -LiteralPath $coreLib)) {
    Fail "CONTRACT_CORE" "缺少 meito-core 公共契约实现"
} else {
    $coreText = Get-Content -LiteralPath $coreLib -Raw -Encoding UTF8
    if ($coreText -notmatch [regex]::Escape('pub const CONTRACT_VERSION: &str = "' + $contractVersion + '"')) {
        Fail "CONTRACT_CORE" "meito-core 的 CONTRACT_VERSION 与契约版本不一致"
    }
    if ($coreText -notmatch [regex]::Escape("logs/session-")) {
        Fail "CONTRACT_CORE" "meito-core 缺少 logs/session- 路径常量"
    }
}

foreach ($shell in @("meito-cli", "meito-web", "meito-runner", "meito-link")) {
    $manifest = Join-Path (Join-Path "crates" $shell) "Cargo.toml"
    if (-not (Test-Path -LiteralPath $manifest)) {
        Fail "CONTRACT_SHELL" "缺少外壳 crate：$shell"
    } else {
        $manifestText = Get-Content -LiteralPath $manifest -Raw -Encoding UTF8
        if ($manifestText -notmatch "meito-core\s*=") {
            Fail "CONTRACT_SHELL" "$shell 未依赖 meito-core"
        }
    }
}

if ($failures.Count -gt 0) {
    Write-Host "FAIL: contract-check" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host " - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "PASS: contract-check（版本 $contractVersion；CLI/日志/workflow/状态机/错误码/artifact manifest 一致）" -ForegroundColor Green
exit 0
