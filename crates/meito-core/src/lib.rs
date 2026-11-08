//! File: meito-core 公共契约与内核实现（M2/M3）。
//!
//! 本 crate 是两个外壳共用的唯一 Model 实现；本地 CLI 与网页外壳只能依赖这里的公共接口，
//! 不得各自复制状态机、P2P 与 artifact 策略。冻结来源见仓库根目录 `contracts/`。

// ===== 契约常量 =====

/// 当前冻结的契约版本，必须与 `contracts/contract-version.json` 一致。
pub const CONTRACT_VERSION: &str = "0.3.4";

/// 会话日志路径前缀，完整路径为 `logs/session-<id>.jsonl`。
pub const SESSION_LOG_PATH_PREFIX: &str = "logs/session-";

/// 连接包 magic，长度必须与 `contracts/connection-package.json` 一致。
pub const CONNECTION_PACKAGE_MAGIC: &str = "MEITO1";

/// 连接包格式版本。
pub const CONNECTION_PACKAGE_FORMAT_VERSION: u8 = 1;

/// 唯一允许的加密算法标识。
pub const CRYPTO_ALGORITHM_ID: &str = "x25519-xchacha20poly1305";

/// 连接包头部写入的算法 id 字节。
pub const CRYPTO_ALGORITHM_ID_BYTE: u8 = 0;

/// 环境台账路径前缀，完整路径为 `logs/env-session-<id>.json`。
pub const ENV_LEDGER_PATH_PREFIX: &str = "logs/env-session-";

mod controller;
mod doctor;
mod entry;
mod env;
mod error;
mod log;
mod logs;
mod state;
mod state_store;
mod status;
mod time;
mod tools_lock;
mod workflow;

pub use controller::{ArtifactStore, Command, Controller, EntryAdapter, P2pProvider, SessionView};
pub use doctor::{run_doctor, CheckStatus, DoctorCheck, DoctorReport, CONTRACT_FILES};
pub use entry::{load_entry_ports, EntryPorts, ENTRY_CONTRACT_PATH};
pub use env::{load_ledger, EnvironmentLedger};
pub use error::{ErrorCode, MeitoError};
pub use log::{
    env_ledger_path, is_sensitive_key, is_uuid_v4, is_valid_event_name, redact_json,
    redaction_keys, session_log_path, to_jsonl, FileLogSink, LogEvent, LogLevel, LogSink,
    LOG_DIRECTORY, REDACTED, REDACTION_KEYS,
};
pub use logs::{read_log_lines, resolve_log_path, LogRecord, SessionLogTail};
pub use state::{
    auto_cancel_rules, transition_for, transitions, AutoCancelRule, SessionEvent, SessionMachine,
    SessionState, Stage, Transition, CHECKPOINT_BEFORE_MAX_MINUTES, IDLE_MINUTES,
    JOB_HARD_LIMIT_MINUTES, NO_JOIN_MINUTES, RECONNECT_MAX_ICE, RECONNECT_MAX_SSH,
    SESSION_DEFAULT_MINUTES, SESSION_MAX_MINUTES,
};
pub use state_store::{SessionSnapshot, StateStore};
pub use status::{read_status, StatusReport};
pub use tools_lock::{load_tools_lock, PinStatus, ToolPin, ToolsLock, TOOLS_LOCK_PATH};
pub use workflow::{init_workflow, InitReport, WORKFLOW_DESTINATION_PATH, WORKFLOW_TEMPLATE_PATH};

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
