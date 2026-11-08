#!/usr/bin/env bash
# ===== Meito runner 远端脚本：预检 =====
# 用途：在安装工具与启动会话前做 fail-closed 预检：输入约束、契约版本一致性、tools.lock 结构、磁盘余量、磁盘可写、可选 UDP 可达性。
# 输入：workflow 输入（GITHUB_EVENT_PATH.inputs 或 MEITO_INPUT_*）；contracts/*.json；仓库根 meito.toml（缺失时按 failClosed 视为无 UDP 目标，不伪造通过）；gate.config.json 的 build.minFreeGB。
# 输出：logs/session-<id>.jsonl 追加 preflight.* 事件；失败原因只写阶段与错误码，不写秘密。
# 退出码：0 通过；2 E_USAGE/E_CONFIG；4 E_PREFLIGHT；6 E_UDP_BLOCKED；1 E_INTERNAL。
# 契约：contracts/workflow.json、contracts/contract-version.json、contracts/tools-lock.schema.json、contracts/meito-config.schema.json、contracts/udp-probe.json、contracts/error-codes.json、contracts/log.json。
# 未实测：UDP 回执与真实 runner 网络行为未实测；未在 GitHub Actions ubuntu-latest 实跑；本地 Git Bash 夹具验证见 docs/变更记录.md。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

meito_require_python

SESSION=$(meito_session_id)
MEITO_SESSION_LOG=$(meito_session_or_unknown "$SESSION")
meito_require_session "$SESSION"

EXIT_USAGE=$(meito_error_exit E_USAGE)
EXIT_CONFIG=$(meito_error_exit E_CONFIG)
EXIT_PREFLIGHT=$(meito_error_exit E_PREFLIGHT)
EXIT_UDP=$(meito_error_exit E_UDP_BLOCKED)
EXIT_INTERNAL=$(meito_error_exit E_INTERNAL)

PY=$(meito_python)

# ===== 1. workflow 输入约束 =====

INPUT_RC=0
INPUT_REPORT=$("$PY" - "$MEITO_ROOT" "$MEITO_CONTRACTS_DIR/workflow.json" <<'PY'
import json
import os
import re
import sys

root, workflow_path = sys.argv[1], sys.argv[2]
inputs = {}
event_path = os.environ.get("GITHUB_EVENT_PATH", "")
if event_path and os.path.exists(event_path):
    try:
        with open(event_path, "r", encoding="utf-8") as handle:
            event = json.load(handle)
    except (OSError, json.JSONDecodeError):
        sys.stderr.write("cannot parse GITHUB_EVENT_PATH\n")
        sys.exit(2)
    if isinstance(event.get("inputs"), dict):
        inputs = event["inputs"]


def value_of(name):
    value = inputs.get(name)
    if value is None:
        value = os.environ.get("MEITO_INPUT_" + name.upper())
    if value is None:
        value = os.environ.get("INPUT_" + name.upper())
    return "" if value is None else str(value)


with open(workflow_path, "r", encoding="utf-8") as handle:
    workflow = json.load(handle)

problems = []
uuid_pattern = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-4[0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$")
public_key_pattern = re.compile(r"^[A-Za-z0-9_-]+$")

for spec in workflow.get("inputs", []):
    name = spec["name"]
    kind = spec.get("type", "string")
    raw = value_of(name)
    if spec.get("required") and not raw:
        problems.append(name + ": 必填但为空")
        continue
    max_length = spec.get("maxLength")
    if isinstance(max_length, int) and len(raw) > max_length:
        problems.append(name + ": 超过契约 maxLength=" + str(max_length))
    if kind == "enum" and raw and raw not in spec.get("enum", []):
        problems.append(name + ": 不在契约枚举内")
    if kind == "string" and isinstance(spec.get("min"), int) and isinstance(spec.get("max"), int):
        if raw:
            if not raw.isdigit():
                problems.append(name + ": 必须是整数")
            elif not (spec["min"] <= int(raw) <= spec["max"]):
                problems.append(name + ": 超出契约范围 " + str(spec["min"]) + ".." + str(spec["max"]))
    if name == "session_id" and raw and not uuid_pattern.match(raw):
        problems.append("session_id: 不是 UUID v4")
    if name == "channel_public_key" and raw and not public_key_pattern.match(raw):
        problems.append("channel_public_key: 不是 base64url 无填充格式")
    if name == "ref" and not raw:
        problems.append("ref: 必填但为空")

flags_raw = value_of("flags")
if not flags_raw:
    problems.append("flags: 必填但为空")
else:
    try:
        flags = json.loads(flags_raw)
    except json.JSONDecodeError:
        problems.append("flags: 不是合法 JSON")
    else:
        if not isinstance(flags, dict):
            problems.append("flags: 必须是 JSON 对象")
        else:
            for field in workflow.get("inputs", []):
                if field.get("name") != "flags":
                    continue
                for required in field.get("fields", []):
                    if required not in flags:
                        problems.append("flags." + required + ": 契约要求存在")

if problems:
    for problem in problems:
        print(problem)
    sys.exit(2)
PY
) || INPUT_RC=$?

if [[ "$INPUT_RC" -ne 0 ]]; then
  printf '%s\n' "$INPUT_REPORT" >&2
  meito_fail preflight preflight.inputs_invalid E_USAGE "$EXIT_USAGE" "workflow 输入不满足 contracts/workflow.json"
fi

# ===== 2. 契约版本一致性 =====

VERSION_RC=0
VERSION_REPORT=$(meito_contract_check_version) || VERSION_RC=$?
if [[ "$VERSION_RC" -ne 0 ]]; then
  printf '%s\n' "$VERSION_REPORT" >&2
  meito_fail preflight preflight.contracts_invalid E_CONFIG "$EXIT_CONFIG" "契约版本不一致或契约文件缺失"
fi

# ===== 3. tools.lock 结构（是否 pinned 由 bootstrap 判定） =====

LOCK_FILE="${MEITO_TOOLS_LOCK:-$MEITO_ROOT/tools.lock.json}"
LOCK_RC=0
LOCK_REPORT=$("$PY" - "$LOCK_FILE" "$MEITO_CONTRACTS_DIR/tools-lock.schema.json" <<'PY'
import json
import re
import sys

lock_path, schema_path = sys.argv[1], sys.argv[2]
try:
    with open(schema_path, "r", encoding="utf-8") as handle:
        schema = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)
try:
    with open(lock_path, "r", encoding="utf-8") as handle:
        lock = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)

