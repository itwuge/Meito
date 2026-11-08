//! File: meito-link 协调服务入口。
//!
//! 先加载 `contracts/link.json`（缺失或字段不全即以 `E_CONFIG` 退出，不使用默认值），
//! 再用契约数值构造存储与 HTTP 服务。本进程不向 stdout 输出，也不记录任何 blob 内容。

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use meito_core::{ErrorCode, MeitoError};
use meito_link::{
    load_link_contract, ChannelPolicy, ChannelStore, InMemoryChannelStore, LinkServer,
    ACCESS_LOG_FILENAME, LINK_CONTRACT_PATH,
};

// ===== 默认值 =====

/// 未显式指定 `--bind` 时的监听地址；只绑回环，避免误暴露到公网。
const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// 访问日志所在目录；文件名由 `meito-link` 的日志模块固定。
const ACCESS_LOG_DIRECTORY: &str = "logs";

// ===== 入口 =====

/// 程序入口；所有失败只映射为契约退出码，不打印内容。
fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => std::process::exit(error.exit_code() as i32),
    }
}

/// 加载契约、构造存储并启动 HTTP 监听。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 参数错误返回 `E_USAGE`；契约缺失或非法返回 `E_CONFIG`；监听失败返回 `E_LINK`。
fn run() -> Result<(), MeitoError> {
    let options = parse_args(&env::args().skip(1).collect::<Vec<String>>())?;
    let contract = load_link_contract(&options.contract)?;
    let policy = ChannelPolicy::from_contract(&contract);
    let store: Arc<dyn ChannelStore> = Arc::new(InMemoryChannelStore::new(policy));
    let server = LinkServer::bind(options.bind, contract, store, options.access_log)
        .map_err(|error| MeitoError::io(ErrorCode::Link, "绑定协调服务监听地址失败", &error))?;
    server
        .run()
        .map_err(|error| MeitoError::io(ErrorCode::Link, "协调服务监听中断", &error))
}

// ===== 参数 =====

/// 已解析的命令行选项。
struct Options {
    /// 契约文件路径。
    contract: PathBuf,
    /// 监听地址。
    bind: SocketAddr,
    /// 访问日志文件路径。
    access_log: PathBuf,
}

/// 解析命令行参数。
///
/// # 参数
/// - `args`: 不含程序名的参数列表。
///
/// # 错误
/// 未知选项、选项缺少取值或 `--bind` 不是合法地址时返回 `E_USAGE`（退出码 2）。
fn parse_args(args: &[String]) -> Result<Options, MeitoError> {
    let mut contract = PathBuf::from(LINK_CONTRACT_PATH);
    let mut bind = DEFAULT_BIND.to_string();
    let mut access_log = PathBuf::from(ACCESS_LOG_DIRECTORY).join(ACCESS_LOG_FILENAME);
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--contract" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| MeitoError::new(ErrorCode::Usage, "--contract 缺少取值"))?;
                contract = PathBuf::from(value);
            }
            "--bind" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| MeitoError::new(ErrorCode::Usage, "--bind 缺少取值"))?;
                bind = value.clone();
            }
            "--access-log" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| MeitoError::new(ErrorCode::Usage, "--access-log 缺少取值"))?;
                access_log = PathBuf::from(value);
            }
            other => {
                return Err(MeitoError::new(
                    ErrorCode::Usage,
                    format!("未知选项：{other}"),
                ));
            }
        }
        index += 1;
    }
    let bind = bind.parse::<SocketAddr>().map_err(|error| {
        MeitoError::new(
            ErrorCode::Usage,
            format!("--bind 不是合法地址 {bind}：{error}"),
        )
    })?;
    Ok(Options {
        contract,
        bind,
        access_log,
    })
}
