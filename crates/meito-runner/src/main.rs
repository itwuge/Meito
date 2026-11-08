//! File: meito runner 侧监督进程（M3 子块：actor 校验与预检）。
//!
//! 本进程只做两件事：按 `meito.toml` 的 `[security] allowed_actors` 做 fail-closed 的
//! actor 门禁，以及按 `contracts/udp-probe.json` 执行 UDP/DNS/磁盘预检。
//! 所有诊断只写结构化 JSONL 日志（`logs/session-<id>.jsonl`），不向 stdout 输出；
//! 调用方通过契约退出码判断结果。真实 runner 上的网络行为仍标“未验证”。

mod actor;
mod config;
mod host_port;
mod preflight;

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use meito_core::{ErrorCode, FileLogSink, LogEvent, LogLevel, LogSink, MeitoError, SessionState};
use serde_json::Value;

use crate::actor::{decide_actor, ActorDecision, ActorSource};
use crate::config::{load_repo_config_file, ConfigLoad, CONFIG_FILE_NAME};
use crate::host_port::HostPort;
use crate::preflight::{
    check_dns, probe_disk, probe_udp, PreflightReport, StepOutcome, StepReport, UdpOutcome,
    UdpProbeReport, PROBE_ATTEMPTS, PROBE_TIMEOUT,
};

// ===== 参数模型 =====

/// 解析后的命令行参数；命令专属选项在分发前统一校验归属。
#[derive(Debug, Default)]
struct Options {
    command: Option<String>,
    session: Option<String>,
    config: Option<String>,
    actor: Option<String>,
    allow_actors: Vec<String>,
    udp_target: Option<String>,
    dns_host: Option<String>,
    disk_path: Option<String>,
}

/// 顶层解析结果。
enum Parsed {
    Run(Options),
}

// ===== 参数解析 =====

/// 解析 runner 参数；只接受 `check-actor` 与 `preflight` 两个子命令。
///
/// # 参数
/// - `args`: 不含程序名的参数列表。
///
/// # 错误
/// 未知选项、重复子命令、选项缺少取值时返回 `E_USAGE`（退出码 2）。
fn parse_args(args: &[String]) -> Result<Parsed, MeitoError> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--session" => options.session = Some(take_value(args, &mut index, "--session")?),
            "--config" => options.config = Some(take_value(args, &mut index, "--config")?),
            "--actor" => options.actor = Some(take_value(args, &mut index, "--actor")?),
            "--allow-actor" => {
                options
                    .allow_actors
                    .push(take_value(args, &mut index, "--allow-actor")?)
            }
            "--udp-target" => {
                options.udp_target = Some(take_value(args, &mut index, "--udp-target")?)
            }
            "--dns-host" => options.dns_host = Some(take_value(args, &mut index, "--dns-host")?),
            "--disk-path" => options.disk_path = Some(take_value(args, &mut index, "--disk-path")?),
            other if other.starts_with('-') => {
                return Err(MeitoError::new(
                    ErrorCode::Usage,
                    format!("未知选项：{other}"),
                ));
            }
            other => {
                if options.command.is_some() {
                    return Err(MeitoError::new(
                        ErrorCode::Usage,
                        format!("只能指定一个子命令，收到多余的 {other}"),
                    ));
                }
                options.command = Some(other.to_string());
            }
        }
        index += 1;
    }
    Ok(Parsed::Run(options))
}

/// 读取选项后的值。
///
/// # 参数
/// - `args`: 参数列表。
/// - `index`: 当前选项下标；成功后移动到值下标。
/// - `option`: 选项名，仅用于错误信息。
///
/// # 错误
/// 缺少取值，或取值看起来像另一个选项时返回 `E_USAGE`。
fn take_value(args: &[String], index: &mut usize, option: &str) -> Result<String, MeitoError> {
    *index += 1;
    match args.get(*index) {
        Some(value) if !value.starts_with("--") => Ok(value.clone()),
        _ => Err(MeitoError::new(
            ErrorCode::Usage,
            format!("{option} 缺少取值"),
        )),
    }
}

// ===== 入口与分发 =====

/// 程序入口；错误只映射为契约退出码，不打印任何内容。
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let code = match parse_args(&args) {
        Ok(Parsed::Run(options)) => match execute(&options) {
            Ok(code) => code,
            Err(error) => error.exit_code() as i32,
        },
        Err(error) => error.exit_code() as i32,
    };
    if code != 0 {
        std::process::exit(code);
    }
}

