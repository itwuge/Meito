//! File: `logs` 子命令的内核实现。
//!
//! 只读取契约规定的 `logs/session-<id>.jsonl`；阶段过滤必须解析 JSON 再比较，
//! 解析失败立即报错并给出行号，不允许把损坏日志当成“空结果”静默跳过。

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{ErrorCode, MeitoError};
use crate::log::{is_uuid_v4, session_log_path};
use crate::state::SessionState;

// ===== 结果模型 =====

/// 一条原始日志行，保留行号用于错误定位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub line_number: usize,
    pub raw: String,
}

// ===== 路径解析 =====

/// 解析要读取的日志文件。
///
/// # 参数
/// - `root`: 工作目录。
/// - `session`: 显式 UUID 或 `latest`/`None`。
///
/// # 错误
/// 会话 id 非法返回 `E_USAGE`；日志目录或文件不存在返回 `E_LOGS_UNAVAILABLE`。
pub fn resolve_log_path(root: &Path, session: Option<&str>) -> Result<PathBuf, MeitoError> {
    match session {
        Some(value) if value != "latest" => {
            if !is_uuid_v4(value) {
                return Err(MeitoError::new(
                    ErrorCode::Usage,
                    format!("--session 必须是 UUID v4 或 latest，收到 {value}"),
                ));
            }
            let path = session_log_path(root, value);
            if !path.is_file() {
                return Err(MeitoError::new(
                    ErrorCode::LogsUnavailable,
                    format!("日志文件不存在：{}", path.display()),
                ));
            }
            Ok(path)
        }
        _ => latest_log_file(root),
    }
}

/// 找到最近修改的 `logs/session-*.jsonl`。
///
/// # 参数
/// - `root`: 工作目录。
///
/// # 错误
/// 日志目录不存在或没有任何会话日志时返回 `E_LOGS_UNAVAILABLE`；
/// 目录或元数据读取失败返回 `E_LOGS_UNAVAILABLE`。
fn latest_log_file(root: &Path) -> Result<PathBuf, MeitoError> {
    let directory = root.join("logs");
    if !directory.is_dir() {
        return Err(MeitoError::new(
            ErrorCode::LogsUnavailable,
            format!("日志目录不存在：{}", directory.display()),
        ));
    }
    let entries = fs::read_dir(&directory).map_err(|error| {
        MeitoError::io(
            ErrorCode::LogsUnavailable,
            format!("读取日志目录 {}", directory.display()),
            &error,
        )
    })?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(|error| {
            MeitoError::io(
                ErrorCode::LogsUnavailable,
                format!("遍历日志目录 {}", directory.display()),
                &error,
            )
        })?;
        let path = entry.path();
        let file_name = match path.file_name().and_then(|value| value.to_str()) {
            Some(value) => value,
            None => continue,
        };
        if !file_name.starts_with("session-") || !file_name.ends_with(".jsonl") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|error| {
                MeitoError::io(
                    ErrorCode::LogsUnavailable,
                    format!("读取日志文件元数据 {}", path.display()),
                    &error,
                )
            })?;
        let replace = match &newest {
            Some((current, _)) => modified > *current,
            None => true,
        };
        if replace {
            newest = Some((modified, path));
        }
    }
    match newest {
        Some((_, path)) => Ok(path),
        None => Err(MeitoError::new(
            ErrorCode::LogsUnavailable,
            format!("{} 下没有 session-*.jsonl", directory.display()),
        )),
    }
}

// ===== 读取与过滤 =====

/// 读取全部日志行，可按阶段过滤。
///
/// # 参数
/// - `root`: 工作目录。
/// - `session`: 显式 UUID 或 `latest`/`None`。
/// - `stage`: 可选阶段过滤。
///
/// # 错误
/// 路径解析失败返回对应错误；文件不是 UTF-8 或过滤时行不是合法 JSON 返回
/// `E_LOGS_UNAVAILABLE`。
pub fn read_log_lines(
    root: &Path,
    session: Option<&str>,
    stage: Option<SessionState>,
) -> Result<Vec<LogRecord>, MeitoError> {
    let path = resolve_log_path(root, session)?;
    let text = fs::read_to_string(&path).map_err(|error| {
        MeitoError::io(
            ErrorCode::LogsUnavailable,
            format!("读取日志文件 {}", path.display()),
            &error,
        )
    })?;
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(expected) = stage {
            if stage_matches(line, expected, line_number)? {
                records.push(LogRecord {
                    line_number,
                    raw: line.to_string(),
                });
            }
        } else {
            records.push(LogRecord {
                line_number,
                raw: line.to_string(),
            });
        }
    }
    Ok(records)
}

