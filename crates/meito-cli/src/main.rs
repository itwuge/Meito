//! File: Meito 本地 CLI 外壳。
//!
//! 外壳只负责参数解析与输出渲染；状态机、日志、台账与诊断逻辑全部调用 `meito-core`，
//! 不复制任何 Model 代码。M2 只实现 init/doctor/status/logs/env 五个子命令。

use std::path::{Path, PathBuf};

use meito_core::{
    init_workflow, load_ledger, read_log_lines, read_status, run_doctor, EnvironmentLedger,
    ErrorCode, LogLevel, MeitoError, SessionLogTail, SessionState, StatusReport, CONTRACT_VERSION,
};

// ===== 参数模型 =====

/// 解析后的命令行参数；命令专属选项在分发前统一校验归属。
#[derive(Debug, Default)]
struct ParsedArgs {
    command: Option<String>,
    repo: Option<String>,
    config: Option<String>,
    session: Option<String>,
    log_level: Option<String>,
    template: Option<String>,
    stage: Option<String>,
    json: bool,
    force: bool,
    deep: bool,
    follow: bool,
}

/// 顶层解析结果。
enum Parsed {
    Help,
    Version,
    Run(ParsedArgs),
}

// ===== 参数解析 =====

/// 解析参数；`--help` 与 `--version` 优先于子命令。
///
/// # 参数
/// - `args`: 不含程序名的参数列表。
///
/// # 错误
/// 未知选项、重复子命令、选项缺少取值时返回 `E_USAGE`。
fn parse_args(args: &[String]) -> Result<Parsed, MeitoError> {
    if args.iter().any(|value| value == "--help") {
        return Ok(Parsed::Help);
    }
    if args.iter().any(|value| value == "--version") {
        return Ok(Parsed::Version);
    }

    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--repo" => parsed.repo = Some(take_value(args, &mut index, "--repo")?),
            "--config" => parsed.config = Some(take_value(args, &mut index, "--config")?),
            "--session" => parsed.session = Some(take_value(args, &mut index, "--session")?),
            "--log-level" => parsed.log_level = Some(take_value(args, &mut index, "--log-level")?),
            "--template" => parsed.template = Some(take_value(args, &mut index, "--template")?),
            "--stage" => parsed.stage = Some(take_value(args, &mut index, "--stage")?),
            "--json" => parsed.json = true,
            "--force" => parsed.force = true,
            "--deep" => parsed.deep = true,
            "--follow" => parsed.follow = true,
            other if other.starts_with('-') => {
                return Err(MeitoError::new(
                    ErrorCode::Usage,
                    format!("未知选项：{other}"),
                ));
            }
            other => {
                if parsed.command.is_some() {
                    return Err(MeitoError::new(
                        ErrorCode::Usage,
                        format!("只能指定一个子命令，收到多余的 {other}"),
                    ));
                }
                parsed.command = Some(other.to_string());
            }
        }
        index += 1;
    }
    Ok(Parsed::Run(parsed))
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

// ===== 主流程 =====

/// 程序入口；把错误统一映射为契约退出码。
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match parse_args(&args) {
        Ok(parsed) => match execute(parsed) {
            Ok(code) => code,
            Err(error) => fail(&error),
        },
        Err(error) => fail(&error),
    };
    if code != 0 {
        std::process::exit(code);
    }
}

/// 执行解析结果。
///
/// # 参数
/// - `parsed`: 顶层解析结果。
///
/// # 错误
/// 命令实现返回的错误原样上抛，由 `main` 映射退出码。
fn execute(parsed: Parsed) -> Result<i32, MeitoError> {
    match parsed {
        Parsed::Help => {
            println!("{}", HELP_TEXT);
            Ok(0)
        }
        Parsed::Version => {
            println!(
                "meito {} (contract {})",
                env!("CARGO_PKG_VERSION"),
                CONTRACT_VERSION
            );
            Ok(0)
        }
        Parsed::Run(args) => execute_command(args),
    }
}

