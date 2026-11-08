//! File: CLI 外壳与内核的契约级运行用例——退出码映射与五个已实现子命令。
//!
//! 每个用例都真正启动 `meito` 进程并在临时工作目录里跑，断言的是进程退出码与 stdout，
//! 而不是内部函数返回值；这样静态契约里的“错误码 → 退出码”才被实际验证。

use std::path::{Path, PathBuf};
use std::process::Command;

use meito_core::{new_session_id, to_jsonl, LogEvent, LogLevel, SessionState};
use serde_json::Value;

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
        let path = std::env::temp_dir().join(format!("meito-cli-{}-{}", label, new_session_id()));
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

    /// 在临时目录内创建文件（含父目录）。
    ///
    /// # 参数
    /// - `relative`: 相对临时目录的路径。
    /// - `content`: 文件内容。
    ///
    /// # 错误
    /// 目录或文件写入失败时 panic。
    fn write(&self, relative: &str, content: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录");
        }
        std::fs::write(&path, content).expect("写入测试文件");
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!("清理临时目录 {} 失败：{error}", self.path.display());
        }
    }
}

// ===== 进程执行 =====

/// 进程执行结果。
struct RunResult {
    code: i32,
    stdout: String,
    stderr: String,
}

impl RunResult {
    /// 断言退出码，失败时把 stderr 一并带出便于定位。
    ///
    /// # 参数
    /// - `expected`: 期望退出码。
    ///
    /// # 错误
    /// 退出码不符时 panic。
    fn assert_code(&self, expected: i32) {
        assert_eq!(
            self.code, expected,
            "退出码不符；stdout={}\nstderr={}",
            self.stdout, self.stderr
        );
    }

    /// 把 stdout 解析为 JSON。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// stdout 不是合法 JSON 时 panic。
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).expect("stdout 应是 JSON")
    }
}

