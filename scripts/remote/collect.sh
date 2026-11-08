#!/usr/bin/env bash
# ===== Meito runner 远端脚本：成果收集 =====
# 用途：按 artifact_spec 收集成果，先做路径白名单与禁项扫描，命中即阻断；生成 artifact-manifest.json、确定性归档与 env ledger。
# 输入：flags.artifact_spec（字符串或 JSON 数组）或 MEITO_ARTIFACT_SPEC；contracts/workflow.json 的 artifactGuard；contracts/artifact-manifest.schema.json；contracts/entry.json 的 ports；contracts/log.json 的台账字段。
# 输出：artifact-manifest.json（覆盖写）、logs/artifacts-<id>.tar（确定性归档）、logs/env-session-<id>.json（合并写）、logs/session-<id>.jsonl 追加 collect.* 事件。
# 退出码：0 收集且门禁通过；2 E_USAGE（artifact_spec 为空）；8 E_ARTIFACT（路径越界、禁项命中、成果缺失或 base commit 不可用）；1 E_INTERNAL。
# 契约：contracts/workflow.json、contracts/artifact-manifest.schema.json、contracts/entry.json、contracts/log.json、contracts/error-codes.json。
# 未实测：未在 GitHub Actions ubuntu-latest 实跑；真实 artifact 上传未实测；本地 Git Bash 夹具验证见 docs/变更记录.md。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

meito_require_python

SESSION=$(meito_session_id)
MEITO_SESSION_LOG=$(meito_session_or_unknown "$SESSION")
meito_require_session "$SESSION"

EXIT_USAGE=$(meito_error_exit E_USAGE)
EXIT_ARTIFACT=$(meito_error_exit E_ARTIFACT)
EXIT_INTERNAL=$(meito_error_exit E_INTERNAL)

PY=$(meito_python)

ARTIFACT_SPEC=$(meito_flag artifact_spec) || ARTIFACT_SPEC=""
if [[ -z "$ARTIFACT_SPEC" ]]; then
  ARTIFACT_SPEC="${MEITO_ARTIFACT_SPEC:-}"
fi
if [[ -z "$ARTIFACT_SPEC" ]]; then
  meito_fail failed_collect collect.spec_missing E_USAGE "$EXIT_USAGE" "flags.artifact_spec 为空；拒绝生成空清单"
fi

BASE_COMMIT=""
if ! BASE_COMMIT=$(git -C "$MEITO_ROOT" rev-parse HEAD 2>/dev/null); then
  meito_fail failed_collect collect.base_commit_unavailable E_ARTIFACT "$EXIT_ARTIFACT" "无法读取 base commit；拒绝生成不可复算的清单"
fi

RUN_ID="${GITHUB_RUN_ID:-local-$SESSION}"