/// 分发五个已实现子命令。
///
/// # 参数
/// - `args`: 解析后的参数。
///
/// # 错误
/// 缺命令、未实现命令、选项归属错误或命令自身失败时返回错误。
fn execute_command(args: ParsedArgs) -> Result<i32, MeitoError> {
    let root = std::env::current_dir()
        .map_err(|error| MeitoError::io(ErrorCode::Internal, "读取当前工作目录失败", &error))?;
    // 级别开关在这里统一校验；trace 只允许显式命令行确认，避免环境变量悄悄打开连接细节。
    let log_level = resolve_log_level(args.log_level.as_deref())?;
    let command = match args.command.as_deref() {
        Some(value) => value.to_string(),
        None => {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                "缺少子命令；支持 init/doctor/status/logs/env",
            ))
        }
    };

    match command.as_str() {
        "init" => {
            ensure_init_options(&args)?;
            let template = args.template.as_deref().map(Path::new);
            let report = init_workflow(&root, template, args.force)?;
            if args.json {
                print_json(report.to_json())?;
            } else {
                println!("installed: {}", report.destination.display());
                println!("template: {}", report.template.display());
                println!("bytes: {}", report.bytes);
            }
            Ok(0)
        }
        "doctor" => {
            ensure_doctor_options(&args)?;
            let config = resolve_config(&args);
            let repo = resolve_repo(&args);
            let report = run_doctor(&root, config.as_deref(), repo.as_deref(), args.deep)?;
            if args.json {
                print_json(report.to_json())?;
            } else {
                render_doctor(&report, log_level);
            }
            if report.ok {
                Ok(0)
            } else {
                Err(MeitoError::new(
                    ErrorCode::Preflight,
                    format!("doctor 发现 {} 项失败", fail_count(&report)),
                ))
            }
        }
        "status" => {
            ensure_no_command_options(&args, "status")?;
            let report = read_status(&root, args.session.as_deref())?;
            if args.json {
                print_json(report.to_json())?;
            } else {
                render_status(&report);
            }
            Ok(0)
        }
        "logs" => {
            ensure_logs_options(&args)?;
            ensure_no_other_options(&args)?;
            let stage = parse_stage(args.stage.as_deref())?;
            if args.follow {
                follow_logs(&root, args.session.as_deref(), stage)?;
                Ok(0)
            } else {
                let records = read_log_lines(&root, args.session.as_deref(), stage)?;
                for record in records {
                    println!("{}", record.raw);
                }
                Ok(0)
            }
        }
        "env" => {
            ensure_no_command_options(&args, "env")?;
            let ledger = load_ledger(&root, args.session.as_deref())?;
            if args.json {
                print_json(ledger.to_json())?;
            } else {
                render_env(&ledger);
            }
            Ok(0)
        }
        other if UNSUPPORTED_COMMANDS.contains(&other) => Err(MeitoError::new(
            ErrorCode::Usage,
            format!("子命令 {other} 不在 M2 范围内，尚未实现"),
        )),
        other => Err(MeitoError::new(
            ErrorCode::Usage,
            format!("未知子命令：{other}"),
        )),
    }
}

// ===== 选项归属校验 =====

/// init 专属选项校验。
///
/// # 参数
/// - `args`: 解析后的参数。
///
/// # 错误
/// 出现 doctor/logs 专属选项时返回 `E_USAGE`。
fn ensure_init_options(args: &ParsedArgs) -> Result<(), MeitoError> {
    reject_if(args.deep, "--deep 只适用于 doctor")?;
    reject_if(args.stage.is_some(), "--stage 只适用于 logs")?;
    reject_if(args.follow, "--follow 只适用于 logs")
}

/// doctor 专属选项校验。
///
/// # 参数
/// - `args`: 解析后的参数。
///
/// # 错误
/// 出现 init/logs 专属选项时返回 `E_USAGE`。
fn ensure_doctor_options(args: &ParsedArgs) -> Result<(), MeitoError> {
    reject_if(args.template.is_some(), "--template 只适用于 init")?;
    reject_if(args.force, "--force 只适用于 init")?;
    reject_if(args.stage.is_some(), "--stage 只适用于 logs")?;
    reject_if(args.follow, "--follow 只适用于 logs")
}

/// logs 专属选项校验。
///
/// # 参数
/// - `args`: 解析后的参数。
///
/// # 错误
/// 出现 init/doctor 专属选项时返回 `E_USAGE`。
fn ensure_logs_options(args: &ParsedArgs) -> Result<(), MeitoError> {
    reject_if(args.template.is_some(), "--template 只适用于 init")?;
    reject_if(args.force, "--force 只适用于 init")?;
    reject_if(args.deep, "--deep 只适用于 doctor")
}

