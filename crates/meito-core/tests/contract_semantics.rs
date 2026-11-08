//! File: 契约语义测试——错误码退出码、状态机转移表与日志路径。
//!
//! 判据来自仓库根目录 `contracts/*.json`；测试直接读取契约文件，避免把断言写成对实现的
//! 第二份拷贝。任一项不符即失败，不放宽阈值。

use std::path::{Path, PathBuf};

use meito_core::{
    env_ledger_path, new_session_id, session_log_path, transition_for, ErrorCode, LogLevel,
    SessionEvent, SessionState,
};
use serde_json::Value;

// ===== 契约读取 =====

/// 返回仓库根目录。
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("解析仓库根目录")
}

/// 读取并解析一份契约 JSON。
fn contract(name: &str) -> Value {
    let path = repository_root().join("contracts").join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取契约 {} 失败：{error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("解析契约 {} 失败：{error}", path.display()))
}

// ===== 错误码 =====

#[test]
fn error_codes_match_contract() {
    let value = contract("error-codes.json");
    let codes = value
        .get("codes")
        .and_then(Value::as_array)
        .expect("契约包含 codes 数组");
    assert!(!codes.is_empty(), "契约 codes 不得为空");

    let exit_codes = value
        .get("exitCodes")
        .and_then(Value::as_array)
        .expect("契约包含 exitCodes 数组");
    assert!(!exit_codes.is_empty(), "契约 exitCodes 不得为空");

    for entry in codes {
        let name = entry
            .get("code")
            .and_then(Value::as_str)
            .expect("错误码含 code 字符串");
        let exit = entry
            .get("exitCode")
            .and_then(Value::as_u64)
            .expect("错误码含 exitCode") as u8;
        let message = entry
            .get("message")
            .and_then(Value::as_str)
            .expect("错误码含 message");
        let parsed = ErrorCode::parse(name).unwrap_or_else(|| panic!("内核缺少契约错误码 {name}"));
        assert_eq!(parsed.as_str(), name, "错误码字符串必须与契约一致");
        assert_eq!(parsed.exit_code(), exit, "{name} 的退出码必须与契约一致");
        assert_eq!(parsed.message(), message, "{name} 的默认说明必须与契约一致");
        assert!(
            exit_codes
                .iter()
                .any(|item| item.get("code").and_then(Value::as_u64) == Some(exit as u64)),
            "{name} 的退出码 {exit} 必须出现在契约 exitCodes 中"
        );
    }

    assert!(
        ErrorCode::parse("E_NOT_IN_CONTRACT").is_none(),
        "未知错误码不得被解析成功"
    );
}

#[test]
fn every_contract_error_code_round_trips() {
    for entry in contract("error-codes.json")
        .get("codes")
        .and_then(Value::as_array)
        .expect("契约包含 codes 数组")
    {
        let name = entry.get("code").and_then(Value::as_str).expect("code");
        let parsed = ErrorCode::parse(name).expect("已解析");
        assert_eq!(parsed.as_str(), name);
    }
}

// ===== 状态机 =====

#[test]
fn transitions_match_contract() {
    let value = contract("state-machine.json");
    let contract_transitions = value
        .get("transitions")
        .and_then(Value::as_array)
        .expect("契约包含 transitions");
    assert!(
        !contract_transitions.is_empty(),
        "契约 transitions 不得为空"
    );
    assert_eq!(
        meito_core::transitions().len(),
        contract_transitions.len(),
        "内核转移表长度必须与契约一致"
    );

    for entry in contract_transitions {
        let from_name = entry.get("from").and_then(Value::as_str).expect("from");
        let event_name = entry.get("event").and_then(Value::as_str).expect("event");
        let to_name = entry.get("to").and_then(Value::as_str).expect("to");
        let from = SessionState::parse(from_name).unwrap_or_else(|| panic!("未知状态 {from_name}"));
        let event =
            SessionEvent::parse(event_name).unwrap_or_else(|| panic!("未知事件 {event_name}"));
        let to = SessionState::parse(to_name).unwrap_or_else(|| panic!("未知状态 {to_name}"));
        let found = transition_for(from, event)
            .unwrap_or_else(|| panic!("内核缺少转移 {from_name} + {event_name}"));
        assert_eq!(found.to, to, "{from_name} + {event_name} 的目标状态不符");
    }
}

#[test]
fn state_terminal_flags_match_contract() {
    for entry in contract("state-machine.json")
        .get("states")
        .and_then(Value::as_array)
        .expect("契约包含 states")
    {
        let name = entry.get("name").and_then(Value::as_str).expect("name");
        let terminal = entry
            .get("terminal")
            .and_then(Value::as_bool)
            .expect("terminal");
        let state = SessionState::parse(name).unwrap_or_else(|| panic!("未知状态 {name}"));
        assert_eq!(state.is_terminal(), terminal, "{name} 的 terminal 标记不符");
    }
}

