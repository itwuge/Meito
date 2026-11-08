//! File: 日志写入运行用例——六字段顺序、写前脱敏与级别过滤。
//!
//! 这些用例直接落盘再读回，验证的是“实际写入行为”，不是序列化函数的孤立断言。

use std::path::{Path, PathBuf};

use meito_core::{
    new_session_id, session_log_path, to_jsonl, ErrorCode, FileLogSink, LogEvent, LogLevel,
    SessionState, REDACTION_KEYS,
};
use serde_json::json;

// ===== 临时目录 =====

/// 测试用临时目录，析构时清理。
struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    /// 创建唯一临时目录。
    ///
    /// # 参数
    /// - `label`: 目录名前缀，便于定位失败用例。
    ///
    /// # 错误
    /// 目录创建失败时 panic。
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("meito-{}-{}", label, new_session_id()));
        std::fs::create_dir_all(&path).expect("创建测试目录");
        Self { path }
    }

    /// 返回目录路径。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!("清理临时目录 {} 失败：{error}", self.path.display());
        }
    }
}

/// 统计日志文件行数。
///
/// # 参数
/// - `path`: 日志文件。
///
/// # 错误
/// 读取失败时 panic。
fn line_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .expect("读取日志文件")
        .lines()
        .count()
}

// ===== 用例 =====

#[test]
fn redaction_is_applied_before_write() {
    let directory = TempDirectory::new("redact");
    let session_id = new_session_id();
    let mut sink =
        FileLogSink::open(directory.path(), &session_id, LogLevel::Info).expect("打开日志 sink");

    let secrets = [
        ("token", "secret-token-value"),
        ("authorization", "Bearer secret-authorization"),
        ("private_key", "PRIVATE-KEY-CONTENT"),
        ("secret", "literal-secret"),
        ("password", "p@ssw0rd-value"),
        ("session_connect_string", "ssh user@host"),
        ("frps_token", "frps-secret"),
        ("tailscale_authkey", "ts-authkey"),
        ("oauth_code", "oauth-secret"),
        ("turn_credential", "turn-secret"),
    ];
    let mut event = LogEvent::new(
        &session_id,
        SessionState::Preflight,
        "preflight.env",
        LogLevel::Info,
    )
    .with_error_code(ErrorCode::Preflight);
    for (key, value) in secrets {
        event = event.with_extra(key, json!(value));
    }
    event = event.with_extra(
        "payload",
        json!({"authorization": "Bearer nested-secret", "port": 22}),
    );
    event = event.with_extra("note", json!("keep-me"));
    sink.append(&event).expect("写入日志");

    let path = session_log_path(directory.path(), &session_id);
    let text = std::fs::read_to_string(&path).expect("读取日志文件");
    let line = text.lines().next().expect("至少一行日志");

    for (_, value) in secrets {
        assert!(!line.contains(value), "敏感值 {value} 不应出现在日志中");
    }
    assert!(
        !line.contains("Bearer nested-secret"),
        "嵌套对象中的敏感值也必须脱敏"
    );
    assert!(line.contains("keep-me"), "非敏感字段必须保留");
    assert_eq!(
        line.matches("[REDACTED]").count(),
        REDACTION_KEYS.len() + 1,
        "10 个顶层敏感键 + 1 个嵌套敏感键都应脱敏"
    );

    let parsed: serde_json::Value = serde_json::from_str(line).expect("日志行是合法 JSON");
    assert_eq!(parsed["session_id"], json!(session_id));
    assert_eq!(parsed["stage"], json!("preflight"));
    assert_eq!(parsed["event"], json!("preflight.env"));
    assert_eq!(parsed["error_code"], json!("E_PREFLIGHT"));
    assert_eq!(parsed["payload"]["port"], json!(22));
}

