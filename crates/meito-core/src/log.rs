//! File: 日志契约的路径、级别与事件模型。
//!
//! 唯一依据是 `contracts/log.json`：路径固定为 `logs/session-<id>.jsonl`，级别只有
//! info/debug/trace 三个对外开关，固定字段顺序为 ts/level/session_id/stage/event/error_code。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{ErrorCode, MeitoError};
use crate::state::Stage;
use crate::time::rfc3339_utc_now;
use crate::{ENV_LEDGER_PATH_PREFIX, SESSION_LOG_PATH_PREFIX};

// ===== 路径约定 =====

/// 返回契约规定的会话日志路径。
///
/// # 参数
/// - `root`: 工作目录；日志始终落在其下的 `logs/`。
/// - `session_id`: 会话 id，调用方应保证是 UUID v4。
///
/// # 错误
/// 不返回错误；路径拼接不访问文件系统。
pub fn session_log_path(root: &Path, session_id: &str) -> PathBuf {
    root.join(format!("{}{}.jsonl", SESSION_LOG_PATH_PREFIX, session_id))
}

/// 返回契约规定环境台账路径。
///
/// # 参数
/// - `root`: 工作目录。
/// - `session_id`: 会话 id。
///
/// # 错误
/// 不返回错误；路径拼接不访问文件系统。
pub fn env_ledger_path(root: &Path, session_id: &str) -> PathBuf {
    root.join(format!("{}{}.json", ENV_LEDGER_PATH_PREFIX, session_id))
}

/// 判断字符串是否为符合契约的 UUID v4。
///
/// # 参数
/// - `value`: 待校验的会话 id。
///
/// # 错误
/// 不返回错误；非法格式返回 `false`。
pub fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let is_separator = matches!(index, 8 | 13 | 18 | 23);
        if is_separator {
            if *byte != b'-' {
                return false;
            }
            continue;
        }
        if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    // 版本位固定为 4，变体位固定为 RFC 4122（8/9/a/b）。
    if bytes[14] != b'4' {
        return false;
    }
    matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
}

// ===== 日志级别 =====

/// 日志级别；对外开关只接受 info/debug/trace，warn/error 由内部事件使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogLevel {
    #[default]
    Info,
    Warn,
    Error,
    Debug,
    Trace,
}

impl LogLevel {
    /// 返回契约中的级别字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    /// 解析 `--log-level` 或 `MEITO_LOG` 的取值。
    ///
    /// # 参数
    /// - `value`: 只允许 `info`、`debug`、`trace`。
    ///
    /// # 错误
    /// 其他取值返回 `E_USAGE`；调用方必须映射到退出码 2。
    pub fn parse_switch(value: &str) -> Result<Self, MeitoError> {
        match value {
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            other => Err(MeitoError::new(
                ErrorCode::Usage,
                format!("--log-level 只接受 info/debug/trace，收到 {other}"),
            )),
        }
    }

    /// 内部严重度序号；越小越严重，用于级别过滤。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    fn severity(&self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
            Self::Info => 2,
            Self::Debug => 3,
            Self::Trace => 4,
        }
    }

    /// 判断当前事件级别是否被开关级别放行。
    ///
    /// # 参数
    /// - `threshold`: 用户选择的最高详细级别（info/debug/trace）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn is_enabled(&self, threshold: LogLevel) -> bool {
        self.severity() <= threshold.severity()
    }
}

// ===== 日志事件 =====

/// 一条 JSONL 日志事件；固定字段与 `contracts/log.json` 一致。
#[derive(Debug, Clone)]
pub struct LogEvent {
    pub ts: String,
    pub level: LogLevel,
    pub session_id: String,
    pub stage: Stage,
    pub event: String,
    pub error_code: Option<ErrorCode>,
    /// 额外字段；不得覆盖六个固定字段，写前必须脱敏。
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl LogEvent {
    /// 构造一条带当前 UTC 时间的日志事件。
    ///
    /// # 参数
    /// - `session_id`: 会话 id。
    /// - `stage`: 契约中的阶段枚举。
    /// - `event`: 事件名，必须匹配 `^[a-z0-9_.-]+$`。
    /// - `level`: 事件级别。
    ///
    /// # 错误
    /// 不返回错误；事件名合法性由 `FileLogSink` 写入前校验。
    pub fn new(
        session_id: impl Into<String>,
        stage: Stage,
        event: impl Into<String>,
        level: LogLevel,
    ) -> Self {
        Self {
            ts: rfc3339_utc_now(),
            level,
            session_id: session_id.into(),
            stage,
            event: event.into(),
            error_code: None,
            extra: BTreeMap::new(),
        }
    }

    /// 附加错误码；日志固定字段的 `error_code` 始终存在，无错误时写空串。
    ///
    /// # 参数
    /// - `code`: 冻结错误码。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn with_error_code(mut self, code: ErrorCode) -> Self {
        self.error_code = Some(code);
        self
    }

    /// 附加一个额外字段；敏感键在写入前统一替换为 `[REDACTED]`。
    ///
    /// # 参数
    /// - `key`: 额外字段名。
    /// - `value`: 字段值。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn with_extra(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}