#[test]
fn event_names_match_contract() {
    let value = contract("state-machine.json");
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .expect("契约包含 events");
    assert!(!events.is_empty(), "契约 events 不得为空");
    for entry in events {
        let name = entry.as_str().expect("事件名是字符串");
        let event = SessionEvent::parse(name).unwrap_or_else(|| panic!("未知事件 {name}"));
        assert_eq!(event.as_str(), name);
    }
}

#[test]
fn timeouts_and_auto_cancel_match_contract() {
    let value = contract("state-machine.json");
    let timeouts = value.get("timeouts").expect("契约包含 timeouts");
    assert_eq!(
        timeouts
            .get("jobHardLimitMinutes")
            .and_then(Value::as_u64)
            .expect("jobHardLimitMinutes"),
        u64::from(meito_core::JOB_HARD_LIMIT_MINUTES)
    );
    assert_eq!(
        timeouts
            .get("sessionDefaultMinutes")
            .and_then(Value::as_u64)
            .expect("sessionDefaultMinutes"),
        u64::from(meito_core::SESSION_DEFAULT_MINUTES)
    );
    assert_eq!(
        timeouts
            .get("sessionMaxMinutes")
            .and_then(Value::as_u64)
            .expect("sessionMaxMinutes"),
        u64::from(meito_core::SESSION_MAX_MINUTES)
    );
    assert_eq!(
        timeouts
            .get("idleMinutes")
            .and_then(Value::as_u64)
            .expect("idleMinutes"),
        u64::from(meito_core::IDLE_MINUTES)
    );
    assert_eq!(
        timeouts
            .get("noJoinMinutes")
            .and_then(Value::as_u64)
            .expect("noJoinMinutes"),
        u64::from(meito_core::NO_JOIN_MINUTES)
    );
    assert_eq!(
        timeouts
            .get("checkpointBeforeMaxMinutes")
            .and_then(Value::as_u64)
            .expect("checkpointBeforeMaxMinutes"),
        u64::from(meito_core::CHECKPOINT_BEFORE_MAX_MINUTES)
    );
    assert_eq!(
        timeouts
            .get("reconnectMaxSsh")
            .and_then(Value::as_u64)
            .expect("reconnectMaxSsh"),
        u64::from(meito_core::RECONNECT_MAX_SSH)
    );
    assert_eq!(
        timeouts
            .get("reconnectMaxIce")
            .and_then(Value::as_u64)
            .expect("reconnectMaxIce"),
        u64::from(meito_core::RECONNECT_MAX_ICE)
    );

    let contract_rules = value
        .get("autoCancel")
        .and_then(Value::as_array)
        .expect("契约包含 autoCancel");
    assert!(!contract_rules.is_empty(), "契约 autoCancel 不得为空");
    let rules = meito_core::auto_cancel_rules();
    assert_eq!(rules.len(), contract_rules.len());
    for entry in contract_rules {
        let when_name = entry.get("when").and_then(Value::as_str).expect("when");
        let timeout = entry
            .get("timeout")
            .and_then(Value::as_str)
            .expect("timeout");
        let event_name = entry.get("event").and_then(Value::as_str).expect("event");
        let error_name = entry
            .get("errorCode")
            .and_then(Value::as_str)
            .expect("errorCode");
        let rule = rules
            .iter()
            .find(|candidate| candidate.when.as_str() == when_name && candidate.timeout == timeout)
            .unwrap_or_else(|| panic!("缺少自动取消规则 {when_name}/{timeout}"));
        assert_eq!(rule.event.as_str(), event_name);
        assert_eq!(rule.error_code.as_str(), error_name);
    }
}

// ===== 日志路径与级别 =====

#[test]
fn log_paths_match_contract() {
    let value = contract("log.json");
    let session_id = new_session_id();
    let expected = value
        .get("path")
        .and_then(Value::as_str)
        .expect("契约包含 path")
        .replace("<id>", &session_id);
    let actual = session_log_path(Path::new(""), &session_id)
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(actual, expected);

    let expected_ledger = value
        .get("envLedgerPath")
        .and_then(Value::as_str)
        .expect("契约包含 envLedgerPath")
        .replace("<id>", &session_id);
    let actual_ledger = env_ledger_path(Path::new(""), &session_id)
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(actual_ledger, expected_ledger);
}

#[test]
fn log_level_switch_matches_contract() {
    let value = contract("log.json");
    let level_switch = value.get("levelSwitch").expect("契约包含 levelSwitch");
    let allowed = level_switch
        .get("allowed")
        .and_then(Value::as_array)
        .expect("契约包含 allowed");
    for entry in allowed {
        let name = entry.as_str().expect("级别名");
        LogLevel::parse_switch(name)
            .unwrap_or_else(|error| panic!("契约允许的级别 {name} 被拒绝：{error}"));
    }
    assert!(
        LogLevel::parse_switch("warn").is_err(),
        "warn 不是对外级别开关"
    );
    assert_eq!(
        level_switch
            .get("default")
            .and_then(Value::as_str)
            .expect("default"),
        LogLevel::default().as_str()
    );
}
