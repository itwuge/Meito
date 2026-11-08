//! File: 冻结错误码与退出码映射。
//!
//! 唯一依据是 `contracts/error-codes.json`：每个错误码都必须映射到该文件声明的退出码，
//! 不允许调用方自行编造新的错误码或退出码。

use std::fmt;

// ===== 错误码定义 =====

/// 冻结错误码；`as_str` 输出与契约完全一致的字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Internal,
    Usage,
    Auth,
    ActorNotAllowed,
    Preflight,
    Bootstrap,
    Dispatch,
    QueueTimeout,
    Link,
    UdpBlocked,
    P2pXtcp,
    P2pTs,
    HostKey,
    Entry,
    Artifact,
    ApplyConflict,
    JobTimeout,
    NoJoinTimeout,
    Cancel,
    WebNoDirectPath,
    WebRelayUsed,
    LogsUnavailable,
    Orphan,
}

impl ErrorCode {
    /// 返回与契约一致的稳定错误码字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Internal => "E_INTERNAL",
            Self::Usage => "E_USAGE",
            Self::Auth => "E_AUTH",
            Self::ActorNotAllowed => "E_ACTOR_NOT_ALLOWED",
            Self::Preflight => "E_PREFLIGHT",
            Self::Bootstrap => "E_BOOTSTRAP",
            Self::Dispatch => "E_DISPATCH",
            Self::QueueTimeout => "E_QUEUE_TIMEOUT",
            Self::Link => "E_LINK",
            Self::UdpBlocked => "E_UDP_BLOCKED",
            Self::P2pXtcp => "E_P2P_XTCP",
            Self::P2pTs => "E_P2P_TS",
            Self::HostKey => "E_HOSTKEY",
            Self::Entry => "E_ENTRY",
            Self::Artifact => "E_ARTIFACT",
            Self::ApplyConflict => "E_APPLY_CONFLICT",
            Self::JobTimeout => "E_JOB_TIMEOUT",
            Self::NoJoinTimeout => "E_NO_JOIN_TIMEOUT",
            Self::Cancel => "E_CANCEL",
            Self::WebNoDirectPath => "E_WEB_NO_DIRECT_PATH",
            Self::WebRelayUsed => "E_WEB_RELAY_USED",
            Self::LogsUnavailable => "E_LOGS_UNAVAILABLE",
            Self::Orphan => "E_ORPHAN",
        }
    }

    /// 返回契约中声明的进程退出码。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Internal => 1,
            Self::Usage => 2,
            Self::Auth | Self::ActorNotAllowed => 3,
            Self::Preflight | Self::Bootstrap => 4,
            Self::Dispatch | Self::QueueTimeout => 5,
            Self::Link | Self::UdpBlocked | Self::P2pXtcp | Self::P2pTs | Self::HostKey => 6,
            Self::Entry => 7,
            Self::Artifact | Self::ApplyConflict => 8,
            Self::JobTimeout | Self::NoJoinTimeout => 9,
            Self::Cancel => 10,
            Self::WebNoDirectPath | Self::WebRelayUsed => 11,
            Self::LogsUnavailable | Self::Orphan => 12,
        }
    }

    /// 返回契约中声明的英文默认说明。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn message(&self) -> &'static str {
        match self {
            Self::Internal => "internal error",
            Self::Usage => "invalid arguments or config",
            Self::Auth => "authentication or GitHub permission failure",
            Self::ActorNotAllowed => "actor is not in the allowlist",
            Self::Preflight => "local or runner preflight failed",
            Self::Bootstrap => "runner bootstrap failed",
            Self::Dispatch => "workflow dispatch failed",
            Self::QueueTimeout => "queued job did not start in time",
            Self::Link => "coordination or encrypted link delivery failed",
            Self::UdpBlocked => "runner outbound UDP is blocked",
            Self::P2pXtcp => "frp xtcp hole punching failed",
            Self::P2pTs => "Tailscale direct path failed or DERP was detected",
            Self::HostKey => "host key pin mismatch",
            Self::Entry => "ttyd/noVNC entry failed",
            Self::Artifact => "artifact missing or checksum mismatch",
            Self::ApplyConflict => "local apply produced conflicts",
            Self::JobTimeout => "six-hour job limit or watchdog timeout",
            Self::NoJoinTimeout => "no user joined before the join timeout",
            Self::Cancel => "session cancelled",
            Self::WebNoDirectPath => "browser has no direct ICE path; TURN is disabled",
            Self::WebRelayUsed => "relay candidate detected; session refused",
            Self::LogsUnavailable => "logs could not be retrieved",
            Self::Orphan => "orphan session detected",
        }
    }

    /// 从契约字符串解析错误码，供状态文件与日志回读使用。
    ///
    /// # 参数
    /// - `value`: 形如 `E_USAGE` 的稳定错误码字符串。
    ///
    /// # 错误
    /// 未知字符串返回 `None`；调用方必须显式处理，不得静默丢弃。
    pub fn parse(value: &str) -> Option<Self> {
        let code = match value {
            "E_INTERNAL" => Self::Internal,
            "E_USAGE" => Self::Usage,
            "E_AUTH" => Self::Auth,
            "E_ACTOR_NOT_ALLOWED" => Self::ActorNotAllowed,
            "E_PREFLIGHT" => Self::Preflight,
            "E_BOOTSTRAP" => Self::Bootstrap,
            "E_DISPATCH" => Self::Dispatch,
            "E_QUEUE_TIMEOUT" => Self::QueueTimeout,
            "E_LINK" => Self::Link,
            "E_UDP_BLOCKED" => Self::UdpBlocked,
            "E_P2P_XTCP" => Self::P2pXtcp,
            "E_P2P_TS" => Self::P2pTs,
            "E_HOSTKEY" => Self::HostKey,
            "E_ENTRY" => Self::Entry,
            "E_ARTIFACT" => Self::Artifact,
            "E_APPLY_CONFLICT" => Self::ApplyConflict,
            "E_JOB_TIMEOUT" => Self::JobTimeout,
            "E_NO_JOIN_TIMEOUT" => Self::NoJoinTimeout,
            "E_CANCEL" => Self::Cancel,
            "E_WEB_NO_DIRECT_PATH" => Self::WebNoDirectPath,
            "E_WEB_RELAY_USED" => Self::WebRelayUsed,
            "E_LOGS_UNAVAILABLE" => Self::LogsUnavailable,
            "E_ORPHAN" => Self::Orphan,
            _ => return None,
        };
        Some(code)
    }
}

