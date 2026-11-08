#!/usr/bin/env bash
# ===== Meito runner 远端脚本：actor 白名单门禁 =====
# 用途：在会话开始时按 fail-closed 规则判定触发者是否在白名单内；配置缺失、名单为空、actor 缺失一律拒绝。
# 输入：GITHUB_ACTOR 或 MEITO_ACTOR（实际 actor）；workflow 输入 flags.allowed_actors（JSON 覆盖，优先级高于仓库配置）；仓库根 meito.toml。
# 输出：logs/session-<id>.jsonl 追加 actor.checked 事件；不写任何秘密。
# 退出码：0 允许；2 参数或配置错误（E_USAGE/E_CONFIG）；3 拒绝（E_ACTOR_NOT_ALLOWED）；1 内部错误（E_INTERNAL）。
# 契约：contracts/meito-config.schema.json（allowed_actors 与 failClosed）、contracts/error-codes.json、contracts/log.json、contracts/workflow.json（flags）。
# 未实测：未在 GitHub Actions ubuntu-latest 实跑；本地 Git Bash 夹具验证见 docs/变更记录.md。

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
EXIT_ACTOR=$(meito_error_exit E_ACTOR_NOT_ALLOWED)
EXIT_INTERNAL=$(meito_error_exit E_INTERNAL)

FLAG_JSON=$(meito_input flags)
if [[ -n "$FLAG_JSON" ]]; then
  if ! OVERRIDE_LIST=$(meito_flag allowed_actors); then
    meito_fail preflight actor.invalid_flags E_USAGE "$EXIT_USAGE" "flags 不是合法 JSON 对象；拒绝判定"
  fi
else
  OVERRIDE_LIST=""
fi

# flags.allowed_actors 约定为 JSON 数组；同时接受逗号分隔的字符串以便手工复现。
OVERRIDE_ACTORS=()
if [[ -n "$OVERRIDE_LIST" ]]; then
  if [[ "$OVERRIDE_LIST" == \[* ]]; then
    if ! OVERRIDE_RAW=$(meito_list_from_text "$OVERRIDE_LIST"); then
      meito_fail preflight actor.invalid_flags E_USAGE "$EXIT_USAGE" "flags.allowed_actors 不是字符串数组"
    fi
    if [[ -n "$OVERRIDE_RAW" ]]; then
      while IFS= read -r entry; do
        if [[ -n "$entry" ]]; then
          OVERRIDE_ACTORS+=("$entry")
        fi
      done <<< "$OVERRIDE_RAW"
    fi
  else
    IFS=',' read -r -a OVERRIDE_ACTORS <<< "$OVERRIDE_LIST"
  fi
fi

CONFIG_FILE="${MEITO_CONFIG:-$MEITO_ROOT/meito.toml}"
CONFIG_MISSING=false
ALLOWED_ACTORS=()
SOURCE="config"

if [[ ${#OVERRIDE_ACTORS[@]} -gt 0 ]]; then
  ALLOWED_ACTORS=("${OVERRIDE_ACTORS[@]}")
  SOURCE="override"
elif [[ -f "$CONFIG_FILE" ]]; then
  if ! ALLOWED_RAW=$(meito_toml_get "$CONFIG_FILE" security allowed_actors); then
    meito_fail preflight actor.config_invalid E_CONFIG "$EXIT_CONFIG" "meito.toml 无法解析或缺少 [security].allowed_actors"
  fi
  if [[ -n "$ALLOWED_RAW" ]]; then
    while IFS= read -r entry; do
      if [[ -n "$entry" ]]; then
        ALLOWED_ACTORS+=("$entry")
      fi
    done <<< "$ALLOWED_RAW"
  fi
else
  CONFIG_MISSING=true
fi

ACTOR="${MEITO_ACTOR:-${GITHUB_ACTOR:-}}"
ACTOR_TRIMMED="$(meito_trim "$ACTOR")"

REASON=""
if [[ -z "$ACTOR_TRIMMED" ]]; then
  REASON="missing_actor"
elif [[ "$CONFIG_MISSING" == "true" && "$SOURCE" == "config" ]]; then
  REASON="config_missing"
elif [[ ${#ALLOWED_ACTORS[@]} -eq 0 ]]; then
  REASON="empty_allowlist"
else
  MATCHED=false
  for entry in "${ALLOWED_ACTORS[@]}"; do
    ENTRY_TRIMMED="$(meito_trim "$entry")"
    if [[ "${ENTRY_TRIMMED,,}" == "${ACTOR_TRIMMED,,}" ]]; then
      MATCHED=true
      break
    fi
  done
  if [[ "$MATCHED" != "true" ]]; then
    REASON="not_listed"
  fi
fi

if [[ -z "$REASON" ]]; then
  meito_event preflight actor.checked info "" "actor=$ACTOR_TRIMMED" "source=$SOURCE" "allowed=true" >/dev/null
  meito_note "actor '$ACTOR_TRIMMED' 允许（来源 $SOURCE）"
  exit 0
fi

meito_fail preflight actor.checked E_ACTOR_NOT_ALLOWED "$EXIT_ACTOR" "actor '$ACTOR_TRIMMED' 被拒绝：$REASON" "actor=$ACTOR_TRIMMED" "source=$SOURCE" "allowed=false" "reason=$REASON"
