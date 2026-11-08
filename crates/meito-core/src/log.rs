//! File: 日志契约的路径、级别与事件模型。
//!
//! 唯一依据是 `contracts/log.json`：路径固定为 `logs/session-<id>.jsonl`，级别只有
//! info/debug/trace 三个对外开关，固定字段顺序为 ts/level/session_id/stage/event/error_code。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

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

// ===== 脱敏 =====

/// 契约 `log.json` 声明的 10 个敏感键根名。
pub const REDACTION_KEYS: [&str; 10] = [
    "token",
    "authorization",
    "private_key",
    "secret",
    "password",
    "session_connect_string",
    "frps_token",
    "tailscale_authkey",
    "oauth_code",
    "turn_credential",
];

/// 敏感值统一替换成的字面量。
pub const REDACTED: &str = "[REDACTED]";

/// 返回契约声明的敏感键，供测试与审计核对。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 不返回错误。
pub fn redaction_keys() -> &'static [&'static str] {
    &REDACTION_KEYS
}

/// 判断字段名是否命中敏感键。
///
/// 匹配做小写与连字符归一化，并允许命名空间前缀（例如 `github_token`），
/// 宁可多遮蔽也不把凭据写进日志。
///
/// # 参数
/// - `key`: JSON 字段名。
///
/// # 错误
/// 不返回错误。
pub fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    REDACTION_KEYS.iter().any(|sensitive| {
        normalized == (*sensitive) || normalized.ends_with(&format!("_{}", sensitive))
    })
}

/// 递归脱敏一个 JSON 值。
///
/// 命中敏感键时整值替换，嵌套对象与数组逐层处理；写入日志前必须调用。
///
/// # 参数
/// - `value`: 待脱敏的 JSON 值。
///
/// # 错误
/// 不返回错误。
pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, child) in map {
                if is_sensitive_key(key) {
                    redacted.insert(key.clone(), Value::String(REDACTED.to_string()));
                } else {
                    redacted.insert(key.clone(), redact_json(child));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        other => other.clone(),
    }
}

// ===== JSONL 序列化 =====

/// 固定字段集合；额外字段不得覆盖这六个键。
const FIXED_FIELDS: [&str; 6] = ["ts", "level", "session_id", "stage", "event", "error_code"];

/// 判断事件名是否符合契约的 `^[a-z0-9_.-]+$`。
///
/// # 参数
/// - `name`: 事件名。
///
/// # 错误
/// 不返回错误。
pub fn is_valid_event_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-')
        })
}

/// 把日志事件序列化为一行 JSON，固定字段顺序不变。
///
/// 额外字段先脱敏再写入；六个固定字段始终存在，无错误码时 `error_code` 写空串。
///
/// # 参数
/// - `event`: 待写入的日志事件。
///
/// # 错误
/// 会话 id 不是 UUID v4、事件名非法或 JSON 序列化失败时返回错误。
pub fn to_jsonl(event: &LogEvent) -> Result<String, MeitoError> {
    if !is_uuid_v4(&event.session_id) {
        return Err(MeitoError::new(
            ErrorCode::Usage,
            format!("会话 id 必须是 UUID v4，收到 {}", event.session_id),
        ));
    }
    if !is_valid_event_name(&event.event) {
        return Err(MeitoError::new(
            ErrorCode::Usage,
            format!("日志事件名不符合 ^[a-z0-9_.-]+$：{}", event.event),
        ));
    }

    let mut line = String::from("{");
    line.push_str("\"ts\":");
    line.push_str(&json_text(&Value::String(event.ts.clone()))?);
    line.push_str(",\"level\":");
    line.push_str(&json_text(&Value::String(
        event.level.as_str().to_string(),
    ))?);
    line.push_str(",\"session_id\":");
    line.push_str(&json_text(&Value::String(event.session_id.clone()))?);
    line.push_str(",\"stage\":");
    line.push_str(&json_text(&Value::String(
        event.stage.as_str().to_string(),
    ))?);
    line.push_str(",\"event\":");
    line.push_str(&json_text(&Value::String(event.event.clone()))?);
    line.push_str(",\"error_code\":");
    // 契约允许 error_code 为空串，因此这里显式区分“无错误”而不是用默认值掩盖。
    let error_code = match event.error_code {
        Some(code) => code.as_str().to_string(),
        None => String::new(),
    };
    line.push_str(&json_text(&Value::String(error_code))?);

    for (key, value) in &event.extra {
        if FIXED_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let redacted = if is_sensitive_key(key) {
            Value::String(REDACTED.to_string())
        } else {
            redact_json(value)
        };
        line.push(',');
        line.push_str(&json_text(&Value::String(key.clone()))?);
        line.push(':');
        line.push_str(&json_text(&redacted)?);
    }
    line.push('}');
    Ok(line)
}