/// 判断某行日志是否属于指定阶段。
///
/// # 参数
/// - `line`: 单行 JSONL。
/// - `expected`: 期望阶段。
/// - `line_number`: 行号，仅用于错误信息。
///
/// # 错误
/// JSON 解析失败、缺少 `stage` 或阶段名未知时返回 `E_LOGS_UNAVAILABLE`。
fn stage_matches(
    line: &str,
    expected: SessionState,
    line_number: usize,
) -> Result<bool, MeitoError> {
    let value: Value = serde_json::from_str(line).map_err(|error| {
        MeitoError::new(
            ErrorCode::LogsUnavailable,
            format!("日志第 {line_number} 行不是合法 JSON：{error}"),
        )
    })?;
    let stage_value = value.get("stage").and_then(Value::as_str).ok_or_else(|| {
        MeitoError::new(
            ErrorCode::LogsUnavailable,
            format!("日志第 {line_number} 行缺少 stage 字段"),
        )
    })?;
    let stage = SessionState::parse(stage_value).ok_or_else(|| {
        MeitoError::new(
            ErrorCode::LogsUnavailable,
            format!("日志第 {line_number} 行阶段名未知：{stage_value}"),
        )
    })?;
    Ok(stage == expected)
}

// ===== 跟随读取 =====

/// 增量读取会话日志，供 `logs --follow` 使用。
#[derive(Debug)]
pub struct SessionLogTail {
    path: PathBuf,
    offset: u64,
    remainder: String,
    stage: Option<SessionState>,
    lines_seen: usize,
}

impl SessionLogTail {
    /// 打开一个跟随读取器。
    ///
    /// # 参数
    /// - `root`: 工作目录。
    /// - `session`: 显式 UUID 或 `latest`/`None`。
    /// - `stage`: 可选阶段过滤。
    ///
    /// # 错误
    /// 同 `resolve_log_path`。
    pub fn open(
        root: &Path,
        session: Option<&str>,
        stage: Option<SessionState>,
    ) -> Result<Self, MeitoError> {
        Ok(Self {
            path: resolve_log_path(root, session)?,
            offset: 0,
            remainder: String::new(),
            stage,
            lines_seen: 0,
        })
    }

    /// 读取自上次调用以来新增的完整行。
    ///
    /// # 参数
    /// - 无（仅 `&mut self`）。
    ///
    /// # 错误
    /// 文件或元数据读取失败、内容不是 UTF-8、过滤行损坏时返回错误。
    pub fn poll(&mut self) -> Result<Vec<LogRecord>, MeitoError> {
        let metadata = fs::metadata(&self.path).map_err(|error| {
            MeitoError::io(
                ErrorCode::LogsUnavailable,
                format!("读取日志文件元数据 {}", self.path.display()),
                &error,
            )
        })?;
        let length = metadata.len();
        // 文件被截断或轮转时从头再读，避免偏移量失效造成永久空转。
        if length < self.offset {
            self.offset = 0;
            self.remainder.clear();
            self.lines_seen = 0;
        }
        if length == self.offset {
            return Ok(Vec::new());
        }

        let mut file = File::open(&self.path).map_err(|error| {
            MeitoError::io(
                ErrorCode::LogsUnavailable,
                format!("打开日志文件 {}", self.path.display()),
                &error,
            )
        })?;
        file.seek(SeekFrom::Start(self.offset)).map_err(|error| {
            MeitoError::io(
                ErrorCode::LogsUnavailable,
                format!("定位日志文件 {}", self.path.display()),
                &error,
            )
        })?;
        let mut chunk = Vec::new();
        file.take(length - self.offset)
            .read_to_end(&mut chunk)
            .map_err(|error| {
                MeitoError::io(
                    ErrorCode::LogsUnavailable,
                    format!("读取日志增量 {}", self.path.display()),
                    &error,
                )
            })?;
        self.offset = length;
        let text = String::from_utf8(chunk).map_err(|error| {
            MeitoError::new(
                ErrorCode::LogsUnavailable,
                format!("日志文件 {} 不是 UTF-8：{}", self.path.display(), error),
            )
        })?;
        let combined = format!("{}{}", self.remainder, text);
        let (complete, remainder) = match combined.rfind('\n') {
            Some(position) => (&combined[..=position], &combined[position + 1..]),
            None => ("", combined.as_str()),
        };
        self.remainder = remainder.to_string();

        let mut records = Vec::new();
        for line in complete.lines() {
            self.lines_seen += 1;
            if line.trim().is_empty() {
                continue;
            }
            match self.stage {
                Some(expected) => {
                    if stage_matches(line, expected, self.lines_seen)? {
                        records.push(LogRecord {
                            line_number: self.lines_seen,
                            raw: line.to_string(),
                        });
                    }
                }
                None => records.push(LogRecord {
                    line_number: self.lines_seen,
                    raw: line.to_string(),
                }),
            }
        }
        Ok(records)
    }
}
