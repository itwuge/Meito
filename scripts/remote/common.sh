# shellcheck shell=bash
# ===== Meito runner 远端脚本公共库 =====
# 用途：为 scripts/remote/*.sh 提供严格模式、契约读取、结构化 JSONL 日志、SHA256、退出码与失败退出。
# 输入：被 source；读取 contracts/log.json、contracts/error-codes.json、contracts/workflow.json，以及 GITHUB_EVENT_PATH 或 MEITO_INPUT_* 环境变量。
# 退出码：库函数不主动终止进程；meito_fail 使用调用方给出的契约退出码。
# 契约：contracts/log.json（字段/路径/阶段枚举/脱敏）、contracts/error-codes.json（错误码与退出码）、contracts/workflow.json（输入约束）。
# 未实测：未在 GitHub Actions ubuntu-latest 实跑；本地 Git Bash 夹具验证见 docs/变更记录.md。

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  printf '[meito] common.sh 只能被 source，不能独立执行\n' >&2
  exit 2
fi

# ===== 路径与常量 =====

MEITO_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MEITO_ROOT="$(cd "$MEITO_LIB_DIR/../.." && pwd)"
MEITO_CONTRACTS_DIR="$MEITO_ROOT/contracts"
MEITO_LOGS_DIR="${MEITO_LOGS_DIR:-$MEITO_ROOT/logs}"
MEITO_PY=""
MEITO_SESSION_LOG=""

# ===== 基础输出 =====

meito_note() {
  local message="$1"
  if [[ "${MEITO_QUIET:-0}" != "1" ]]; then
    printf '[meito] %s\n' "$message" >&2
  fi
}

meito_abort() {
  local message="$1"
  printf '[meito] %s\n' "$message" >&2
  exit 1
}

# ===== Python 解释器 =====

meito_python() {
  local candidate
  if [[ -n "$MEITO_PY" ]]; then
    printf '%s' "$MEITO_PY"
    return 0
  fi
  for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import sys' >/dev/null 2>&1; then
      MEITO_PY="$candidate"
      printf '%s' "$MEITO_PY"
      return 0
    fi
  done
  return 1
}

meito_require_python() {
  if ! meito_python >/dev/null; then
    meito_abort "未找到可用的 python3/python；脚本依赖契约 JSON 解析，拒绝继续"
  fi
}

# ===== 契约读取 =====

meito_json_get() {
  local file="$1"
  local dotted="$2"
  local py
  py=$(meito_python) || return 1
  "$py" - "$file" "$dotted" <<'PY'
import json
import sys

file_path, dotted = sys.argv[1], sys.argv[2]
try:
    with open(file_path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)

node = data
for part in dotted.split("."):
    if isinstance(node, dict) and part in node:
        node = node[part]
    elif isinstance(node, list) and part.isdigit() and int(part) < len(node):
        node = node[int(part)]
    else:
        sys.exit(2)

if node is None:
    print("")
elif isinstance(node, bool):
    print("true" if node else "false")
elif isinstance(node, (dict, list)):
    print(json.dumps(node, ensure_ascii=False, separators=(",", ":")))
else:
    print(node)
PY
}

meito_json_list() {
  local file="$1"
  local dotted="$2"
  local py
  py=$(meito_python) || return 1
  "$py" - "$file" "$dotted" <<'PY'
import json
import sys

file_path, dotted = sys.argv[1], sys.argv[2]
try:
    with open(file_path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)

node = data
for part in dotted.split("."):
    if isinstance(node, dict) and part in node:
        node = node[part]
    else:
        sys.exit(2)
if not isinstance(node, list):
    sys.exit(2)
for item in node:
    if item is None:
        print("")
    elif isinstance(item, bool):
        print("true" if item else "false")
    elif isinstance(item, (dict, list)):
        print(json.dumps(item, ensure_ascii=False, separators=(",", ":")))
    else:
        print(item)
PY
}

meito_list_from_text() {
  local raw="$1"
  local py
  py=$(meito_python) || return 1
  "$py" - "$raw" <<'PY'
import json
import sys

raw = sys.argv[1]
try:
    data = json.loads(raw)
except json.JSONDecodeError:
    sys.exit(2)
if isinstance(data, str):
    data = [data]
if not isinstance(data, list):
    sys.exit(2)
for item in data:
    if isinstance(item, str):
        print(item)
PY
}

meito_trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

meito_error_exit() {
  local name="$1"
  local py value
  py=$(meito_python) || return 1
  value=$("$py" - "$MEITO_CONTRACTS_DIR/error-codes.json" "$name" <<'PY'
import json
import sys

file_path, name = sys.argv[1], sys.argv[2]
try:
    with open(file_path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)
for entry in data.get("codes", []):
    if entry.get("code") == name:
        print(entry["exitCode"])
        sys.exit(0)
sys.exit(2)
PY
) || return 1
  printf '%s' "$value"
}