/// 把 JSON 值编码为字符串，失败时带上字段上下文。
///
/// # 参数
/// - `value`: 待编码的 JSON 值。
///
/// # 错误
/// 编码失败返回 `E_INTERNAL`，不得忽略。
fn json_text(value: &Value) -> Result<String, MeitoError> {
    serde_json::to_string(value).map_err(|error| {
        MeitoError::new(
            ErrorCode::Internal,
            format!("日志字段 JSON 编码失败：{}", error),
        )
    })
}

// ===== 文件写入 =====

/// 日志目录名。
pub const LOG_DIRECTORY: &str = "logs";

/// 日志写入接口；错误必须回传，禁止静默吞掉。
pub trait LogSink {
    /// 写入一条日志；级别未放行时成功返回且不落盘。
    ///
    /// # 参数
    /// - `event`: 待写入事件。
    ///
    /// # 错误
    /// 文件打开、写入或脱敏后序列化失败时返回错误。
    fn emit(&mut self, event: &LogEvent) -> Result<(), MeitoError>;
}

/// 追加写 JSONL 的会话日志。
#[derive(Debug)]
pub struct FileLogSink {
    path: PathBuf,
    threshold: LogLevel,
    session_id: String,
    file: File,
}

impl FileLogSink {
    /// 打开（或创建）`logs/session-<id>.jsonl`。
    ///
    /// # 参数
    /// - `root`: 工作目录。
    /// - `session_id`: 必须是 UUID v4，且事件写入时必须与它一致。
    /// - `threshold`: 用户选择的日志级别开关。
    ///
    /// # 错误
    /// 会话 id 非法返回 `E_USAGE`；目录或文件创建失败返回 `E_LOGS_UNAVAILABLE`。
    pub fn open(root: &Path, session_id: &str, threshold: LogLevel) -> Result<Self, MeitoError> {
        if !is_uuid_v4(session_id) {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                format!("会话 id 必须是 UUID v4，收到 {}", session_id),
            ));
        }
        let directory = root.join(LOG_DIRECTORY);
        fs::create_dir_all(&directory).map_err(|error| {
            MeitoError::io(
                ErrorCode::LogsUnavailable,
                format!("创建日志目录 {}", directory.display()),
                &error,
            )
        })?;
        let path = session_log_path(root, session_id);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                MeitoError::io(
                    ErrorCode::LogsUnavailable,
                    format!("打开日志文件 {}", path.display()),
                    &error,
                )
            })?;
        Ok(Self {
            path,
            threshold,
            session_id: session_id.to_string(),
            file,
        })
    }

    /// 返回日志文件路径。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回该 sink 绑定的会话 id。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 按级别过滤后写入一行 JSONL，并立即 flush 保证可观测。
    ///
    /// # 参数
    /// - `event`: 待写入事件；其 `session_id` 必须与 sink 绑定的一致。
    ///
    /// # 错误
    /// 会话 id 不一致或写盘失败时返回错误；级别未放行时返回 `Ok(())` 且不写盘。
    pub fn append(&mut self, event: &LogEvent) -> Result<(), MeitoError> {
        if event.session_id != self.session_id {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                format!(
                    "日志事件会话 id {} 与目标文件会话 id {} 不一致",
                    event.session_id, self.session_id
                ),
            ));
        }
        // 级别过滤发生在序列化之前，避免被过滤的事件产生任何落盘副作用。
        if !event.level.is_enabled(self.threshold) {
            return Ok(());
        }
        let line = to_jsonl(event)?;
        writeln!(self.file, "{}", line).map_err(|error| {
            MeitoError::io(
                ErrorCode::LogsUnavailable,
                format!("写入日志文件 {}", self.path.display()),
                &error,
            )
        })?;
        self.file.flush().map_err(|error| {
            MeitoError::io(
                ErrorCode::LogsUnavailable,
                format!("刷新日志文件 {}", self.path.display()),
                &error,
            )
        })
    }
}

impl LogSink for FileLogSink {
    /// 写入一条日志；语义与 `FileLogSink::append` 相同。
    ///
    /// # 参数
    /// - `event`: 待写入事件。
    ///
    /// # 错误
    /// 同 `FileLogSink::append`。
    fn emit(&mut self, event: &LogEvent) -> Result<(), MeitoError> {
        self.append(event)
    }
}
