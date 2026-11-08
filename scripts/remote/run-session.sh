#!/usr/bin/env bash
# ===== Meito runner 远端脚本：启动会话 =====
# 用途：校验会话参数后启动 meito-runner 的 session 子命令；同一 session id 只允许一个活动实例（原子目录锁）。
# 输入：workflow 输入（GITHUB_EVENT_PATH.inputs 或 MEITO_INPUT_*）；contracts/workflow.json 的上限；contracts/entry.json 的图形预留状态；tools.lock.json 的 meito-runner pinned 条目；MEITO_RUNNER_BIN（可选显式路径）。
# 输出：logs/session-<id>.jsonl 追加 session.* 事件；日志与成果路径由 runner 按契约写入；不打印 token。
# 退出码：0 runner 成功；2 E_USAGE（重复启动/参数非法）；4 E_BOOTSTRAP（runner 未就绪）；7 E_ENTRY（图形入口未获许可）；其余按 contracts/error-codes.json 透传 runner 退出码；1 E_INTERNAL。
# 契约：contracts/workflow.json、contracts/entry.json、contracts/tools-lock.schema.json、contracts/error-codes.json、contracts/log.json。
# 未实测：meito-runner 的 session 子命令在本仓库尚未实现（M4 补齐）；未在 GitHub Actions ubuntu-latest 实跑；本地仅验证参数与锁分支，见 docs/变更记录.md。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

meito_require_python

SESSION=$(meito_session_id)
MEITO_SESSION_LOG=$(meito_session_or_unknown "$SESSION")
meito_require_session "$SESSION"

EXIT_USAGE=$(meito_error_exit E_USAGE)
EXIT_BOOTSTRAP=$(meito_error_exit E_BOOTSTRAP)
EXIT_ENTRY=$(meito_error_exit E_ENTRY)
EXIT_INTERNAL=$(meito_error_exit E_INTERNAL)

# ===== 1. 参数上限来自契约 =====

MAX_MINUTES=$(meito_input max_minutes "60")
IDLE_MINUTES=$(meito_input idle_minutes "10")
SESSION_MAX=$(meito_json_get "$MEITO_CONTRACTS_DIR/workflow.json" sessionMaxMinutes)

if [[ ! "$MAX_MINUTES" =~ ^[0-9]+$ ]] || [[ "$MAX_MINUTES" -lt 1 ]] || [[ "$MAX_MINUTES" -gt "$SESSION_MAX" ]]; then
  meito_fail preflight session.invalid_max E_USAGE "$EXIT_USAGE" "max_minutes 必须是 1..$SESSION_MAX 的整数"
fi
if [[ ! "$IDLE_MINUTES" =~ ^[0-9]+$ ]] || [[ "$IDLE_MINUTES" -lt 1 ]]; then
  meito_fail preflight session.invalid_idle E_USAGE "$EXIT_USAGE" "idle_minutes 必须是正整数"
fi

ENTRY=$(meito_input entry "terminal")
case "$ENTRY" in
  terminal|graphical|both) : ;;
  *) meito_fail preflight session.invalid_entry E_USAGE "$EXIT_USAGE" "entry 不在契约枚举内" ;;
esac

if [[ "$ENTRY" != "terminal" ]]; then
  ALLOW_GRAPHICAL=$(meito_flag allow_graphical "false")
  GRAPHICAL_STATUS=$(meito_json_get "$MEITO_CONTRACTS_DIR/entry.json" graphical.status)
  if [[ "$ALLOW_GRAPHICAL" != "true" || "$GRAPHICAL_STATUS" != "reserved" ]]; then
    meito_fail failed_entry entry.refused E_ENTRY "$EXIT_ENTRY" "图形入口在 v1 为 reserved，且未显式 allow_graphical=true" "entry=$ENTRY"
  fi
fi

P2P=$(meito_input p2p_provider "frp_xtcp")
case "$P2P" in
  frp_xtcp|tailscale_direct) : ;;
  *) meito_fail preflight session.invalid_p2p E_USAGE "$EXIT_USAGE" "p2p_provider 不在契约枚举内" ;;
esac

# ===== 2. runner 二进制：显式路径优先，否则只认 pinned 工具缓存 =====

RUNNER=""
if [[ -n "${MEITO_RUNNER_BIN:-}" ]]; then
  RUNNER="$MEITO_RUNNER_BIN"