#[test]
fn fixed_fields_keep_contract_order() {
    let session_id = new_session_id();
    let event = LogEvent::new(
        &session_id,
        SessionState::Idle,
        "session.created",
        LogLevel::Info,
    )
    .with_extra("note", json!("extra"));
    let line = to_jsonl(&event).expect("序列化日志");
    let fields = [
        "\"ts\":",
        "\"level\":",
        "\"session_id\":",
        "\"stage\":",
        "\"event\":",
        "\"error_code\":",
    ];
    let mut previous = 0;
    for field in fields {
        let position = line
            .find(field)
            .unwrap_or_else(|| panic!("日志缺少固定字段 {field}"));
        assert!(position >= previous, "固定字段 {field} 顺序错误");
        previous = position;
    }
    assert!(
        line.contains("\"error_code\":\"\""),
        "无错误码时必须写空串而不是省略"
    );
    assert!(line.starts_with('{'), "JSONL 行以 {{ 开头");
}

#[test]
fn invalid_event_name_is_rejected() {
    let session_id = new_session_id();
    let event = LogEvent::new(&session_id, SessionState::Idle, "Bad Event", LogLevel::Info);
    let error = to_jsonl(&event).expect_err("非法事件名必须报错");
    assert_eq!(error.code(), ErrorCode::Usage);
    assert!(error.message().contains("事件名"));
}

#[test]
fn level_switch_filters_events() {
    let directory = TempDirectory::new("level");
    let info_session = new_session_id();
    let mut info_sink =
        FileLogSink::open(directory.path(), &info_session, LogLevel::Info).expect("打开 info sink");
    info_sink
        .append(&LogEvent::new(
            &info_session,
            SessionState::Idle,
            "debug.event",
            LogLevel::Debug,
        ))
        .expect("debug 事件不应报错");
    let info_path = session_log_path(directory.path(), &info_session);
    assert_eq!(line_count(&info_path), 0, "info 开关不得写入 debug");

    info_sink
        .append(&LogEvent::new(
            &info_session,
            SessionState::Idle,
            "info.event",
            LogLevel::Info,
        ))
        .expect("写入 info 事件");
    assert_eq!(line_count(&info_path), 1);

    info_sink
        .append(&LogEvent::new(
            &info_session,
            SessionState::Idle,
            "trace.event",
            LogLevel::Trace,
        ))
        .expect("trace 事件不应报错");
    assert_eq!(line_count(&info_path), 1, "info 开关不得写入 trace");

    let trace_session = new_session_id();
    let mut trace_sink = FileLogSink::open(directory.path(), &trace_session, LogLevel::Trace)
        .expect("打开 trace sink");
    for (event_name, level) in [
        ("debug.event", LogLevel::Debug),
        ("trace.event", LogLevel::Trace),
        ("info.event", LogLevel::Info),
    ] {
        trace_sink
            .append(&LogEvent::new(
                &trace_session,
                SessionState::Idle,
                event_name,
                level,
            ))
            .expect("写入 trace 级事件");
    }
    let trace_path = session_log_path(directory.path(), &trace_session);
    assert_eq!(line_count(&trace_path), 3, "trace 开关应放行全部三级");

    assert!(LogLevel::Debug.is_enabled(LogLevel::Debug));
    assert!(LogLevel::Info.is_enabled(LogLevel::Debug));
    assert!(!LogLevel::Debug.is_enabled(LogLevel::Info));
    assert!(!LogLevel::Trace.is_enabled(LogLevel::Info));
}

#[test]
fn session_mismatch_is_rejected() {
    let directory = TempDirectory::new("mismatch");
    let session_id = new_session_id();
    let other_session = new_session_id();
    let mut sink =
        FileLogSink::open(directory.path(), &session_id, LogLevel::Info).expect("打开 sink");
    let event = LogEvent::new(
        &other_session,
        SessionState::Idle,
        "session.mismatch",
        LogLevel::Info,
    );
    let error = sink.append(&event).expect_err("会话 id 不一致必须报错");
    assert_eq!(error.code(), ErrorCode::Usage);
}

#[test]
fn invalid_session_id_is_rejected() {
    let directory = TempDirectory::new("bad-session");
    let error = FileLogSink::open(directory.path(), "not-a-uuid", LogLevel::Info)
        .expect_err("非法会话 id 必须报错");
    assert_eq!(error.code(), ErrorCode::Usage);
}