problems = []
allowed_versions = schema["properties"]["lockVersion"]["enum"]
if lock.get("lockVersion") not in allowed_versions:
    problems.append("lockVersion: 不在受支持版本 " + str(allowed_versions))
tools = lock.get("tools")
if not isinstance(tools, dict) or not tools:
    problems.append("tools: 必须是非空对象")
else:
    sha_pattern = re.compile(r"^[0-9a-f]{64}$")
    for name, entry in tools.items():
        if not isinstance(entry, dict):
            problems.append(name + ": 条目必须是对象")
            continue
        status = entry.get("status")
        if status not in ("pinned", "unpinned"):
            problems.append(name + ": status 非法")
        if status == "pinned":
            if not entry.get("version") or not entry.get("url"):
                problems.append(name + ": pinned 条目缺少 version/url")
            if not isinstance(entry.get("sha256"), str) or not sha_pattern.match(entry.get("sha256", "")):
                problems.append(name + ": pinned 条目 sha256 非法")

if problems:
    for problem in problems:
        print(problem)
    sys.exit(2)
PY
) || LOCK_RC=$?

if [[ "$LOCK_RC" -ne 0 ]]; then
  printf '%s\n' "$LOCK_REPORT" >&2
  meito_fail preflight preflight.tools_lock_invalid E_CONFIG "$EXIT_CONFIG" "tools.lock.json 缺失或不符合 contracts/tools-lock.schema.json"
fi

# ===== 4. 仓库配置（存在则必须可解析） =====

CONFIG_FILE="${MEITO_CONFIG:-$MEITO_ROOT/meito.toml}"
UDP_TARGET=""
if [[ -f "$CONFIG_FILE" ]]; then
  TOML_RC=0
  UDP_TARGET=$(meito_toml_get "$CONFIG_FILE" network udp_probe_target) || TOML_RC=$?
  case "$TOML_RC" in
    0) : ;;
    2) UDP_TARGET="" ;;
    3) meito_fail preflight preflight.config_invalid E_CONFIG "$EXIT_CONFIG" "meito.toml 无法解析" ;;
    4) meito_fail preflight preflight.config_invalid E_CONFIG "$EXIT_CONFIG" "当前 Python 缺少 tomllib，无法校验 meito.toml" ;;
    *) meito_fail preflight preflight.config_invalid E_CONFIG "$EXIT_CONFIG" "读取 meito.toml 失败" ;;
  esac
fi

# ===== 5. 磁盘余量（低于门槛先停后报） =====

DISK_TARGET="${MEITO_DISK_PATH:-$MEITO_ROOT}"
if ! FREE_BYTES=$(meito_free_bytes "$DISK_TARGET"); then
  meito_fail preflight preflight.disk_unavailable E_PREFLIGHT "$EXIT_PREFLIGHT" "无法读取磁盘余量：$DISK_TARGET"
fi
MIN_FREE_BYTES=$(meito_min_free_bytes) || meito_fail preflight preflight.disk_config_invalid E_CONFIG "$EXIT_CONFIG" "gate.config.json 缺少 build.minFreeGB"