/// 无专属选项的命令校验。
///
/// # 参数
/// - `args`: 解析后的参数。
/// - `command`: 命令名，仅用于错误信息。
///
/// # 错误
/// 出现任何专属选项时返回 `E_USAGE`。
fn ensure_no_command_options(args: &ParsedArgs, command: &str) -> Result<(), MeitoError> {
    reject_if(
        args.template.is_some(),
        &format!("--template 不适用于 {command}"),
    )?;
    reject_if(args.force, &format!("--force 不适用于 {command}"))?;
    reject_if(args.deep, &format!("--deep 不适用于 {command}"))?;
    reject_if(args.stage.is_some(), &format!("--stage 不适用于 {command}"))?;
    reject_if(args.follow, &format!("--follow 不适用于 {command}"))?;
    ensure_no_other_options(args)
}

/// logs 路径额外校验：不允许 init/doctor 的专属选项。
///
/// # 参数
/// - `args`: 解析后的参数。
///
/// # 错误
/// 出现不适用选项时返回 `E_USAGE`。
fn ensure_no_other_options(args: &ParsedArgs) -> Result<(), MeitoError> {
    reject_if(args.template.is_some(), "--template 只适用于 init")?;
    reject_if(args.force, "--force 只适用于 init")?;
    reject_if(args.deep, "--deep 只适用于 doctor")
}

/// 条件成立时返回用法错误。
///
/// # 参数
/// - `condition`: 是否出现非法组合。
/// - `message`: 错误说明。
///
/// # 错误
/// 条件成立时返回 `E_USAGE`。
fn reject_if(condition: bool, message: &str) -> Result<(), MeitoError> {
    if condition {
        return Err(MeitoError::new(ErrorCode::Usage, message));
    }
    Ok(())
}

/// 统计失败检查项数量。
///
/// # 参数
/// - `report`: doctor 报告。
///
/// # 错误
/// 不返回错误。
fn fail_count(report: &meito_core::DoctorReport) -> usize {
    report
        .checks
        .iter()
        .filter(|check| check.status == meito_core::CheckStatus::Fail)
        .count()
}

// ===== 环境解析 =====

/// 解析日志级别开关。
///
/// # 参数
/// - `flag`: `--log-level` 的取值；`None` 时回退 `MEITO_LOG`。
///
/// # 错误
/// 取值非法返回 `E_USAGE`；仅通过 `MEITO_LOG=trace` 开启 trace 也返回 `E_USAGE`，
/// 因为 trace 必须由显式命令行确认。
fn resolve_log_level(flag: Option<&str>) -> Result<LogLevel, MeitoError> {
    if let Some(value) = flag {
        return LogLevel::parse_switch(value);
    }
    match std::env::var("MEITO_LOG") {
        Ok(value) if !value.is_empty() => {
            if value == "trace" {
                return Err(MeitoError::new(
                    ErrorCode::Usage,
                    "trace 级别需要显式 --log-level trace 确认，不能仅靠 MEITO_LOG 开启",
                ));
            }
            LogLevel::parse_switch(&value)
        }
        _ => Ok(LogLevel::Info),
    }
}

