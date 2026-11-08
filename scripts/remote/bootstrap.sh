#!/usr/bin/env bash
# ===== Meito runner 远端脚本：工具引导 =====
# 用途：只按 tools.lock.json 的 pinned 条目安装工具；整批先判定再动作，任一 unpinned 立即拒绝，下载后必须校验 SHA256。
# 输入：tools.lock.json（默认仓库根）；MEITO_BOOTSTRAP_TOOLS（空格分隔的可选子集，默认全部条目）；MEITO_TOOLS_DIR（默认 $HOME/.meito/tools）；MEITO_TOOLS_LOCK。
# 输出：工具写入 $MEITO_TOOLS_DIR/<tool>/<version>/<tool> 与同名 .sha256 旁证；logs/tools-installed.json 覆盖写入；logs/session-<id>.jsonl 追加 bootstrap.* 事件。
# 退出码：0 全部就绪或命中缓存；2 E_CONFIG（清单结构非法）；4 E_UNPINNED_TOOL/E_BOOTSTRAP（未钉死、下载或校验失败）；1 E_INTERNAL。
# 契约：contracts/tools-lock.schema.json（pinned 必填与 sha256 格式、refuseUnpinned、forbidCurlPipeShell）、contracts/error-codes.json、contracts/log.json。
# 未实测：未在 GitHub Actions ubuntu-latest 实跑；真实 pinned 下载未实测；本地 Git Bash 用预置缓存与夹具验证，见 docs/变更记录.md。

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
EXIT_BOOTSTRAP=$(meito_error_exit E_BOOTSTRAP)
EXIT_UNPINNED=$(meito_error_exit E_UNPINNED_TOOL)
EXIT_INTERNAL=$(meito_error_exit E_INTERNAL)

PY=$(meito_python)
LOCK_FILE="${MEITO_TOOLS_LOCK:-$MEITO_ROOT/tools.lock.json}"
TOOLS_DIR="${MEITO_TOOLS_DIR:-${HOME:?HOME 未设置}/.meito/tools}"

if [[ ! -f "$LOCK_FILE" ]]; then
  meito_fail failed_bootstrap bootstrap.lock_missing E_CONFIG "$EXIT_CONFIG" "tools.lock.json 不存在：$LOCK_FILE"
fi

# ===== 1. 整批判定：先全部校验，再开始任何下载 =====

LOCK_RC=0
LOCK_REPORT=$("$PY" - "$LOCK_FILE" "$MEITO_CONTRACTS_DIR/tools-lock.schema.json" "${MEITO_BOOTSTRAP_TOOLS:-}" <<'PY'
import json
import re
import sys

lock_path, schema_path, selection = sys.argv[1], sys.argv[2], sys.argv[3]
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
unpinned = []
allowed_versions = schema["properties"]["lockVersion"]["enum"]
if lock.get("lockVersion") not in allowed_versions:
    problems.append("lockVersion: 不在受支持版本 " + str(allowed_versions))
tools = lock.get("tools")
if not isinstance(tools, dict) or not tools:
    problems.append("tools: 必须是非空对象")
    tools = {}

names = [name for name in selection.split() if name]
if not names:
    names = sorted(tools.keys())

sha_pattern = re.compile(r"^[0-9a-f]{64}$")
name_pattern = re.compile(r"^[A-Za-z0-9._+-]+$")
version_pattern = re.compile(r"^[A-Za-z0-9._+-]+$")

for name in names:
    entry = tools.get(name)
    if not isinstance(entry, dict):
        problems.append(name + ": 清单中没有该工具")
        continue
    if entry.get("status") != "pinned":
        unpinned.append(name)
        continue
    version = entry.get("version")
    url = entry.get("url")
    sha = entry.get("sha256")
    if not isinstance(version, str) or not version_pattern.match(version) or version in (".", ".."):
        problems.append(name + ": version 非法或含路径字符")
    if not isinstance(url, str) or not url.startswith("https://") or " " in url:
        problems.append(name + ": url 必须是 https 且不含空白")
    if not isinstance(sha, str) or not sha_pattern.match(sha):
        problems.append(name + ": sha256 必须是 64 位小写十六进制")

if unpinned:
    for name in unpinned:
        sys.stderr.write("UNPINNED " + name + "\n")
    sys.exit(4)
if problems:
    for problem in problems:
        sys.stderr.write("CONFIG " + problem + "\n")
    sys.exit(3)
print(" ".join(names))
PY
) || LOCK_RC=$?