/// 分发两个 runner 子命令。
///
/// # 参数
/// - `options`: 解析后的参数。
///
/// # 错误
/// 缺命令、未知命令、选项归属错误或命令自身失败时返回错误。
fn execute(options: &Options) -> Result<i32, MeitoError> {
    match options.command.as_deref() {
        Some("check-actor") => run_check_actor(options),
        Some("preflight") => run_preflight(options),
        Some(other) => Err(MeitoError::new(
            ErrorCode::Usage,
            format!("未知子命令：{other}；只支持 check-actor、preflight"),
        )),
        None => Err(MeitoError::new(
            ErrorCode::Usage,
            "缺少子命令；只支持 check-actor、preflight",
        )),
    }
}

/// 解析会话 id：命令行优先，其次 `MEITO_SESSION_ID`。
///
/// # 参数
/// - `options`: 解析后的参数。
///
/// # 错误
/// 两者都缺失或为空时返回 `E_USAGE`；格式合法性由 `FileLogSink::open` 校验。
fn resolve_session(options: &Options) -> Result<String, MeitoError> {
    let candidate = options.session.clone().or_else(|| {
        env::var("MEITO_SESSION_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    candidate.ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Usage,
            "--session 或 MEITO_SESSION_ID 必须提供 UUID v4 会话 id",
        )
    })
}

/// 返回当前工作目录作为日志与配置的根。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 读取当前目录失败时返回 `E_INTERNAL`。
fn current_root() -> Result<PathBuf, MeitoError> {
    env::current_dir()
        .map_err(|error| MeitoError::io(ErrorCode::Internal, "读取当前工作目录失败", &error))
}

/// 解析配置文件路径：`--config` 优先，否则取仓库根 `meito.toml`。
///
/// # 参数
/// - `options`: 解析后的参数。
/// - `root`: 工作目录。
///
/// # 错误
/// 不返回错误。
fn config_path(options: &Options, root: &Path) -> PathBuf {
    match &options.config {
        Some(value) => PathBuf::from(value),
        None => root.join(CONFIG_FILE_NAME),
    }
}

/// 拒绝不属于当前子命令的选项。
///
/// # 参数
/// - `options`: 解析后的参数。
/// - `command`: 当前子命令名，仅用于错误信息。
///
/// # 错误
/// 出现不该有的选项时返回 `E_USAGE`。
fn reject_foreign_options(options: &Options, command: &str) -> Result<(), MeitoError> {
    if command == "check-actor" {
        if options.udp_target.is_some() || options.dns_host.is_some() || options.disk_path.is_some()
        {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                "--udp-target/--dns-host/--disk-path 只适用于 preflight",
            ));
        }
        return Ok(());
    }
    if options.actor.is_some() || !options.allow_actors.is_empty() {
        return Err(MeitoError::new(
            ErrorCode::Usage,
            "--actor/--allow-actor 只适用于 check-actor",
        ));
    }
    Ok(())
}

// ===== actor 子命令 =====

/// 执行 actor 白名单门禁；缺配置或空名单一律拒绝。
///
/// # 参数
/// - `options`: 解析后的参数。
///
/// # 错误
/// 参数、配置或日志失败返回对应错误；actor 被拒返回 `E_ACTOR_NOT_ALLOWED`（退出码 3）。
fn run_check_actor(options: &Options) -> Result<i32, MeitoError> {
    reject_foreign_options(options, "check-actor")?;
    let root = current_root()?;
    let session = resolve_session(options)?;
    let mut sink = FileLogSink::open(&root, &session, LogLevel::Info)?;
    let path = config_path(options, &root);

    let config = match load_repo_config_file(&path) {
        Ok(loaded) => loaded,
        Err(error) => {
            let event = LogEvent::new(
                &session,
                SessionState::Preflight,
                "actor.config_invalid",
                LogLevel::Error,
            )
            .with_error_code(error.code())
            .with_extra("detail", Value::String(error.message().to_string()));
            sink.emit(&event)?;
            return Err(error);
        }
    };

    let (allowed, source, config_missing) = resolve_allowlist(options, &config)?;
    let actor = resolve_actor(options);
    let decision = decide_actor(actor.as_deref(), &allowed, config_missing);

    let mut event = LogEvent::new(
        &session,
        SessionState::Preflight,
        "actor.checked",
        if decision.is_allowed() {
            LogLevel::Info
        } else {
            LogLevel::Error
        },
    )
    .with_extra("actor", optional_text(actor.as_deref()))
    .with_extra("source", Value::String(source.as_str().to_string()))
    .with_extra("allowed", Value::Bool(decision.is_allowed()))
    .with_extra(
        "reason",
        match decision.reason() {
            Some(reason) => Value::String(reason.as_str().to_string()),
            None => Value::Null,
        },
    );
    if !decision.is_allowed() {
        event = event.with_error_code(ErrorCode::ActorNotAllowed);
    }
    sink.emit(&event)?;

    match decision {
        ActorDecision::Allowed => Ok(0),
        ActorDecision::Denied(reason) => Err(MeitoError::new(
            ErrorCode::ActorNotAllowed,
            format!(
                "actor {} 被拒绝：{}",
                actor.as_deref().unwrap_or("<缺失>"),
                reason
            ),
        )),
    }
}