/// 解析配置文件路径；命令行优先于 `MEITO_CONFIG`。
///
/// # 参数
/// - `args`: 解析后的参数。
///
/// # 错误
/// 不返回错误。
fn resolve_config(args: &ParsedArgs) -> Option<PathBuf> {
    if let Some(value) = &args.config {
        return Some(PathBuf::from(value));
    }
    match std::env::var("MEITO_CONFIG") {
        Ok(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => None,
    }
}

/// 解析仓库坐标；命令行优先于 `MEITO_REPO`。
///
/// # 参数
/// - `args`: 解析后的参数。
///
/// # 错误
/// 不返回错误。
fn resolve_repo(args: &ParsedArgs) -> Option<String> {
    if let Some(value) = &args.repo {
        return Some(value.clone());
    }
    match std::env::var("MEITO_REPO") {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// 解析 `--stage` 为契约状态名。
///
/// # 参数
/// - `value`: 可选阶段字符串。
///
/// # 错误
/// 阶段名未知时返回 `E_USAGE`。
fn parse_stage(value: Option<&str>) -> Result<Option<SessionState>, MeitoError> {
    match value {
        Some(stage) => match SessionState::parse(stage) {
            Some(parsed) => Ok(Some(parsed)),
            None => Err(MeitoError::new(
                ErrorCode::Usage,
                format!("--stage 不是契约阶段名：{stage}"),
            )),
        },
        None => Ok(None),
    }
}

// ===== 输出渲染 =====

/// 以缩进 JSON 输出。
///
/// # 参数
/// - `value`: 待输出 JSON。
///
/// # 错误
/// 序列化失败返回 `E_INTERNAL`。
fn print_json(value: serde_json::Value) -> Result<(), MeitoError> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| MeitoError::new(ErrorCode::Internal, format!("JSON 输出失败：{error}")))?;
    println!("{text}");
    Ok(())
}

/// 渲染 doctor 文本报告。
///
/// # 参数
/// - `report`: doctor 报告。
/// - `level`: 已校验的日志级别，展示在报告头部。
///
/// # 错误
/// 不返回错误。
fn render_doctor(report: &meito_core::DoctorReport, level: LogLevel) {
    println!("root: {}", report.root.display());
    println!("log_level: {}", level.as_str());
    for check in &report.checks {
        println!("[{}] {}: {}", check.status.as_str(), check.id, check.detail);
    }
    println!("doctor: {}", if report.ok { "ok" } else { "failed" });
}

/// 渲染 status 文本报告。
///
/// # 参数
/// - `report`: status 报告。
///
/// # 错误
/// 不返回错误。
fn render_status(report: &StatusReport) {
    println!("session: {}", optional_text(&report.session_id));
    println!("state: {}", report.state.as_str());
    println!("terminal: {}", report.terminal);
    println!("run_id: {}", optional_text(&report.run_id));
    println!("entry: {}", optional_text(&report.entry));
    println!("last_heartbeat: {}", optional_text(&report.last_heartbeat));
    println!("p2p_provider: {}", optional_text(&report.p2p_provider));
    println!("error_code: {}", optional_text(&report.error_code));
    if report.entry_urls.is_empty() {
        println!("entry_urls: -");
    } else {
        for (key, value) in &report.entry_urls {
            println!("entry_urls.{key}: {value}");
        }
    }
}

/// 渲染 17 字段环境台账。
///
/// # 参数
/// - `ledger`: 环境台账。
///
/// # 错误
/// 不返回错误。
fn render_env(ledger: &EnvironmentLedger) {
    for (key, value) in ledger.entries() {
        println!("{key}: {}", if value.is_empty() { "-" } else { value });
    }
}

/// 把可选字符串渲染为 `-`。
///
/// # 参数
/// - `value`: 可选字符串。
///
/// # 错误
/// 不返回错误。
fn optional_text(value: &Option<String>) -> &str {
    match value {
        Some(text) if !text.is_empty() => text.as_str(),
        _ => "-",
    }
}

/// 跟随日志：先输出已有行，再按 200ms 轮询新增行。
///
/// # 参数
/// - `root`: 工作目录。
/// - `session`: 会话选择。
/// - `stage`: 可选阶段过滤。
///
/// # 错误
/// 打开日志或轮询失败时返回错误；该函数正常情况下不返回。
fn follow_logs(
    root: &Path,
    session: Option<&str>,
    stage: Option<SessionState>,
) -> Result<(), MeitoError> {
    let mut tail = SessionLogTail::open(root, session, stage)?;
    loop {
        let records = tail.poll()?;
        for record in records {
            println!("{}", record.raw);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// 统一输出错误并把契约退出码返回给进程。
///
/// # 参数
/// - `error`: 内核错误。
///
/// # 错误
/// 不返回错误；返回值为进程退出码。
fn fail(error: &MeitoError) -> i32 {
    eprintln!(
        "meito: {} (exit {}): {}",
        error.code().as_str(),
        error.exit_code(),
        error.message()
    );
    error.exit_code() as i32
}

// ===== 常量 =====

/// 契约中存在但 M2 尚未实现的子命令。
const UNSUPPORTED_COMMANDS: [&str; 11] = [
    "up", "web", "share", "connect", "entry", "pull", "kconfig", "down", "clean", "p2p", "config",
];

/// 帮助文本。
const HELP_TEXT: &str = "meito - GitHub Actions 点对点远程会话\n\
\n\
用法: meito <command> [options]\n\
\n\
M2 已实现命令:\n\
  init      安装 workflow 模板到目标仓库\n\
  doctor    只读环境与权限诊断\n\
  status    查看最近或指定会话状态\n\
  logs      查看或跟随会话日志\n\
  env       输出环境台账\n\
\n\
通用选项:\n\
  --repo <owner/repo>   --config <path>   --session <uuid|latest>\n\
  --log-level <info|debug|trace>   --json\n\
\n\
init 选项: --template <path>   --force\n\
doctor 选项: --deep\n\
logs 选项: --stage <state>   --follow\n\
\n\
退出码见 contracts/error-codes.json；E_USAGE=2、E_PREFLIGHT=4、E_LOGS_UNAVAILABLE=12。";