meito_contract_check_version() {
  local py
  py=$(meito_python) || return 1
  "$py" - "$MEITO_CONTRACTS_DIR" <<'PY'
import json
import os
import sys

contracts_dir = sys.argv[1]
try:
    with open(os.path.join(contracts_dir, "contract-version.json"), "r", encoding="utf-8") as handle:
        version_contract = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)

expected = version_contract.get("contractVersion")
problems = []
for name in version_contract.get("files", []):
    path = os.path.join(contracts_dir, name)
    if not os.path.exists(path):
        problems.append("missing contract file: " + name)
        continue
    try:
        with open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, json.JSONDecodeError):
        problems.append("unreadable contract file: " + name)
        continue
    if "contractVersion" not in data:
        continue
    if data["contractVersion"] != expected:
        problems.append("contract version mismatch: " + name + "=" + str(data["contractVersion"]) + " expected " + str(expected))

if not isinstance(expected, str) or not expected:
    problems.append("contract-version.json has no contractVersion")
for problem in problems:
    print(problem)
sys.exit(2 if problems else 0)
PY
}

meito_toml_get() {
  local file="$1"
  local section="$2"
  local key="$3"
  local py
  py=$(meito_python) || return 1
  "$py" - "$file" "$section" "$key" <<'PY'
import sys

try:
    import tomllib
except ImportError:
    sys.exit(4)

file_path, section, key = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    with open(file_path, "rb") as handle:
        data = tomllib.load(handle)
except Exception:
    sys.exit(3)

node = data
if section:
    if not isinstance(node, dict) or section not in node:
        sys.exit(2)
    node = node[section]
if not isinstance(node, dict) or key not in node:
    sys.exit(2)

value = node[key]
if value is None:
    print("")
elif isinstance(value, bool):
    print("true" if value else "false")
elif isinstance(value, list):
    for item in value:
        print(item)
else:
    print(value)
PY
}

# ===== 工作流输入 =====

meito_input() {
  local name="$1"
  local default="${2-}"
  local value=""
  local env_name
  if [[ -n "${GITHUB_EVENT_PATH:-}" && -f "${GITHUB_EVENT_PATH}" ]]; then
    if value=$(meito_json_get "$GITHUB_EVENT_PATH" "inputs.$name" 2>/dev/null); then
      :
    else
      value=""
    fi
  fi
  if [[ -z "$value" ]]; then
    env_name="MEITO_INPUT_${name^^}"
    value="${!env_name-}"
  fi
  if [[ -z "$value" ]]; then
    env_name="INPUT_${name^^}"
    value="${!env_name-}"
  fi
  if [[ -z "$value" ]]; then
    value="$default"
  fi
  printf '%s' "$value"
}

meito_flag() {
  local key="$1"
  local default="${2-}"
  local flags value env_name py
  flags=$(meito_input flags)
  if [[ -n "$flags" ]]; then
    py=$(meito_python) || return 1
    value=$("$py" - "$key" "$flags" <<'PY'
import json
import sys

key, raw = sys.argv[1], sys.argv[2]
try:
    data = json.loads(raw)
except json.JSONDecodeError:
    sys.exit(2)
if not isinstance(data, dict):
    sys.exit(2)
if key not in data:
    sys.exit(3)
value = data[key]
if value is None:
    print("")
elif isinstance(value, bool):
    print("true" if value else "false")
elif isinstance(value, (dict, list)):
    print(json.dumps(value, ensure_ascii=False, separators=(",", ":")))
else:
    print(value)
PY
) || return 2
    printf '%s' "$value"
    return 0
  fi
  env_name="MEITO_FLAG_${key^^}"
  value="${!env_name-}"
  if [[ -z "$value" ]]; then
    value="$default"
  fi
  printf '%s' "$value"
}

# ===== 会话与日志 =====

meito_session_id() {
  local session
  session=$(meito_input session_id)
  if [[ -z "$session" ]]; then
    session="${MEITO_SESSION_ID:-}"
  fi
  printf '%s' "$session"
}