/// 计算生效白名单与来源；workflow 覆盖优先且忽略配置缺失。
///
/// # 参数
/// - `options`: 解析后的参数。
/// - `config`: 已加载的配置结果。
///
/// # 错误
/// 覆盖白名单元素为空时返回 `E_CONFIG`（退出码 2）。
fn resolve_allowlist(
    options: &Options,
    config: &ConfigLoad,
) -> Result<(Vec<String>, ActorSource, bool), MeitoError> {
    if !options.allow_actors.is_empty() {
        if let Some(empty) = options
            .allow_actors
            .iter()
            .find(|entry| entry.trim().is_empty())
        {
            return Err(MeitoError::new(
                ErrorCode::Config,
                format!("--allow-actor 元素不得为空，收到 {empty:?}"),
            ));
        }
        return Ok((options.allow_actors.clone(), ActorSource::Override, false));
    }
    match config {
        ConfigLoad::Missing => Ok((Vec::new(), ActorSource::Config, true)),
        ConfigLoad::Loaded(repo) => {
            Ok((repo.allowed_actors().to_vec(), ActorSource::Config, false))
        }
    }
}

/// 解析实际 actor：`--actor` 优先，其次 `GITHUB_ACTOR`。
///
/// # 参数
/// - `options`: 解析后的参数。
///
/// # 错误
/// 不返回错误；缺失由 `decide_actor` 判定为 `missing_actor`。
fn resolve_actor(options: &Options) -> Option<String> {
    options.actor.clone().or_else(|| {
        env::var("GITHUB_ACTOR")
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

// ===== preflight 子命令 =====

/// 依次执行 UDP、DNS、磁盘预检，每一步都落结构化日志。
///
/// # 参数
/// - `options`: 解析后的参数。
///
/// # 错误
/// 参数、配置或日志失败返回对应错误；预检失败按 UDP→DNS→磁盘的顺序返回第一个错误。
fn run_preflight(options: &Options) -> Result<i32, MeitoError> {
    reject_foreign_options(options, "preflight")?;
    let root = current_root()?;
    let session = resolve_session(options)?;
    let mut sink = FileLogSink::open(&root, &session, LogLevel::Info)?;
    let path = config_path(options, &root);

    let config = match load_repo_config_file(&path) {
        Ok(loaded) => loaded,
        Err(error) => {
            let event = LogEvent::new(
                &session,
                SessionState::Preflight,
                "preflight.config_invalid",
                LogLevel::Error,
            )
            .with_error_code(error.code())
            .with_extra("detail", Value::String(error.message().to_string()));
            sink.emit(&event)?;
            return Err(error);
        }
    };

    let target = resolve_udp_target(options, &config)?;
    let dns_host = options
        .dns_host
        .clone()
        .or_else(|| target.as_ref().map(|value| value.host().to_string()));
    let disk_path = PathBuf::from(options.disk_path.clone().unwrap_or_else(|| ".".to_string()));
    let started = Instant::now();

    let udp = probe_udp(target.as_ref(), PROBE_ATTEMPTS, PROBE_TIMEOUT);
    emit_udp(&mut sink, &session, &udp)?;
    let dns = check_dns(dns_host.as_deref());
    emit_step(
        &mut sink,
        &session,
        "preflight.dns",
        &dns,
        "host",
        dns_host.as_deref(),
    )?;
    let disk = probe_disk(&disk_path);
    emit_step(
        &mut sink,
        &session,
        "preflight.disk",
        &disk,
        "path",
        Some(disk_path.to_string_lossy().as_ref()),
    )?;

    let report = PreflightReport { udp, dns, disk };
    let error = report.error();
    emit_summary(
        &mut sink,
        &session,
        &report,
        error.as_ref(),
        elapsed_ms(started),
    )?;
    match error {
        Some(error) => Err(error),
        None => Ok(0),
    }
}

/// 解析 UDP 目标：命令行优先，其次 `[network] udp_probe_target`。
///
/// # 参数
/// - `options`: 解析后的参数。
/// - `config`: 已加载的配置结果。
///
/// # 错误
/// 目标是 `host:port` 之外的形状时返回 `E_CONFIG`（退出码 2）。
fn resolve_udp_target(
    options: &Options,
    config: &ConfigLoad,
) -> Result<Option<HostPort>, MeitoError> {
    if let Some(text) = &options.udp_target {
        return HostPort::parse(text).map(Some);
    }
    match config {
        ConfigLoad::Missing => Ok(None),
        ConfigLoad::Loaded(repo) => Ok(repo.udp_probe_target().cloned()),
    }
}

/// 写入 UDP 步骤日志。
///
/// # 参数
/// - `sink`: 会话日志。
/// - `session`: 会话 id。
/// - `report`: UDP 探测结果。
///
/// # 错误
/// 日志写盘失败时返回 `E_LOGS_UNAVAILABLE`。
fn emit_udp(
    sink: &mut FileLogSink,
    session: &str,
    report: &UdpProbeReport,
) -> Result<(), MeitoError> {
    let level = if report.outcome == UdpOutcome::Ok {
        LogLevel::Info
    } else {
        LogLevel::Error
    };
    let mut event = LogEvent::new(session, SessionState::Preflight, "preflight.udp", level);
    if let Some(code) = report.outcome.error_code() {
        event = event.with_error_code(code);
    }
    let event = event
        .with_extra("result", Value::String(report.outcome.as_str().to_string()))
        .with_extra("target", optional_text(report.target.as_deref()))
        .with_extra("attempts_used", Value::from(report.attempts_used))
        .with_extra("duration_ms", Value::from(report.duration_ms))
        .with_extra("detail", optional_text(report.detail.as_deref()));
    sink.emit(&event)
}

/// 写入 DNS 或磁盘步骤日志。
///
/// # 参数
/// - `sink`: 会话日志。
/// - `session`: 会话 id。
/// - `event_name`: 事件名，必须是 `preflight.dns` 或 `preflight.disk`。
/// - `report`: 步骤结果。
/// - `subject_key`: 目标字段名（`host` 或 `path`）。
/// - `subject`: 目标值。
///
/// # 错误
/// 日志写盘失败时返回 `E_LOGS_UNAVAILABLE`。
fn emit_step(
    sink: &mut FileLogSink,
    session: &str,
    event_name: &str,
    report: &StepReport,
    subject_key: &str,
    subject: Option<&str>,
) -> Result<(), MeitoError> {
    let level = match report.outcome {
        StepOutcome::Failed => LogLevel::Error,
        StepOutcome::Ok | StepOutcome::Skipped => LogLevel::Info,
    };
    let mut event = LogEvent::new(session, SessionState::Preflight, event_name, level);
    if report.outcome == StepOutcome::Failed {
        event = event.with_error_code(ErrorCode::Preflight);
    }
    let event = event
        .with_extra("result", Value::String(report.outcome.as_str().to_string()))
        .with_extra(subject_key, optional_text(subject))
        .with_extra("duration_ms", Value::from(report.duration_ms))
        .with_extra("detail", optional_text(report.detail.as_deref()));
    sink.emit(&event)
}

/// 写入预检汇总日志。
///
/// # 参数
/// - `sink`: 会话日志。
/// - `session`: 会话 id。
/// - `report`: 三步结果。
/// - `error`: 汇总错误；成功时为 `None`。
/// - `total_ms`: 预检总耗时。
///
/// # 错误
/// 日志写盘失败时返回 `E_LOGS_UNAVAILABLE`。
fn emit_summary(
    sink: &mut FileLogSink,
    session: &str,
    report: &PreflightReport,
    error: Option<&MeitoError>,
    total_ms: u64,
) -> Result<(), MeitoError> {
    let mut event = LogEvent::new(
        session,
        SessionState::Preflight,
        "preflight.summary",
        if error.is_some() {
            LogLevel::Error
        } else {
            LogLevel::Info
        },
    )
    .with_extra(
        "result",
        Value::String(if error.is_some() { "failed" } else { "ok" }.to_string()),
    )
    .with_extra(
        "udp",
        Value::String(report.udp.outcome.as_str().to_string()),
    )
    .with_extra(
        "dns",
        Value::String(report.dns.outcome.as_str().to_string()),
    )
    .with_extra(
        "disk",
        Value::String(report.disk.outcome.as_str().to_string()),
    )
    .with_extra("duration_ms", Value::from(total_ms));
    if let Some(error) = error {
        event = event.with_error_code(error.code());
    }
    sink.emit(&event)
}

/// 把可选字符串转成 JSON 值；缺失写 `null`，便于日志侧区分“空”和“没有”。
///
/// # 参数
/// - `value`: 可选文本。
///
/// # 错误
/// 不返回错误。
fn optional_text(value: Option<&str>) -> Value {
    match value {
        Some(text) => Value::String(text.to_string()),
        None => Value::Null,
    }
}

/// 把 `Instant` 转成毫秒。
///
/// # 参数
/// - `started`: 起始时刻。
///
/// # 错误
/// 不返回错误。
fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