case "$LOCK_RC" in
  0) TOOL_NAMES="$LOCK_REPORT" ;;
  4) meito_fail failed_bootstrap bootstrap.refused E_UNPINNED_TOOL "$EXIT_UNPINNED" "tools.lock.json 存在 unpinned 条目；拒绝安装任何工具（fail-closed）" ;;
  3) meito_fail failed_bootstrap bootstrap.invalid_lock E_CONFIG "$EXIT_CONFIG" "tools.lock.json 不符合 contracts/tools-lock.schema.json" ;;
  *) meito_fail failed_bootstrap bootstrap.invalid_lock E_BOOTSTRAP "$EXIT_BOOTSTRAP" "读取 tools.lock.json 失败" ;;
esac

if [[ -z "$TOOL_NAMES" ]]; then
  meito_fail failed_bootstrap bootstrap.empty_selection E_USAGE "$EXIT_USAGE" "MEITO_BOOTSTRAP_TOOLS 未选中任何工具"
fi

# ===== 2. 逐个安装（幂等：先判定缓存，再下载） =====

read -r -a REQUIRED_TOOLS <<< "$TOOL_NAMES"
for tool in "${REQUIRED_TOOLS[@]}"; do
  VERSION=$(meito_json_get "$LOCK_FILE" "tools.$tool.version")
  URL=$(meito_json_get "$LOCK_FILE" "tools.$tool.url")
  SHA=$(meito_json_get "$LOCK_FILE" "tools.$tool.sha256")
  DEST_DIR="$TOOLS_DIR/$tool/$VERSION"
  DEST="$DEST_DIR/$tool"
  SIDECAR="$DEST_DIR/$tool.sha256"

  mkdir -p "$DEST_DIR"

  if [[ -f "$DEST" && -f "$SIDECAR" ]]; then
    EXPECTED=$(tr -d '\r\n' < "$SIDECAR")
    ACTUAL=$(meito_sha256 "$DEST")
    if [[ "$EXPECTED" == "$SHA" && "$ACTUAL" == "$SHA" ]]; then
      meito_event runner_starting bootstrap.tool_present info "" "tool=$tool" "version=$VERSION" >/dev/null
      meito_note "工具已就绪（跳过下载）：$tool $VERSION"
      continue
    fi
  fi

  if ! command -v curl >/dev/null 2>&1; then
    meito_fail failed_bootstrap bootstrap.curl_missing E_BOOTSTRAP "$EXIT_BOOTSTRAP" "缺少 curl，无法下载 pinned 工具" "tool=$tool"
  fi

  TMP="$DEST_DIR/.download-$tool.tmp"
  rm -f "$TMP"
  if ! curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 --retry 2 --output "$TMP" "$URL"; then
    rm -f "$TMP"
    meito_fail failed_bootstrap bootstrap.download_failed E_BOOTSTRAP "$EXIT_BOOTSTRAP" "工具下载失败；不做任何降级或跳过" "tool=$tool" "version=$VERSION"
  fi

  ACTUAL=$(meito_sha256 "$TMP")
  if [[ "$ACTUAL" != "$SHA" ]]; then
    rm -f "$TMP"
    meito_fail failed_bootstrap bootstrap.checksum_mismatch E_BOOTSTRAP "$EXIT_BOOTSTRAP" "SHA256 校验失败；删除下载文件且不执行" "tool=$tool" "version=$VERSION" "expected=$SHA" "actual=$ACTUAL"
  fi

  chmod 0755 "$TMP"
  mv -f "$TMP" "$DEST"
  printf '%s\n' "$SHA" > "$SIDEcar"
  meito_event runner_starting bootstrap.tool_installed info "" "tool=$tool" "version=$VERSION" "sha256=$SHA" >/dev/null
  meito_note "已校验并安装：$tool $VERSION"
done

# ===== 3. 安装台账（覆盖写，重复执行不追加） =====

MANIFEST="$MEITO_LOGS_DIR/tools-installed.json"
mkdir -p "$MEITO_LOGS_DIR"
"$PY" - "$MANIFEST" "$LOCK_FILE" "$TOOLS_DIR" $TOOL_NAMES <<'PY'
import json
import os
import sys

manifest_path, lock_path, tools_dir = sys.argv[1], sys.argv[2], sys.argv[3]
names = sys.argv[4:]
with open(lock_path, "r", encoding="utf-8") as handle:
    lock = json.load(handle)
tools = {}
for name in names:
    entry = lock["tools"][name]
    tools[name] = {
        "version": entry["version"],
        "sha256": entry["sha256"],
        "path": os.path.join(tools_dir, name, entry["version"], name),
    }
payload = {"lockVersion": lock["lockVersion"], "tools": tools}
tmp_path = manifest_path + ".tmp"
with open(tmp_path, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, ensure_ascii=False, indent=2, sort_keys=True)
    handle.write("\n")
os.replace(tmp_path, manifest_path)
PY

meito_event runner_starting bootstrap.completed info "" "tools_count=${#REQUIRED_TOOLS[@]}" >/dev/null
meito_note "工具引导完成：${#REQUIRED_TOOLS[@]} 个工具已校验"
exit 0