else
  LOCK_FILE="${MEITO_TOOLS_LOCK:-$MEITO_ROOT/tools.lock.json}"
  RUNNER_VERSION=$(meito_json_get "$LOCK_FILE" tools.meito-runner.version 2>/dev/null) || RUNNER_VERSION=""
  RUNNER_SHA=$(meito_json_get "$LOCK_FILE" tools.meito-runner.sha256 2>/dev/null) || RUNNER_SHA=""
  RUNNER_STATUS=$(meito_json_get "$LOCK_FILE" tools.meito-runner.status 2>/dev/null) || RUNNER_STATUS=""
  if [[ "$RUNNER_STATUS" == "pinned" && -n "$RUNNER_VERSION" && -n "$RUNNER_SHA" ]]; then
    RUNNER="${MEITO_TOOLS_DIR:-${HOME:?HOME 未设置}/.meito/tools}/meito-runner/$RUNNER_VERSION/meito-runner"
    if [[ ! -x "$RUNNER" ]]; then
      meito_fail failed_bootstrap session.runner_missing E_BOOTSTRAP "$EXIT_BOOTSTRAP" "runner 未按 pinned 缓存安装；先执行 bootstrap.sh"
    fi
    ACTUAL=$(meito_sha256 "$RUNNER")
    if [[ "$ACTUAL" != "$RUNNER_SHA" ]]; then
      meito_fail failed_bootstrap session.runner_checksum E_BOOTSTRAP "$EXIT_BOOTSTRAP" "runner 二进制 SHA256 与 tools.lock 不符；拒绝启动"
    fi
  else
    meito_fail failed_bootstrap session.runner_unpinned E_BOOTSTRAP "$EXIT_BOOTSTRAP" "meito-runner 未 pinned 或未提供 MEITO_RUNNER_BIN"
  fi
fi

if [[ ! -x "$RUNNER" ]]; then
  meito_fail failed_bootstrap session.runner_missing E_BOOTSTRAP "$EXIT_BOOTSTRAP" "runner 不可执行：$RUNNER"
fi

# ===== 3. 单实例锁（先判定再动作） =====

mkdir -p "$MEITO_LOGS_DIR"
LOCK_DIR="$MEITO_LOGS_DIR/.meito-session-$SESSION.lock"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  meito_fail preflight session.duplicate_start E_USAGE "$EXIT_USAGE" "同一 session id 已有活动实例；拒绝重复启动" "session=$SESSION"
fi

cleanup_lock() {
  if [[ -d "$LOCK_DIR" ]]; then
    if ! rmdir "$LOCK_DIR"; then
      printf '[meito] 清理会话锁失败：%s\n' "$LOCK_DIR" >&2
    fi
  fi
}
trap cleanup_lock EXIT

# ===== 4. 启动 runner 并透传契约退出码 =====

CONFIG_FILE="${MEITO_CONFIG:-$MEITO_ROOT/meito.toml}"
export MEITO_SESSION_ID="$SESSION"
export MEITO_ENTRY="$ENTRY"
export MEITO_P2P_PROVIDER="$P2P"
export MEITO_MAX_MINUTES="$MAX_MINUTES"
export MEITO_IDLE_MINUTES="$IDLE_MINUTES"
export MEITO_CONFIG="$CONFIG_FILE"
export MEITO_CHANNEL="${MEITO_CHANNEL:-$(meito_input channel)}"
export MEITO_CHANNEL_WRITE_TOKEN="${MEITO_CHANNEL_WRITE_TOKEN:-$(meito_input channel_write_token)}"
export MEITO_CHANNEL_PUBLIC_KEY="${MEITO_CHANNEL_PUBLIC_KEY:-$(meito_input channel_public_key)}"

meito_event runner_starting session.starting info "" "entry=$ENTRY" "p2p_provider=$P2P" "max_minutes=$MAX_MINUTES" "idle_minutes=$IDLE_MINUTES" >/dev/null

RUN_RC=0
if "$RUNNER" session --session "$SESSION" --config "$CONFIG_FILE"; then
  RUN_RC=0
else
  RUN_RC=$?
fi

if [[ "$RUN_RC" -eq 0 ]]; then
  meito_event ending session.finished info "" "exit_code=0" >/dev/null
  meito_note "会话正常结束"
else
  meito_event ending session.finished error "" "exit_code=$RUN_RC" >/dev/null
  meito_note "会话失败；runner 退出码 $RUN_RC"
fi
exit "$RUN_RC"