meito_is_uuid_v4() {
  local value="$1"
  [[ "$value" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-4[0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$ ]]
}

meito_session_or_unknown() {
  local value="$1"
  if meito_is_uuid_v4 "$value"; then
    printf '%s' "$value"
  else
    printf 'unknown'
  fi
}

meito_require_session() {
  local session="$1"
  local exit_usage
  exit_usage=$(meito_error_exit E_USAGE) || meito_abort "契约 error-codes.json 不可读"
  if ! meito_is_uuid_v4 "$session"; then
    meito_fail preflight usage.invalid_session E_USAGE "$exit_usage" "session_id 必须是 UUID v4，收到非契约格式"
  fi
}

meito_event() {
  local stage="$1"
  local event="$2"
  local level="$3"
  local code="$4"
  shift 4
  local py
  py=$(meito_python) || return 1
  "$py" - "$MEITO_CONTRACTS_DIR" "$MEITO_LOGS_DIR" "$MEITO_SESSION_LOG" "$stage" "$event" "$level" "$code" "$@" <<'PY'
import json
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

contracts_dir, logs_dir, session, stage, event, level, code = sys.argv[1:8]
extras = sys.argv[8:]

with open(os.path.join(contracts_dir, "log.json"), "r", encoding="utf-8") as handle:
    contract = json.load(handle)
fields = {entry["name"]: entry for entry in contract["requiredFields"]}
stages = fields["stage"]["enum"]
levels = fields["level"]["enum"]
event_pattern = re.compile(fields["event"]["pattern"])

if stage not in stages:
    sys.stderr.write("unknown log stage: " + stage + "\n")
    sys.exit(2)
if level not in levels:
    sys.stderr.write("unknown log level: " + level + "\n")
    sys.exit(2)
if not event_pattern.match(event):
    sys.stderr.write("invalid log event: " + event + "\n")
    sys.exit(2)

redaction = contract.get("redaction", {})
redact_keys = {key.lower() for key in redaction.get("keys", [])}
replacement = redaction.get("replacement", "[REDACTED]")
allowed_names = {entry["name"] for entry in contract["requiredFields"]}
allowed_names |= {entry["name"] for entry in contract.get("optionalFields", [])}
evidence = []
path_template = contract["path"]
log_path = Path(logs_dir) / Path(path_template.replace("<id>", session)).name

record = {
    "ts": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z",
    "level": level,
    "session_id": session,
    "stage": stage,
    "event": event,
    "error_code": code,
}
for item in extras:
    if "=" not in item:
        continue
    key, value = item.split("=", 1)
    if key in record:
        continue
    if key.lower() in redact_keys:
        value = replacement
    if key in allowed_names:
        record[key] = value
    else:
        evidence.append(key + "=" + value)
if evidence:
    record["evidence"] = "; ".join(evidence)

log_path.parent.mkdir(parents=True, exist_ok=True)
with open(log_path, "a", encoding="utf-8", newline="\n") as handle:
    handle.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n")
print(str(log_path))
PY
}

meito_fail() {
  local stage="$1"
  local event="$2"
  local code="$3"
  local exit_code="$4"
  local message="$5"
  shift 5
  if ! meito_event "$stage" "$event" "error" "$code" "$@"; then
    printf '[meito] 结构化日志写入失败；仍以非零退出：%s\n' "$message" >&2
    exit 1
  fi
  printf '[meito] %s（%s，退出码 %s）\n' "$event" "$code" "$exit_code" >&2
  if [[ -n "$message" ]]; then
    printf '[meito] %s\n' "$message" >&2
  fi
  exit "$exit_code"
}

# ===== 文件与磁盘 =====

meito_sha256() {
  local file="$1"
  local output py
  if command -v sha256sum >/dev/null 2>&1; then
    output=$(sha256sum "$file") || return 1
    printf '%s' "${output%% *}"
    return 0
  fi
  py=$(meito_python) || return 1
  "$py" - "$file" <<'PY'
import hashlib
import sys

digest = hashlib.sha256()
with open(sys.argv[1], "rb") as handle:
    for chunk in iter(lambda: handle.read(1048576), b""):
        digest.update(chunk)
print(digest.hexdigest())
PY
}

meito_free_bytes() {
  local target="$1"
  local py
  py=$(meito_python) || return 1
  "$py" - "$target" <<'PY'
import shutil
import sys

print(shutil.disk_usage(sys.argv[1]).free)
PY
}

meito_min_free_bytes() {
  local config="$MEITO_ROOT/gate.config.json"
  local py value
  py=$(meito_python) || return 1
  value=$("$py" - "$config" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], "r", encoding="utf-8") as handle:
        data = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)
min_gb = data.get("build", {}).get("minFreeGB")
if not isinstance(min_gb, int) or min_gb < 0:
    sys.exit(2)
print(min_gb * 1024 * 1024 * 1024)
PY
) || return 1
  printf '%s' "$value"
}

meito_now_utc() {
  date -u '+%Y-%m-%dT%H:%M:%SZ'
}
