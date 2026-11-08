#!/usr/bin/env bash
# ===== Meito runner 远端脚本：Job Summary =====
# 用途：从结构化日志生成 presence-only 的 Job Summary；重复执行只替换受管区块，不重复追加；命中禁项时拒绝写入。
# 输入：logs/session-<id>.jsonl；contracts/workflow.json 的 outputs.jobSummary（字段与禁项）；可选环境 MEITO_READ_LINK_PRESENT/MEITO_WRITE_LINK_PRESENT/MEITO_EXPIRES_AT/MEITO_ARTIFACT_NAME；GITHUB_STEP_SUMMARY（缺省写 logs/summary-<id>.md）。
# 输出：Job Summary 受管区块；logs/session-<id>.jsonl 追加 summary.* 事件；不写 token/连接包。
# 退出码：0 写入成功；2 E_CONFIG（契约不可读）；8 E_ARTIFACT（命中禁项，拒绝写入）；12 E_LOGS_UNAVAILABLE（日志缺失或无有效行）；1 E_INTERNAL。
# 契约：contracts/workflow.json（jobSummary.fields/forbidden、artifactGuard.scanPatterns）、contracts/log.json（路径与阶段）、contracts/error-codes.json。
# 未实测：未在 GitHub Actions ubuntu-latest 实跑；本地 Git Bash 夹具验证见 docs/变更记录.md。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

meito_require_python

SESSION=$(meito_session_id)
MEITO_SESSION_LOG=$(meito_session_or_unknown "$SESSION")
meito_require_session "$SESSION"

EXIT_CONFIG=$(meito_error_exit E_CONFIG)
EXIT_ARTIFACT=$(meito_error_exit E_ARTIFACT)
EXIT_LOGS=$(meito_error_exit E_LOGS_UNAVAILABLE)
EXIT_INTERNAL=$(meito_error_exit E_INTERNAL)

PY=$(meito_python)
LOG_PATH="$MEITO_LOGS_DIR/session-$SESSION.jsonl"
TARGET="${GITHUB_STEP_SUMMARY:-$MEITO_LOGS_DIR/summary-$SESSION.md}"

if [[ ! -f "$LOG_PATH" ]]; then
  meito_fail failed_collect summary.logs_missing E_LOGS_UNAVAILABLE "$EXIT_LOGS" "结构化日志不存在：$LOG_PATH"
fi

RESULT_RC=0
RESULT=$("$PY" - "$MEITO_ROOT" "$MEITO_CONTRACTS_DIR" "$LOG_PATH" "$TARGET" "$SESSION" <<'PY'
import json
import os
import sys
from pathlib import Path

root, contracts_dir, log_path, target, session = sys.argv[1:6]
try:
    with open(os.path.join(contracts_dir, "workflow.json"), "r", encoding="utf-8") as handle:
        workflow = json.load(handle)
    with open(os.path.join(contracts_dir, "log.json"), "r", encoding="utf-8") as handle:
        log_contract = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)

records = []
try:
    with open(log_path, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                records.append(json.loads(line))
            except json.JSONDecodeError:
                continue
except OSError:
    sys.exit(12)
if not records:
    sys.exit(12)

last = records[-1]
stage = str(last.get("stage", ""))
if stage == "done":
    status = "success"
elif stage == "cancelled":
    status = "cancelled"
elif stage.startswith("failed_"):
    status = "failed"
elif stage == "orphaned":
    status = "orphaned"
else:
    status = "in_progress"


def env_flag(name, default):
    raw = os.environ.get(name, "")
    if raw.lower() in ("1", "true", "yes"):
        return "true"
    if raw.lower() in ("0", "false", "no"):
        return "false"
    return default


def event_presence(event_name):
    return any(record.get("event") == event_name for record in records)


entry = os.environ.get("MEITO_ENTRY", "") or str(last.get("entry", "")) or "unknown"
read_present = env_flag(
    "MEITO_READ_LINK_PRESENT",
    "true" if event_presence("link.read_issued") else "false",
)
write_present = env_flag(
    "MEITO_WRITE_LINK_PRESENT",
    "true" if event_presence("link.write_issued") else "false",
)
expires_at = os.environ.get("MEITO_EXPIRES_AT", "") or str(last.get("expires_at", "")) or "unknown"
artifact_name = os.environ.get("MEITO_ARTIFACT_NAME", "") or ("meito-artifacts-" + session)

values = {
    "session_id": session,
    "status": status,
    "entry": entry,
    "read_link_present": read_present,
    "write_link_present": write_present,
    "expires_at": expires_at,
    "artifact_name": artifact_name,
}
fields = workflow["outputs"]["jobSummary"]["fields"]

lines = ["<!-- meito:summary:start -->", "## Meito 会话摘要", "", "| 字段 | 值 |", "| --- | --- |"]
for field in fields:
    lines.append("| " + field + " | " + str(values.get(field, "unknown")) + " |")
lines.append("")
lines.append("只展示 presence 与过期时间；凭据、私钥与连接包不会写入摘要。")
lines.append("<!-- meito:summary:end -->")
block = "\n".join(lines) + "\n"

forbidden = list(workflow["outputs"]["jobSummary"]["forbidden"])
forbidden += list(workflow.get("artifactGuard", {}).get("scanPatterns", []))
lowered = block.lower()
for pattern in forbidden:
    if pattern and (pattern in block or pattern.lower() in lowered):
        sys.stderr.write("forbidden content: " + pattern + "\n")
        sys.exit(8)

target_path = Path(target)
target_path.parent.mkdir(parents=True, exist_ok=True)
existing = ""
if target_path.exists():
    existing = target_path.read_text(encoding="utf-8")
start = "<!-- meito:summary:start -->"
end = "<!-- meito:summary:end -->"
if start in existing and end in existing:
    prefix = existing.split(start, 1)[0]
    suffix = existing.split(end, 1)[1]
    content = prefix + block + suffix.lstrip("\n")
else:
    separator = "" if existing.endswith("\n") or not existing else "\n"
    content = existing + separator + block
target_path.write_text(content, encoding="utf-8")
print(str(target_path))
PY
) || RESULT_RC=$?

case "$RESULT_RC" in
  0) meito_event collecting summary.written info "" "target=$RESULT" >/dev/null ;;
  3) meito_fail failed_collect summary.contract_invalid E_CONFIG "$EXIT_CONFIG" "契约不可读，拒绝生成摘要" ;;
  8) meito_fail failed_collect summary.refused E_ARTIFACT "$EXIT_ARTIFACT" "摘要命中禁项，拒绝写入" ;;
  12) meito_fail failed_collect summary.logs_unavailable E_LOGS_UNAVAILABLE "$EXIT_LOGS" "日志无有效行或不可读" ;;
  *) meito_fail failed_collect summary.failed E_INTERNAL "$EXIT_INTERNAL" "生成摘要失败" ;;
esac

meito_note "Job Summary 已写入：$RESULT"
exit 0