if [[ "$FREE_BYTES" -lt "$MIN_FREE_BYTES" ]]; then
  meito_fail preflight preflight.disk_low E_PREFLIGHT "$EXIT_PREFLIGHT" "磁盘可用量低于阈值：先停后报" "free_bytes=$FREE_BYTES" "threshold_bytes=$MIN_FREE_BYTES" "path=$DISK_TARGET"
fi

# ===== 6. 磁盘可写探针 =====

PROBE_FILE="$MEITO_LOGS_DIR/.meito-preflight-${SESSION}.tmp"
mkdir -p "$MEITO_LOGS_DIR"
if ! printf 'meito-preflight' > "$PROBE_FILE"; then
  meito_fail preflight preflight.disk_write_failed E_PREFLIGHT "$EXIT_PREFLIGHT" "日志目录不可写：$MEITO_LOGS_DIR"
fi
if ! rm -f "$PROBE_FILE"; then
  meito_fail preflight preflight.disk_write_failed E_PREFLIGHT "$EXIT_PREFLIGHT" "探针文件无法删除：$PROBE_FILE"
fi

# ===== 7. 可选 UDP 探测 =====

DNS_HOST="${MEITO_DNS_HOST:-}"
if [[ -n "$DNS_HOST" ]]; then
  if ! "$PY" - "$DNS_HOST" <<'PY'
import socket
import sys

try:
    socket.getaddrinfo(sys.argv[1], None)
except OSError:
    sys.exit(4)
PY
  then
    meito_fail preflight preflight.dns_failed E_PREFLIGHT "$EXIT_PREFLIGHT" "DNS 预检失败：$DNS_HOST"
  fi
fi

if [[ -n "$UDP_TARGET" ]]; then
  UDP_RC=0
  UDP_REPORT=$("$PY" - "$MEITO_CONTRACTS_DIR/udp-probe.json" "$UDP_TARGET" <<'PY'
import json
import secrets
import socket
import sys

contract_path, target = sys.argv[1], sys.argv[2]
with open(contract_path, "r", encoding="utf-8") as handle:
    contract = json.load(handle)

host, _, port_text = target.rpartition(":")
if not host or not port_text.isdigit():
    sys.stderr.write("invalid udp probe target\n")
    sys.exit(3)
port = int(port_text)
attempts = int(contract["timeout"]["attempts"])
timeout_seconds = int(contract["timeout"]["perAttemptMs"]) / 1000.0
magic = contract["request"]["magic"].encode("utf-8")
nonce_bytes = int(contract["request"]["nonceBytes"])

try:
    addresses = socket.getaddrinfo(host, port, type=socket.SOCK_DGRAM)
except OSError as error:
    sys.stderr.write("dns: " + str(error) + "\n")
    sys.exit(4)
if not addresses:
    sys.exit(4)

family, socktype, proto, _, address = addresses[0]
sock = socket.socket(family, socktype, proto)
sock.settimeout(timeout_seconds)
last = ""
for _ in range(attempts):
    packet = magic + secrets.token_bytes(nonce_bytes)
    try:
        sock.sendto(packet, address)
        data, _peer = sock.recvfrom(64)
    except OSError as error:
        last = str(error)
        continue
    if data == packet:
        sock.close()
        print("ok")
        sys.exit(0)
    last = "unexpected reply"
sock.close()
sys.stderr.write("udp blocked: " + last + "\n")
sys.exit(6)
PY
  ) || UDP_RC=$?

  case "$UDP_RC" in
    0) meito_event preflight preflight.udp_ok info "" "target=$UDP_TARGET" >/dev/null ;;
    4) meito_fail preflight preflight.udp_dns_failed E_PREFLIGHT "$EXIT_PREFLIGHT" "UDP 目标 DNS 解析失败" "target=$UDP_TARGET" ;;
    6) meito_fail preflight preflight.udp_blocked E_UDP_BLOCKED "$EXIT_UDP" "UDP 探测未收到回执：禁止继续" "target=$UDP_TARGET" ;;
    3) meito_fail preflight preflight.udp_target_invalid E_CONFIG "$EXIT_CONFIG" "udp_probe_target 不是 host:port" ;;
    *) meito_fail preflight preflight.udp_failed E_PREFLIGHT "$EXIT_PREFLIGHT" "UDP 探测失败" ;;
  esac
else
  meito_event preflight preflight.udp_skipped info "" "reason=target_not_configured" >/dev/null
fi

meito_event preflight preflight.completed info "" "free_bytes=$FREE_BYTES" "threshold_bytes=$MIN_FREE_BYTES" >/dev/null
meito_note "预检通过：磁盘可用 $FREE_BYTES 字节（阈值 $MIN_FREE_BYTES）"
exit 0
