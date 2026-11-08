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

$contractFiles = @(
    "cli.json",
    "log.json",
    "workflow.json",
    "state-machine.json",
    "error-codes.json",
    "artifact-manifest.schema.json",
    "connection-package.json",
    "udp-probe.json",
    "meito-config.schema.json",
    "tools-lock.schema.json",
    "link.json",
    "entry.json",
    "acceptance.json"
)
foreach ($f in $contractFiles) {
    if ($null -ne $version -and -not (@($version.files) -contains $f)) {
        Fail "CONTRACT_FILE_LIST" "contract-version.files 缺少冻结契约：$f"
    }
    $doc = Load-Json (Join-Path "contracts" $f)
    if ($null -ne $doc -and ($doc.PSObject.Properties.Name -contains "contractVersion")) {
        if ([string]$doc.contractVersion -ne $contractVersion) {
            Fail "CONTRACT_VERSION" "$f 的 contractVersion 与 contract-version.json 不一致"
        }
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
    foreach ($required in @("session_id", "ref", "max_minutes", "idle_minutes", "p2p_provider", "entry", "channel", "channel_write_token", "channel_public_key", "flags")) {
        if (-not $inputNames.ContainsKey($required)) { Fail "CONTRACT_WF_INPUTS" "缺少冻结输入：$required" }
    }
    $channel = $wf.inputs | Where-Object { $_.name -eq "channel" }
    if ([int]$channel.maxLength -gt 128) { Fail "CONTRACT_WF_CHANNEL" "channel 必须是不透明 id 且不超过 128 字符" }
    if ($channel.description -notmatch "不透明") { Fail "CONTRACT_WF_CHANNEL" "channel 必须明确为不透明 id" }
    $flags = $wf.inputs | Where-Object { $_.name -eq "flags" }
    foreach ($fieldName in @("artifact_spec", "meito_version", "allowed_actors")) {
        if (@($flags.fields) -notcontains $fieldName) { Fail "CONTRACT_WF_FLAGS" "flags 缺少字段：$fieldName" }
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

# ===== 连接包 / UDP / 配置 / 依赖锁定（0.2.0 新增） =====
$cp = Load-Json "contracts/connection-package.json"
if ($null -ne $cp) {
    if ([string]$cp.magic -ne "MEITO1" -or [int]$cp.magicBytes -ne 6) {
        Fail "CONTRACT_CP_MAGIC" "连接包 magic 必须为 MEITO1 且占 6 字节"
    }
    if ([int]$cp.formatVersion -ne 1) { Fail "CONTRACT_CP_VERSION" "连接包 formatVersion 必须为 1" }
    if ([string]$cp.algorithm.id -ne "x25519-xchacha20poly1305") {
        Fail "CONTRACT_CP_ALG" "算法 id 必须为 x25519-xchacha20poly1305"
    }
    if ([int]$cp.algorithm.idByte -ne 0) { Fail "CONTRACT_CP_ALG" "算法 idByte 必须为 0" }
    $algorithmIds = @($cp.algorithmIds)
    if ($algorithmIds.Count -ne 1) { Fail "CONTRACT_CP_ALG" "算法 id 表当前只允许 1 项" }
    if ([string]$algorithmIds[0].name -ne [string]$cp.algorithm.id -or [int]$algorithmIds[0].id -ne [int]$cp.algorithm.idByte) {
        Fail "CONTRACT_CP_ALG" "algorithmIds 必须与 algorithm.id/idByte 一致"
    }
    if ([string]$cp.encoding.wireEncoding -ne "base64url-no-padding") {
        Fail "CONTRACT_CP_LAYOUT" "线格式必须为 base64url-no-padding"
    }
    $expectedLayout = @("magic", "format_version", "algorithm_id", "ephemeral_public_key", "nonce", "ciphertext")
    $actualLayout = @($cp.layout | ForEach-Object { [string]$_.name })
    if (($actualLayout -join ",") -ne ($expectedLayout -join ",")) {
        Fail "CONTRACT_CP_LAYOUT" "连接包布局顺序必须为 magic,format_version,algorithm_id,ephemeral_public_key,nonce,ciphertext"
    }
    $expectedBytes = @(6, 1, 1, 32, 24)
    for ($i = 0; $i -lt $expectedBytes.Count; $i++) {
        if ([int]$cp.layout[$i].bytes -ne $expectedBytes[$i]) {
            Fail "CONTRACT_CP_LAYOUT" "连接包第 $($i + 1) 段字节数不符"
        }
    }
    if ($cp.layout[5].includesTag -ne $true) { Fail "CONTRACT_CP_LAYOUT" "密文必须包含 AEAD tag" }
    if ([string]$cp.serverHandling.store -ne "ciphertext-only" -or $cp.serverHandling.parse -ne $false -or $cp.serverHandling.log -ne $false) {
        Fail "CONTRACT_CP_SERVER" "服务端只能存密文，禁止解析或记录"
    }
    if ([int]$cp.serverHandling.maxCiphertextBytes -ne 16384) {
        Fail "CONTRACT_CP_SERVER" "单包密文上限必须为 16384 字节"
    }
}

$udp = Load-Json "contracts/udp-probe.json"
if ($null -ne $udp) {
    if ([string]$udp.request.magic -ne "MEITOUDP" -or [int]$udp.request.magicBytes -ne 8) {
        Fail "CONTRACT_UDP_MAGIC" "UDP magic 必须为 MEITOUDP 且占 8 字节"
    }
    if ([int]$udp.request.nonceBytes -ne 8) { Fail "CONTRACT_UDP_NONCE" "UDP nonce 必须为 8 字节" }
    if ([int]$udp.timeout.attempts -ne 3 -or [int]$udp.timeout.perAttemptMs -ne 1000) {
        Fail "CONTRACT_UDP_TIMEOUT" "UDP 探测必须为 3 次尝试、每次 1000ms"
    }
    if ([string]$udp.target.key -ne "udp_probe_target" -or $udp.target.requiredWhenProbeEnabled -ne $true) {
        Fail "CONTRACT_UDP_TARGET" "UDP 目标必须来自 meito.toml 的 udp_probe_target 且启用时必填"
    }
    if ([string]$udp.listener.provider -ne "meito-link" -or $udp.listener.defaultEnabled -ne $false -or $udp.listener.forwardsSessionBytes -ne $false) {
        Fail "CONTRACT_UDP_LISTENER" "meito-link UDP 监听必须默认关闭且不转发会话字节"
    }
}

$config = Load-Json "contracts/meito-config.schema.json"
if ($null -ne $config) {
    if (@($config.required) -notcontains "security") { Fail "CONTRACT_CONFIG" "配置必须要求 security 表" }
    if ($config.additionalProperties -ne $false) { Fail "CONTRACT_CONFIG" "配置不得允许未知顶层字段" }
    if (@($config.properties.security.required) -notcontains "allowed_actors") { Fail "CONTRACT_CONFIG" "security 必须要求 allowed_actors" }
    if ($config.properties.security.additionalProperties -ne $false) { Fail "CONTRACT_CONFIG" "security 不得允许未知字段" }
    if ([int]$config.properties.security.properties.allowed_actors.items.minLength -ne 1) { Fail "CONTRACT_CONFIG" "allowed_actors 元素不得为空" }
    foreach ($key in @("missingConfig", "missingAllowedActors", "emptyAllowedActors")) {
        if ([string]$config.failClosed.$key -ne "deny-all") { Fail "CONTRACT_CONFIG" "failClosed.$key 必须为 deny-all" }
    }
    if ([string]$config.failClosed.workflowOverride -ne "allowed") { Fail "CONTRACT_CONFIG" "workflowOverride 必须允许" }
    if ([string]$config.failClosed.workflowOverrideInput -ne "flags.allowed_actors") { Fail "CONTRACT_CONFIG" "覆盖输入必须为 flags.allowed_actors" }
    if ([string]$config.failClosed.workflowOverridePrecedence -ne "override") { Fail "CONTRACT_CONFIG" "覆盖优先级必须为 override" }
    $udpPattern = [string]$config.properties.network.properties.udp_probe_target.pattern
    if (-not $udpPattern.StartsWith("^[A-Za-z0-9_") -or -not $udpPattern.Contains(":[0-9]")) {
        Fail "CONTRACT_CONFIG" "udp_probe_target 必须为 host:port 形状"
    }
}

$lockSchema = Load-Json "contracts/tools-lock.schema.json"
if ($null -ne $lockSchema) {
    if (@($lockSchema.properties.lockVersion.enum) -notcontains 1) { Fail "CONTRACT_LOCK" "lockVersion 只允许 1" }
    if ([int]$lockSchema.properties.tools.minProperties -lt 1) { Fail "CONTRACT_LOCK" "tools 不得为空" }
    if (@($lockSchema.properties.tools.additionalProperties.properties.status.enum) -notcontains "pinned" -or @($lockSchema.properties.tools.additionalProperties.properties.status.enum) -notcontains "unpinned") {
        Fail "CONTRACT_LOCK" "status 只允许 pinned/unpinned"
    }
    $requiredPinned = @($lockSchema.properties.tools.additionalProperties.allOf[0].then.required)
    foreach ($f in @("version", "url", "sha256")) {
        if ($requiredPinned -notcontains $f) { Fail "CONTRACT_LOCK" "pinned 条目必须要求 $f" }
    }
    if ([string]$lockSchema.properties.tools.additionalProperties.properties.sha256.pattern -ne "^[0-9a-f]{64}$") {
        Fail "CONTRACT_LOCK" "sha256 必须为 64 位小写十六进制"
    }
    foreach ($key in @("refuseUnpinned", "refuseInstallWhenUnpinned", "verifySha256BeforeRun", "forbidCurlPipeShell")) {
        if ($lockSchema.policy.$key -ne $true) { Fail "CONTRACT_LOCK" "policy.$key 必须为 true" }
    }
    foreach ($pattern in @("curl:*|*bash", "curl:*|*sh", "wget:*|*bash", "wget:*|*sh")) {
        if (@($lockSchema.policy.forbiddenPatterns) -notcontains $pattern) { Fail "CONTRACT_LOCK" "forbiddenPatterns 缺少：$pattern" }
    }
}

# ===== 入口端口契约 entry.json（0.3.2） =====
$entry = Load-Json "contracts/entry.json"
if ($null -ne $entry) {
    if ([string]$entry.bind.address -ne "127.0.0.1") {
        Fail "CONTRACT_ENTRY_BIND" "bind.address 必须为 127.0.0.1"
    }
    if ($entry.bind.loopbackOnly -ne $true -or $entry.bind.allowWildcardBind -ne $false) {
        Fail "CONTRACT_ENTRY_BIND" "bind 必须 loopbackOnly=true 且 allowWildcardBind=false"
    }
    $expectedPorts = @{
        terminal = 7681
        graphical = 6080
        vnc = 5900
    }
    foreach ($key in $expectedPorts.Keys) {
        if ([int]$entry.ports.$key -ne [int]$expectedPorts[$key]) {
            Fail "CONTRACT_ENTRY_PORT" "ports.$key 必须为 $($expectedPorts[$key])"
        }
    }
    if ([int]$entry.graphical.framebuffer.width -ne 1280 -or [int]$entry.graphical.framebuffer.height -ne 800 -or [int]$entry.graphical.framebuffer.depth -ne 24) {
        Fail "CONTRACT_ENTRY_FB" "图形帧缓冲必须为 1280x800x24"
    }
    if ([int]$entry.graphical.vncPasswordChars -ne 8) {
        Fail "CONTRACT_ENTRY_VNC" "vncPasswordChars 必须为 8"
    }
    if ([string]$entry.terminal.writeFlag -ne "-W" -or [string]$entry.terminal.singleClientFlag -ne "-m 1" -or [string]$entry.terminal.originCheckFlag -ne "-O") {
        Fail "CONTRACT_ENTRY_TTYD" "ttyd 标志必须为 -W / -m 1 / -O"
    }
}

# ===== 验收指标契约 acceptance.json（0.3.2） =====
$acceptance = Load-Json "contracts/acceptance.json"
if ($null -ne $acceptance) {
    $expectedMetrics = @{
        sc01ColdStartP95Seconds = 180
        sc01Samples = 10
        sc04AuthFailureSeconds = 10
        sc05TransferSizeMegabytes = 100
        sc05TransferRepeats = 3
        sc05ServerOverheadPercentMax = 5
        sc08MaxOverrunSeconds = 60
        sc08IdleStopGraceMinutes = 5
        sc08DownStopGraceSeconds = 60
        sc13DefconfigRuns = 2
        sc15FirstRunMinutes = 15
    }
    foreach ($key in $expectedMetrics.Keys) {
        if ([int]$acceptance.metrics.$key -ne [int]$expectedMetrics[$key]) {
            Fail "CONTRACT_ACCEPTANCE" "metrics.$key 必须为 $($expectedMetrics[$key])"
        }
    }
    if ((@($acceptance.metrics.sc07UnauthorizedStatuses) -join ",") -ne "401,403") {
        Fail "CONTRACT_ACCEPTANCE" "sc07UnauthorizedStatuses 必须为 401,403"
    }
}

# ===== 图形入口降级（0.3.3）：reserved 不阻塞 v1；转 required 必须进变更记录 =====
$graphicalKeys = @("ports.graphical", "ports.vnc", "graphical.framebuffer", "graphical.vncPasswordChars", "graphical.vncPasswordDesOnly")
$mandatoryKeys = @("bind.address", "bind.loopbackOnly", "bind.allowWildcardBind", "ports.terminal", "terminal.writeFlag", "terminal.singleClientFlag", "terminal.originCheckFlag")
if ($null -ne $entry) {
    foreach ($key in $mandatoryKeys) {
        if ([string]$entry.requiredness.$key -ne "required") {
            Fail "CONTRACT_ENTRY_REQUIREDNESS" "$key 必须为 required（终端入口与回环绑定不可降级）"
        }
    }
    $promoted = $false
    foreach ($key in $graphicalKeys) {
        $state = [string]$entry.requiredness.$key
        if ($state -eq "required") {
            $promoted = $true
        } elseif ($state -ne "reserved" -and $state -ne "optional") {
            Fail "CONTRACT_ENTRY_REQUIREDNESS" "$key 必须为 reserved/optional，当前为 $state"
        }
    }
    if (-not $promoted) {
        if ([string]$entry.graphical.status -ne "reserved") {
            Fail "CONTRACT_ENTRY_GRAPHICAL" "graphical.status 必须为 reserved"
        }
        if ([string]$entry.graphical.reason -notmatch "渲染方案待定") {
            Fail "CONTRACT_ENTRY_GRAPHICAL" "graphical.reason 必须说明渲染方案待定"
        }
    }
    if ([string]$entry.v1GraphicalFallback.status -ne "required" -or [string]$entry.v1GraphicalFallback.sshFlag -ne "-X") {
        Fail "CONTRACT_ENTRY_GRAPHICAL" "v1GraphicalFallback 必须为 required 且 sshFlag=-X"
    }
    if ((@($entry.v1GraphicalFallback.webCandidates) -join ",") -notmatch "KasmVNC" -or (@($entry.v1GraphicalFallback.webCandidates) -join ",") -notmatch "noVNC") {
        Fail "CONTRACT_ENTRY_GRAPHICAL" "webCandidates 必须包含 KasmVNC 与 noVNC 兜底"
    }
    $changeLog = Get-Content -LiteralPath "docs/变更记录.md" -Raw -Encoding UTF8
    if ($promoted -and -not [regex]::IsMatch($changeLog, "(?m)^#{2,3}\s+.*图形入口转正")) {
        Fail "CONTRACT_ENTRY_PROMOTION" "图形字段改为 required 时必须在变更记录新增标题含「图形入口转正」的条目"
    }
}
if ($null -ne $acceptance) {
    $acceptancePromoted = $false
    foreach ($metric in @($acceptance.graphical.optionalMetrics)) {
        $metricState = [string]$acceptance.requiredness.$metric
        if ($metricState -eq "required") {
            $acceptancePromoted = $true
        } elseif ($metricState -ne "optional") {
            Fail "CONTRACT_ACCEPTANCE_GRAPHICAL" "$metric 必须为 optional，当前为 $metricState"
        }
    }
    if (-not $acceptancePromoted -and [string]$acceptance.graphical.status -ne "reserved") {
        Fail "CONTRACT_ACCEPTANCE_GRAPHICAL" "acceptance.graphical.status 必须为 reserved"
    }
    if ($acceptancePromoted) {
        $changeLog = Get-Content -LiteralPath "docs/变更记录.md" -Raw -Encoding UTF8
        if (-not [regex]::IsMatch($changeLog, "(?m)^#{2,3}\s+.*图形入口转正")) {
            Fail "CONTRACT_ENTRY_PROMOTION" "图形指标改为 required 时必须在变更记录新增标题含「图形入口转正」的条目"
        }
    }
    if ($acceptance.metrics.sc07VncPasswordOnlyMustFail -ne $true) {
        Fail "CONTRACT_ACCEPTANCE_GRAPHICAL" "sc07VncPasswordOnlyMustFail 必须保留为 true（图形鉴权预留）"
    }
}

# ===== 轻量性预算（0.3.4）：已定项与 pending-calibration 项 =====
$lightweight = $acceptance.lightweight
if ($null -eq $lightweight) {
    Fail "CONTRACT_LIGHTWEIGHT" "acceptance.json 缺少 lightweight 预算"
} else {
    if ([string]$lightweight.status -ne "required") {
        Fail "CONTRACT_LIGHTWEIGHT" "lightweight.status 必须为 required"
    }
    $fixedBudgets = @{
        runnerResidentProcessCountMax = 4
        runnerNewPackageCountMax = 3
        graphicalComponentCountMax = 0
    }
    foreach ($key in $fixedBudgets.Keys) {
        if ([int]$lightweight.metrics.$key -ne [int]$fixedBudgets[$key]) {
            Fail "CONTRACT_LIGHTWEIGHT" "lightweight.metrics.$key 必须为 $($fixedBudgets[$key])"
        }
    }
    $pendingBudgets = @("releaseBinaryBytesMax", "localIdleResidentBytesMax", "runnerPreflightResidentBytesMax", "linkIdleResidentBytesMax", "cliColdStartOfflineMillisMax", "directDependencyCountMax")
    foreach ($key in $pendingBudgets) {
        if ([string]$lightweight.metrics.$key -ne "pending-calibration") {
            Fail "CONTRACT_LIGHTWEIGHT" "lightweight.metrics.$key 必须为 pending-calibration"
        }
        if ([string]::IsNullOrWhiteSpace([string]$lightweight.pendingCalibration.$key.neededMeasurement)) {
            Fail "CONTRACT_LIGHTWEIGHT" "lightweight.pendingCalibration.$key 缺少 neededMeasurement"
        }
        if ([string]::IsNullOrWhiteSpace([string]$lightweight.pendingCalibration.$key.calibratedBy)) {
            Fail "CONTRACT_LIGHTWEIGHT" "lightweight.pendingCalibration.$key 缺少 calibratedBy"
        }
    }
    foreach ($key in (@($fixedBudgets.Keys) + $pendingBudgets)) {
        if ([string]::IsNullOrWhiteSpace([string]$lightweight.methods.$key)) {
            Fail "CONTRACT_LIGHTWEIGHT" "lightweight.methods.$key 缺少测法（不可执行）"
        }
    }
}

# 入口端口只能来自契约；加载器不得硬编码
$entrySrc = "crates/meito-core/src/entry.rs"
if (-not (Test-Path -LiteralPath $entrySrc)) {
    Fail "CONTRACT_ENTRY_CODE" "缺少入口契约加载器 crates/meito-core/src/entry.rs"
} else {
    $entryText = Get-Content -LiteralPath $entrySrc -Raw -Encoding UTF8
    if ($entryText -notmatch [regex]::Escape("contracts/entry.json")) {
        Fail "CONTRACT_ENTRY_CODE" "entry.rs 未引用 contracts/entry.json"
    }
    foreach ($raw in @("7681", "6080", "5900")) {
        if ($entryText -match $raw) {
            Fail "CONTRACT_ENTRY_CODE" "entry.rs 不得硬编码端口 $raw"
        }
    }
}

# ===== 协调服务 link.json（0.3.x） =====
$link = Load-Json "contracts/link.json"
if ($null -ne $link) {
    if ([int]$link.channelTtlSeconds -ne 7200) { Fail "CONTRACT_LINK_TTL" "通道 TTL 必须为 7200 秒" }
    if ([string]$link.tokenHash.algorithm -ne "sha256" -or [string]$link.tokenHash.encoding -ne "hex" -or $link.tokenHash.logged -ne $false) {
        Fail "CONTRACT_LINK_HASH" "tokenHash 必须为 sha256/hex 且不落日志"
    }
    $registerRequired = @($link.registerPayload.required)
    foreach ($f in @("channel_id", "read_token_hash", "write_token_hash", "public_key")) {
        if ($registerRequired -notcontains $f) { Fail "CONTRACT_LINK_REGISTER" "registerPayload 缺少字段：$f" }
    }
    if ($registerRequired.Count -ne 4) { Fail "CONTRACT_LINK_REGISTER" "registerPayload 必须同时包含 4 个字段" }
    $repeatableUntil = @($link.getSemantics.repeatableUntil)
    foreach ($until in @("delete", "ttl")) {
        if ($repeatableUntil -notcontains $until) { Fail "CONTRACT_LINK_GET" "getSemantics.repeatableUntil 缺少：$until" }
    }
    if ([int]$link.getSemantics.maxReads -ne 10) { Fail "CONTRACT_LINK_GET" "getSemantics.maxReads 必须为 10" }
    if ([string]$link.tokens.readTokenIssuance -ne "once-per-channel" -or $link.tokens.readTokenOneTime -ne $false) {
        Fail "CONTRACT_LINK_GET" "读 token 必须每通道只签发一次，且不是只能读一次"
    }
    if ([int]$link.rateLimit.perChannelPerMinute -ne 60 -or [int]$link.rateLimit.perIpPerMinute -ne 120 -or [int]$link.rateLimit.burst -ne 10) {
        Fail "CONTRACT_LINK_RATE" "限流必须为每通道 60/分、每 IP 120/分、突发 10"
    }
    $expectedStatus = @{
        success = 200
        duplicate_channel = 400
        bad_token = 403
        unknown_channel = 404
        too_large = 413
        rate_limited = 429
        method_not_allowed = 405
        malformed = 400
        internal = 500
    }
    foreach ($key in $expectedStatus.Keys) {
        if ([int]$link.httpStatusMap.$key -ne [int]$expectedStatus[$key]) {
            Fail "CONTRACT_LINK_STATUS" "httpStatusMap.$key 必须为 $($expectedStatus[$key])"
        }
    }
    $expectedOps = @{
        register = @{ method = "POST"; auth = "none" }
        put = @{ method = "PUT"; auth = "write_token" }
        get = @{ method = "GET"; auth = "read_token" }
        delete = @{ method = "DELETE"; auth = "write_token" }
    }
    foreach ($name in $expectedOps.Keys) {
        if ([string]$link.operations.$name.method -ne $expectedOps[$name].method -or [string]$link.operations.$name.auth -ne $expectedOps[$name].auth) {
            Fail "CONTRACT_LINK_HTTP" "operations.$name 必须为 $($expectedOps[$name].method) / $($expectedOps[$name].auth)"
        }
        if ($link.operations.$name.PSObject.Properties.Name -contains "successStatus" -or $link.operations.$name.PSObject.Properties.Name -contains "status") {
            Fail "CONTRACT_LINK_HTTP" "operations.$name 不得复写状态码，必须引用 httpStatusMap"
        }
    }
    if ([string]$link.operations.delete.auth -ne "write_token") {
        Fail "CONTRACT_LINK_HTTP" "DELETE 必须用写 token 鉴权"
    }
    if ([string]$link.operations.register.duplicateLiveChannel -ne "duplicate_channel") {
        Fail "CONTRACT_LINK_HTTP" "重复注册必须引用 duplicate_channel（400）"
    }
    if ([string]$link.authHeader.name -ne "Authorization" -or [string]$link.authHeader.scheme -ne "Bearer" -or [string]$link.authHeader.format -ne "Authorization: Bearer <token>") {
        Fail "CONTRACT_LINK_HTTP" "鉴权头必须为 Authorization: Bearer <token>"
    }
    if ((@($link.authHeader.appliesTo) -join ",") -ne "put,get,delete") {
        Fail "CONTRACT_LINK_HTTP" "鉴权头必须作用于 put/get/delete"
    }
    if ($link.logging.onlyAllowedFields -ne $true) { Fail "CONTRACT_LINK_LOG" "logging.onlyAllowedFields 必须为 true" }
    foreach ($f in @("channel_id", "size", "status")) {
        if (@($link.logging.allowedFields) -notcontains $f) { Fail "CONTRACT_LINK_LOG" "logging.allowedFields 缺少：$f" }
    }
    foreach ($f in @("blob", "ciphertext", "digest", "hash", "sha256")) {
        if (@($link.logging.forbiddenContent) -notcontains $f) { Fail "CONTRACT_LINK_LOG" "logging.forbiddenContent 缺少：$f" }
    }
    if ($link.logging.blobLogged -ne $false -or $link.logging.hashLogged -ne $false) {
        Fail "CONTRACT_LINK_LOG" "blob 与摘要均不得落日志"
    }
    if ($link.PSObject.Properties.Name -contains "maxCiphertextBytes") {
        Fail "CONTRACT_LINK_LIMIT" "link.json 不得复写单包上限数值"
    }
    $limitRef = [string]$link.packageLimit.maxCiphertextBytesRef
    if ($limitRef -ne "connection-package.json#serverHandling.maxCiphertextBytes") {
        Fail "CONTRACT_LINK_LIMIT" "单包上限必须引用 connection-package.json#serverHandling.maxCiphertextBytes"
    }
    if ($null -ne $cp -and $null -eq $cp.serverHandling.maxCiphertextBytes) {
        Fail "CONTRACT_LINK_LIMIT" "引用的 serverHandling.maxCiphertextBytes 不存在"
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

# ===== 文档数值必须回溯到契约（通用规则） =====
function Add-ContractNumbers($value, $set) {
    if ($null -eq $value) { return }
    if ($value -is [bool]) { return }
    if ($value -is [string]) {
        if ($value -match '^\d+$') { $set[([string][int]$value)] = $true }
        return
    }
    if ($value -is [int] -or $value -is [long] -or $value -is [double] -or $value -is [decimal]) {
        $set[([string][int]$value)] = $true
        return
    }
    if ($value -is [System.Collections.IEnumerable]) {
        foreach ($item in $value) { Add-ContractNumbers $item $set }
        return
    }
    foreach ($prop in $value.PSObject.Properties) { Add-ContractNumbers $prop.Value $set }
}

function Get-DocSection([string]$path, [string]$startPattern, [string]$endPattern) {
    if (-not (Test-Path -LiteralPath $path)) { return "" }
    $lines = (Get-Content -LiteralPath $path -Raw -Encoding UTF8) -split '\r?\n'
    $inside = $false
    $buffer = @()
    foreach ($line in $lines) {
        if (-not $inside -and $line -match $startPattern) { $inside = $true; continue }
        if ($inside -and $line -match $endPattern) { break }
        if ($inside) { $buffer += $line }
    }
    return ($buffer -join "`n")
}

$contractNumbers = @{}
foreach ($contractFile in (Get-ChildItem -Path "contracts" -File -Filter *.json)) {
    try {
        Add-ContractNumbers (Get-Content -LiteralPath $contractFile.FullName -Raw -Encoding UTF8 | ConvertFrom-Json) $contractNumbers
    } catch {
        Fail "CONTRACT_DOC_NUMERIC" "$($contractFile.Name) 不是合法 JSON"
    }
}

$docSections = @(
    @{ File = "docs/design.md"; Start = "^##\s+2\.\s+成功判据"; End = "^##\s+3\.\s+用户使用流程" },
    @{ File = "docs/design.md"; Start = "^###\s+3\.2\s"; End = "^###\s+3\.2\.1\s" },
    @{ File = "docs/design.md"; Start = "^###\s+5\.6\s"; End = "^###\s+5\.7\s" },
    @{ File = "docs/design.md"; Start = "^###\s+5\.7\s"; End = "^###\s+5\.8\s" },
    @{ File = "docs/design.md"; Start = "^###\s+5\.10\s"; End = "^##\s+6\." },
    @{ File = "docs/design.md"; Start = "^###\s+8\.3\s"; End = "^###\s+8\.4\s" },
    @{ File = "docs/design.md"; Start = "^###\s+9\.4\s"; End = "^###\s+9\.5\s" },
    @{ File = "docs/design.md"; Start = "^###\s+10\.2\s"; End = "^依赖安装耗时" },
    @{ File = "docs/design.md"; Start = "^###\s+10\.3\s"; End = "^###\s+10\.4\s" },
    @{ File = "docs/design.md"; Start = "^###\s+10\.4\s"; End = "^###\s+10\.5\s" },
    @{ File = "docs/design.md"; Start = "^###\s+10\.7\s"; End = "^##\s+11\." },
    @{ File = "docs/接口冻结.md"; Start = "^##\s+5\.2\s"; End = "^##\s+5\.3\s" },
    @{ File = "docs/接口冻结.md"; Start = "^##\s+5\.3\s"; End = "^##\s+5\.4\s" },
    @{ File = "docs/接口冻结.md"; Start = "^##\s+5\.4\s"; End = "^##\s+5\.5\s" },
    @{ File = "docs/接口冻结.md"; Start = "^##\s+5\.5\s"; End = "^##\s+5\.6\s" },
    @{ File = "docs/接口冻结.md"; Start = "^##\s+5\.6\s"; End = "^##\s+6\." }
)
foreach ($section in $docSections) {
    $body = Get-DocSection $section.File $section.Start $section.End
    if ([string]::IsNullOrWhiteSpace($body)) {
        Fail "CONTRACT_DOC_NUMERIC" "无法定位文档段落：$($section.File) $($section.Start)"
        continue
    }
    foreach ($m in [regex]::Matches($body, '(\d+)`?\s*(ms|min|m|s|秒|分钟|小时|次|字节|端口|位|MB|KB|%)')) {
        $raw = [int]$m.Groups[1].Value
        $seconds = $null
        switch ($m.Groups[2].Value) {
            "秒" { $seconds = $raw }
            "s" { $seconds = $raw }
            "分钟" { $seconds = $raw * 60 }
            "min" { $seconds = $raw * 60 }
            "m" { $seconds = $raw * 60 }
            "小时" { $seconds = $raw * 3600 }
        }
        if ($null -ne $seconds) {
            if (-not $contractNumbers.ContainsKey([string]$seconds) -and -not $contractNumbers.ContainsKey([string]$raw)) {
                Fail "CONTRACT_DOC_NUMERIC" "$($section.File) 的 $($m.Value) 在 contracts 中无对应字段"
            }
        } elseif (-not $contractNumbers.ContainsKey([string]$raw)) {
            Fail "CONTRACT_DOC_NUMERIC" "$($section.File) 的 $($m.Value) 在 contracts 中无对应字段"
        }
    }
    foreach ($m in [regex]::Matches($body, '(?<![0-9A-Za-z])(\d{4,5})(?![0-9A-Za-z])')) {
        $port = $m.Groups[1].Value
        if (-not $contractNumbers.ContainsKey($port)) {
            Fail "CONTRACT_DOC_NUMERIC" "$($section.File) 的四位以上数值 $port 在 contracts 中无对应字段"
        }
    }
    foreach ($m in [regex]::Matches($body, '(\d+)x(\d+)x(\d+)')) {
        foreach ($group in @(1, 2, 3)) {
            $value = $m.Groups[$group].Value
            if (-not $contractNumbers.ContainsKey($value)) {
                Fail "CONTRACT_DOC_NUMERIC" "$($section.File) 的分辨率段 $value 在 contracts 中无对应字段"
            }
        }
    }
    foreach ($m in [regex]::Matches($body, '(?<!\d)(4\d\d|5\d\d)(?!\d)')) {
        $code = $m.Groups[1].Value
        if (-not $contractNumbers.ContainsKey($code)) {
            Fail "CONTRACT_DOC_NUMERIC" "$($section.File) 的状态码 $code 在 contracts 中无对应字段"
        }
    }
}

# 人工审计项必须在文档显式标注，且不得混入机器覆盖段落
$designText = Get-Content -LiteralPath "docs/design.md" -Raw -Encoding UTF8
foreach ($pattern in @("(?m)^###\s+10\.5\s.*人工审计", "(?m)^###\s+10\.6\s.*人工审计", "(?m)^依赖安装耗时（\*\*人工审计\*\*")) {
    if (-not [regex]::IsMatch($designText, $pattern)) {
        Fail "CONTRACT_DOC_AUDIT" "design.md 缺少人工审计标注：$pattern"
    }
}
foreach ($needle in @("机器覆盖段落", "人工审计项")) {
    if ($designText -notmatch [regex]::Escape($needle)) {
        Fail "CONTRACT_DOC_AUDIT" "design §5.9 缺少覆盖/审计清单：$needle"
    }
}

$linkDoc = Get-DocSection "docs/design.md" "^###\s+5\.7\s" "^###\s+5\.8\s"
foreach ($needle in @('Authorization: Bearer <token>', '读 token 只用于 `GET`', '写 token 用于 `PUT` 与 `DELETE`', '重复注册返回 `400`', '成功响应统一 `200`')) {
    if ($linkDoc -notmatch [regex]::Escape($needle)) {
        Fail "CONTRACT_LINK_DOC" "design §5.7 缺少：$needle"
    }
}

if ($failures.Count -gt 0) {
    Write-Host "FAIL: contract-check" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host " - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "PASS: contract-check（版本 $contractVersion；CLI/日志/workflow/状态机/错误码/artifact manifest/link 服务端语义/entry 端口/acceptance 指标与轻量预算一致；图形入口字段为 reserved/optional，不阻塞 v1；文档数值可回溯契约，人工审计项已显式标注）" -ForegroundColor Green
exit 0