RESULT_RC=0
RESULT=$("$PY" - "$MEITO_ROOT" "$MEITO_CONTRACTS_DIR" "$MEITO_LOGS_DIR" "$SESSION" "$RUN_ID" "$BASE_COMMIT" "$ARTIFACT_SPEC" <<'PY'
import fnmatch
import glob
import hashlib
import json
import os
import platform
import tarfile
import sys
from datetime import datetime, timezone
from pathlib import Path

root, contracts_dir, logs_dir, session, run_id, base_commit, spec_raw = sys.argv[1:8]


def load_json(path):
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


try:
    workflow = load_json(os.path.join(contracts_dir, "workflow.json"))
    manifest_schema = load_json(os.path.join(contracts_dir, "artifact-manifest.schema.json"))
    entry_contract = load_json(os.path.join(contracts_dir, "entry.json"))
    log_contract = load_json(os.path.join(contracts_dir, "log.json"))
except (OSError, json.JSONDecodeError):
    sys.stderr.write("contract unreadable\n")
    sys.exit(3)

guard = workflow.get("artifactGuard", {})
whitelist = list(guard.get("pathWhitelist", []))
scan_patterns = list(guard.get("scanPatterns", []))
block_on_hit = bool(guard.get("blockOnHit", True))

patterns = []
try:
    parsed = json.loads(spec_raw)
except json.JSONDecodeError:
    parsed = spec_raw
if isinstance(parsed, str):
    patterns = [line.strip() for line in parsed.splitlines() if line.strip()]
elif isinstance(parsed, list):
    patterns = [str(item).strip() for item in parsed if str(item).strip()]
if not patterns:
    sys.stderr.write("artifact spec empty\n")
    sys.exit(2)

violations = []
records = []
seen = set()
for pattern in patterns:
    matches = sorted(glob.glob(os.path.join(root, pattern), recursive=True))
    if not matches:
        violations.append("artifact_missing:" + pattern)
        continue
    for match in matches:
        path = Path(match)
        if path.is_dir():
            continue
        try:
            rel = path.resolve().relative_to(Path(root).resolve()).as_posix()
        except ValueError:
            violations.append("path_outside_repo:" + match)
            continue
        if rel in seen:
            continue
        seen.add(rel)
        allowed = any(
            fnmatch.fnmatch(rel, item) or fnmatch.fnmatch(path.name, item)
            for item in whitelist
        )
        if not allowed:
            violations.append("path_not_whitelisted:" + rel)
            continue
        try:
            data = path.read_bytes()
        except OSError:
            violations.append("unreadable:" + rel)
            continue
        text = data.decode("utf-8", errors="ignore").lower()
        for pattern_word in scan_patterns:
            if pattern_word and pattern_word.lower() in text:
                violations.append("content_pattern:" + rel + ":pattern=" + pattern_word)
                break
        digest = hashlib.sha256(data).hexdigest()
        records.append({"path": rel, "sha256": digest, "size": len(data), "abs": str(path)})

records.sort(key=lambda item: item["path"])
files_sha = hashlib.sha256()
for record in records:
    files_sha.update((record["sha256"] + "  " + record["path"] + "\n").encode("utf-8"))

logs_path = Path(logs_dir)
logs_path.mkdir(parents=True, exist_ok=True)
archive_path = logs_path / ("artifacts-" + session + ".tar")
with tarfile.open(archive_path, "w") as archive:
    for record in records:
        info = tarfile.TarInfo(record["path"])
        info.size = record["size"]
        info.mtime = 0
        info.mode = 0o644
        info.uid = 0
        info.gid = 0
        info.uname = ""
        info.gname = ""
        with open(record["abs"], "rb") as handle:
            archive.addfile(info, handle)

archive_sha = hashlib.sha256(archive_path.read_bytes()).hexdigest()

manifest_version = manifest_schema["properties"]["manifestVersion"]["enum"][0]
session_log_path = log_contract["path"].replace("<id>", session)
env_ledger_path = log_contract["envLedgerPath"].replace("<id>", session)
manifest = {
    "manifestVersion": manifest_version,
    "sessionId": session,
    "runId": run_id,
    "baseCommit": base_commit,
    "createdAt": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "files": [{key: record[key] for key in ("path", "sha256", "size")} for record in records],
    "logs": {"sessionLog": session_log_path, "envLedger": env_ledger_path},
    "checksums": {"filesSha256": files_sha.hexdigest(), "archiveSha256": archive_sha},
    "guardPassed": False,
}
if violations:
    manifest["guardViolations"] = violations
else:
    manifest["guardPassed"] = True

manifest_path = os.path.join(root, "artifact-manifest.json")
tmp_manifest = manifest_path + ".tmp"
with open(tmp_manifest, "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, ensure_ascii=False, indent=2, sort_keys=True)
    handle.write("\n")
os.replace(tmp_manifest, manifest_path)

# 运行环境台账：只写契约声明的字段；先读旧值再合并，重复执行不重复键。
ledger_path = Path(logs_dir) / os.path.basename(env_ledger_path)
ledger = {}
if ledger_path.exists():
    try:
        with open(ledger_path, "r", encoding="utf-8") as handle:
            loaded = json.load(handle)
        if isinstance(loaded, dict):
            ledger = loaded
    except (OSError, json.JSONDecodeError):
        ledger = {}

tool_versions = {}
tools_manifest = Path(logs_dir) / "tools-installed.json"
if tools_manifest.exists():
    try:
        with open(tools_manifest, "r", encoding="utf-8") as handle:
            installed = json.load(handle)
        for name, entry in installed.get("tools", {}).items():
            tool_versions[name] = entry.get("version")
    except (OSError, json.JSONDecodeError):
        tool_versions = {}

try:
    contract_version = load_json(os.path.join(contracts_dir, "contract-version.json"))
except (OSError, json.JSONDecodeError):
    contract_version = {}

udp_report = {"status": "not_run"}
udp_path = Path(logs_dir) / ("udp-probe-" + session + ".json")
if udp_path.exists():
    try:
        with open(udp_path, "r", encoding="utf-8") as handle:
            udp_report = json.load(handle)
    except (OSError, json.JSONDecodeError):
        udp_report = {"status": "unreadable"}

try:
    exit_code = int(os.environ.get("MEITO_EXIT_CODE", "0"))
except ValueError:
    exit_code = 0

values = {
    "session_id": session,
    "run_id": run_id,
    "image": os.environ.get("MEITO_IMAGE", "unknown"),
    "runner_label": os.environ.get("RUNNER_NAME", os.environ.get("MEITO_RUNNER_LABEL", "unknown")),
    "os": platform.platform(),
    "meito_version": os.environ.get("MEITO_VERSION", contract_version.get("contractVersion", "unknown")),
    "frpc_version": tool_versions.get("frpc"),
    "ttyd_version": tool_versions.get("ttyd"),
    "x11vnc_version": tool_versions.get("x11vnc"),
    "websockify_version": tool_versions.get("websockify"),
    "p2p_provider": os.environ.get("MEITO_P2P_PROVIDER", "unknown"),
    "p2p_state": os.environ.get("MEITO_P2P_STATE", "not_verified"),
    "udp_probe": udp_report,
    "ice_candidates": ledger.get("ice_candidates", []),
    "entry_ports": entry_contract.get("ports", {}),
    "exit_code": exit_code,
    "error_code": os.environ.get("MEITO_ERROR_CODE", ""),
}
ledger = {field: values.get(field) for field in log_contract.get("envLedgerFields", [])}
tmp_ledger = str(ledger_path) + ".tmp"
with open(tmp_ledger, "w", encoding="utf-8") as handle:
    json.dump(ledger, handle, ensure_ascii=False, indent=2, sort_keys=True)
os.replace(tmp_ledger, str(ledger_path))

print(json.dumps({
    "files": len(records),
    "violations": violations,
    "manifest": manifest_path,
    "archive": str(archive_path),
    "ledger": str(ledger_path),
    "guardPassed": manifest["guardPassed"],
}, ensure_ascii=False, separators=(",", ":")))
sys.exit(8 if violations else 0)
PY
) || RESULT_RC=$?

if [[ "$RESULT_RC" -eq 0 ]]; then
  FILES_COUNT=$("$PY" -c 'import json,sys; print(json.loads(sys.argv[1])["files"])' "$RESULT")
  meito_event collecting collect.completed info "" "files=$FILES_COUNT" >/dev/null
  meito_note "成果收集完成：$FILES_COUNT 个文件通过门禁"
  exit 0
fi

case "$RESULT_RC" in
  8)
    VIOLATION_COUNT=$("$PY" -c 'import json,sys; print(len(json.loads(sys.argv[1])["violations"]))' "$RESULT" 2>/dev/null) || VIOLATION_COUNT=1
    meito_fail failed_collect collect.guard_failed E_ARTIFACT "$EXIT_ARTIFACT" "成果门禁未通过；不生成可上传的通过结论" "violations=$VIOLATION_COUNT"
    ;;
  3) meito_fail failed_collect collect.contract_invalid E_INTERNAL "$EXIT_INTERNAL" "契约不可读，拒绝收集" ;;
  2) meito_fail failed_collect collect.spec_empty E_USAGE "$EXIT_USAGE" "artifact_spec 未匹配任何可用模式" ;;
  *) meito_fail failed_collect collect.failed E_INTERNAL "$EXIT_INTERNAL" "成果收集内部失败" ;;
esac