/// 在指定工作目录运行 `meito`，并清掉可能干扰的环境变量。
///
/// # 参数
/// - `root`: 工作目录。
/// - `args`: 参数列表。
/// - `envs`: 额外设置的环境变量。
///
/// # 错误
/// 进程启动失败时 panic。
fn run(root: &Path, args: &[&str], envs: &[(&str, &str)]) -> RunResult {
    let mut command = Command::new(env!("CARGO_BIN_EXE_meito"));
    command
        .current_dir(root)
        .args(args)
        .env_remove("MEITO_LOG")
        .env_remove("MEITO_CONFIG")
        .env_remove("MEITO_REPO");
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("启动 meito 进程");
    RunResult {
        code: output
            .status
            .code()
            .expect("进程应正常退出而不是被信号终止"),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

// ===== 退出码映射 =====

#[test]
fn usage_errors_exit_with_two() {
    let directory = TempDirectory::new("usage");

    // 未知子命令、非法级别、非法会话 id 都映射到 E_USAGE=2。
    let unknown = run(directory.path(), &["nope"], &[]);
    unknown.assert_code(2);
    assert!(unknown.stderr.contains("E_USAGE"), "stderr 应带错误码");
    assert!(unknown.stderr.contains("exit 2"), "stderr 应带退出码");

    let bad_level = run(directory.path(), &["status", "--log-level", "verbose"], &[]);
    bad_level.assert_code(2);
    assert!(bad_level.stderr.contains("E_USAGE"));
}

#[test]
fn trace_via_env_alone_exits_with_two() {
    let directory = TempDirectory::new("trace-env");
    let result = run(directory.path(), &["status"], &[("MEITO_LOG", "trace")]);
    result.assert_code(2);
    assert!(
        result.stderr.contains("显式"),
        "应说明 trace 需要显式命令行确认：{}",
        result.stderr
    );

    // 显式 --log-level trace 才允许通过。
    let confirmed = run(directory.path(), &["status", "--log-level", "trace"], &[]);
    confirmed.assert_code(0);
}

#[test]
fn missing_logs_exit_with_twelve() {
    let directory = TempDirectory::new("logs-missing");
    // 没有任何 logs/ 目录。
    let none = run(directory.path(), &["logs"], &[]);
    none.assert_code(12);
    assert!(none.stderr.contains("E_LOGS_UNAVAILABLE"));

    // 显式指定但文件不存在。
    let session_id = new_session_id();
    let absent = run(directory.path(), &["logs", "--session", &session_id], &[]);
    absent.assert_code(12);
    assert!(absent.stderr.contains("E_LOGS_UNAVAILABLE"));
}

#[test]
fn missing_explicit_session_status_exits_with_twelve() {
    let directory = TempDirectory::new("status-orphan");
    let session_id = new_session_id();
    let result = run(directory.path(), &["status", "--session", &session_id], &[]);
    result.assert_code(12);
    assert!(result.stderr.contains("E_ORPHAN"));
}

// ===== status =====

#[test]
fn status_without_snapshot_is_idle() {
    let directory = TempDirectory::new("status-idle");
    let result = run(directory.path(), &["status", "--json"], &[]);
    result.assert_code(0);
    let value = result.json();
    assert_eq!(value["state"], Value::String("idle".to_string()));
    assert!(!value["terminal"].as_bool().expect("terminal 应是布尔"));
    assert!(
        value["session_id"].is_null(),
        "无会话时 session_id 应为 null"
    );
}

// ===== env =====

#[test]
fn env_outputs_seventeen_contract_fields() {
    let directory = TempDirectory::new("env");
    let result = run(directory.path(), &["env", "--json"], &[]);
    result.assert_code(0);
    let value = result.json();
    let object = value.as_object().expect("env 输出应是 JSON 对象");
    assert_eq!(object.len(), 17, "环境台账必须正好 17 个字段");

    for key in [
        "session_id",
        "run_id",
        "image",
        "runner_label",
        "os",
        "meito_version",
        "frpc_version",
        "ttyd_version",
        "x11vnc_version",
        "websockify_version",
        "p2p_provider",
        "p2p_state",
        "udp_probe",
        "ice_candidates",
        "entry_ports",
        "exit_code",
        "error_code",
    ] {
        assert!(object.contains_key(key), "缺少台账字段 {key}");
    }
    assert_eq!(value["p2p_state"], Value::String("not_started".to_string()));
    assert_eq!(value["udp_probe"], Value::String("not_probed".to_string()));
    assert_eq!(value["exit_code"], Value::String("0".to_string()));
    assert_eq!(
        value["meito_version"],
        Value::String(env!("CARGO_PKG_VERSION").to_string()),
        "meito_version 应取当前 crate 版本"
    );
}

// ===== logs =====

#[test]
fn logs_stage_filter_only_returns_matching_lines() {
    let directory = TempDirectory::new("logs-stage");
    let session_id = new_session_id();
    let first = to_jsonl(&LogEvent::new(
        &session_id,
        SessionState::Idle,
        "session.created",
        LogLevel::Info,
    ))
    .expect("生成 idle 日志");
    let second = to_jsonl(&LogEvent::new(
        &session_id,
        SessionState::Preflight,
        "preflight.done",
        LogLevel::Info,
    ))
    .expect("生成 preflight 日志");
    directory.write(
        &format!("logs/session-{session_id}.jsonl"),
        &format!("{first}\n{second}\n"),
    );

    let all = run(directory.path(), &["logs"], &[]);
    all.assert_code(0);
    assert_eq!(all.stdout.lines().count(), 2, "不过滤时应输出两行");

    let filtered = run(directory.path(), &["logs", "--stage", "preflight"], &[]);
    filtered.assert_code(0);
    let lines: Vec<&str> = filtered.stdout.lines().collect();
    assert_eq!(lines.len(), 1, "阶段过滤应只剩一行");
    assert!(
        lines[0].contains("\"preflight\""),
        "过滤结果应属于 preflight：{}",
        lines[0]
    );

    let bad_stage = run(directory.path(), &["logs", "--stage", "bogus"], &[]);
    bad_stage.assert_code(2);
    assert!(bad_stage.stderr.contains("E_USAGE"));
}

// ===== init =====

#[test]
fn init_installs_template_and_requires_force_to_overwrite() {
    let directory = TempDirectory::new("init");
    directory.write("templates/meito-session.yml", "name: meito-session\n");

    let first = run(directory.path(), &["init", "--json"], &[]);
    first.assert_code(0);
    let value = first.json();
    assert_eq!(
        value["destination"],
        Value::String(
            directory
                .path()
                .join(".github/workflows/meito-session.yml")
                .display()
                .to_string()
        )
    );
    assert!(
        directory
            .path()
            .join(".github/workflows/meito-session.yml")
            .is_file(),
        "目标 workflow 应已安装"
    );

    // 已存在且未加 --force：E_USAGE=2，不能静默覆盖用户文件。
    let second = run(directory.path(), &["init"], &[]);
    second.assert_code(2);
    assert!(second.stderr.contains("E_USAGE"));

    let forced = run(directory.path(), &["init", "--force"], &[]);
    forced.assert_code(0);
}

#[test]
fn init_with_missing_template_exits_with_four() {
    let directory = TempDirectory::new("init-missing");
    let result = run(directory.path(), &["init"], &[]);
    result.assert_code(4);
    assert!(result.stderr.contains("E_PREFLIGHT"));
}

// ===== doctor =====

#[test]
fn doctor_fails_when_contracts_are_missing() {
    let directory = TempDirectory::new("doctor-fail");
    let result = run(directory.path(), &["doctor", "--json"], &[]);
    result.assert_code(4);
    assert!(result.stderr.contains("E_PREFLIGHT"));

    // JSON 报告本身必须已经打印出来，便于定位失败项。
    let value = result.json();
    assert_eq!(value["ok"], Value::Bool(false));
    let checks = value["checks"].as_array().expect("checks 应是数组");
    assert!(
        checks
            .iter()
            .any(|check| check["id"] == Value::String("contract.files".to_string())),
        "报告应包含契约文件检查项"
    );
}

#[test]
fn doctor_rejects_bad_option_ownership() {
    let directory = TempDirectory::new("doctor-options");
    let result = run(directory.path(), &["doctor", "--force"], &[]);
    result.assert_code(2);
    assert!(result.stderr.contains("E_USAGE"));
}

// ===== 版本与帮助 =====

#[test]
fn version_and_help_exit_with_zero() {
    let directory = TempDirectory::new("version");

    let version = run(directory.path(), &["--version"], &[]);
    version.assert_code(0);
    assert!(
        version.stdout.contains("meito") && version.stdout.contains("contract 0.1.0"),
        "版本输出应包含程序名与契约版本：{}",
        version.stdout
    );

    let help = run(directory.path(), &["--help"], &[]);
    help.assert_code(0);
    assert!(help.stdout.contains("init"), "帮助应列出 init");
}