// ===== 统一错误类型 =====

/// 携带冻结错误码与人类可读上下文的统一错误。
///
/// 外壳只通过 `code().exit_code()` 决定进程退出码，禁止自行映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeitoError {
    code: ErrorCode,
    message: String,
}

impl MeitoError {
    /// 构造一个错误；`message` 只写上下文，不重复错误码默认文案。
    ///
    /// # 参数
    /// - `code`: 冻结错误码。
    /// - `message`: 人类可读上下文，必须包含可定位的对象或路径。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// 返回冻结错误码。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// 返回人类可读上下文。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn message(&self) -> &str {
        &self.message
    }

    /// 返回契约声明的进程退出码。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn exit_code(&self) -> u8 {
        self.code.exit_code()
    }

    /// 把 IO 错误包装为带操作上下文的 `MeitoError`。
    ///
    /// # 参数
    /// - `code`: 该操作对应的冻结错误码。
    /// - `context`: 操作说明与目标路径。
    /// - `source`: 原始 IO 错误，保留其系统信息。
    ///
    /// # 错误
    /// 不返回错误；返回值本身即错误描述。
    pub fn io(code: ErrorCode, context: impl AsRef<str>, source: &std::io::Error) -> Self {
        Self::new(code, format!("{}: {}", context.as_ref(), source))
    }

    /// 输出机读错误对象，字段顺序固定为 code/exit_code/message。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "code".to_string(),
            serde_json::Value::String(self.code.as_str().to_string()),
        );
        map.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.code.exit_code() as u64)),
        );
        map.insert(
            "message".to_string(),
            serde_json::Value::String(self.message.clone()),
        );
        serde_json::Value::Object(map)
    }
}

impl fmt::Display for MeitoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for MeitoError {}
