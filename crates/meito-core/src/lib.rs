//! File: meito-core 公共契约与内核实现（M2）。
//!
//! 本 crate 是两个外壳共用的唯一 Model 实现；本地 CLI 与网页外壳只能依赖这里的公共接口，
//! 不得各自复制状态机、P2P 与 artifact 策略。冻结来源见仓库根目录 `contracts/`。

// ===== 契约常量 =====

/// 当前冻结的契约版本，必须与 `contracts/contract-version.json` 一致。
pub const CONTRACT_VERSION: &str = "0.1.0";

/// 会话日志路径前缀，完整路径为 `logs/session-<id>.jsonl`。
pub const SESSION_LOG_PATH_PREFIX: &str = "logs/session-";

/// 环境台账路径前缀，完整路径为 `logs/env-session-<id>.json`。
pub const ENV_LEDGER_PATH_PREFIX: &str = "logs/env-session-";

mod controller;
mod error;
mod log;
mod state;
mod time;

pub use controller::{ArtifactStore, Command, Controller, EntryAdapter, P2pProvider, SessionView};
pub use error::{ErrorCode, MeitoError};
pub use log::{
    env_ledger_path, is_sensitive_key, is_uuid_v4, is_valid_event_name, redact_json,
    redaction_keys, session_log_path, to_jsonl, FileLogSink, LogEvent, LogLevel, LogSink,
    LOG_DIRECTORY, REDACTED, REDACTION_KEYS,
};
pub use state::{
    auto_cancel_rules, transitions, AutoCancelRule, SessionEvent, SessionState, Stage, Transition,
    CHECKPOINT_BEFORE_MAX_MINUTES, IDLE_MINUTES, JOB_HARD_LIMIT_MINUTES, NO_JOIN_MINUTES,
    RECONNECT_MAX_ICE, RECONNECT_MAX_SSH, SESSION_DEFAULT_MINUTES, SESSION_MAX_MINUTES,
};

/// 生成一个符合契约的 UUID v4 会话 id。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 不返回错误；UUID 生成失败由 `uuid` crate 内部保证不会发生。
pub fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
