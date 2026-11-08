//! File: meito-link 的访问日志结构。
//!
//! 服务端必须能证明自己“不知道内容”：访问日志只允许三类字段——通道 id、密文大小、
//! HTTP 状态码。不得写入 blob 片段、blob 哈希、token、公钥或任何请求头。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use meito_core::{ErrorCode, MeitoError};
use serde_json::{json, Value};

// ===== 访问日志 =====

/// 服务端访问日志文件名；由部署方落在自己的日志目录。
pub const ACCESS_LOG_FILENAME: &str = "link-access.jsonl";

/// 一条访问记录；字段集合就是白名单，新增字段必须同步安全评审。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLogRecord {
    /// 请求涉及的通道 id；未知通道也记录请求里的值，便于排查。
    pub channel_id: String,
    /// 本次请求携带的密文字节数；GET/DELETE 等无 blob 请求为 `None`。
    pub size: Option<u64>,
    /// HTTP 状态码。
    pub status: u16,
}

impl AccessLogRecord {
    /// 序列化为一行 JSON。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误；字段全是基本类型，序列化不会失败。
    pub fn to_json_line(&self) -> String {
        let value = json!({
            "channel_id": self.channel_id,
            "size": self.size,
            "status": self.status,
        });
        value.to_string()
    }
}

/// 追加一条访问记录到 JSONL 文件。
///
/// # 参数
/// - `path`: 访问日志的完整文件路径。
/// - `record`: 待写入记录。
///
/// # 错误
/// 父目录创建、文件打开或写入失败时返回 `E_LOGS_UNAVAILABLE`（退出码 12）。
pub fn append_access_log(path: &Path, record: &AccessLogRecord) -> Result<(), MeitoError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                MeitoError::io(ErrorCode::LogsUnavailable, "创建访问日志目录失败", &error)
            })?;
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| MeitoError::io(ErrorCode::LogsUnavailable, "打开访问日志失败", &error))?;
    writeln!(file, "{}", record.to_json_line())
        .map_err(|error| MeitoError::io(ErrorCode::LogsUnavailable, "写入访问日志失败", &error))
}

/// 把记录转换成 JSON 值，供测试与上层审计使用。
///
/// # 参数
/// - `record`: 访问记录。
///
/// # 错误
/// 不返回错误。
pub fn record_to_value(record: &AccessLogRecord) -> Value {
    json!({
        "channel_id": record.channel_id,
        "size": record.size,
        "status": record.status,
    })
}
