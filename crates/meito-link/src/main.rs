//! File: meito-link 协调服务入口（M3 骨架）。
//!
//! 当前只做契约加载与依赖构造：`contracts/link.json` 缺失或字段不全时立即以 `E_CONFIG`
//! （退出码 2）退出，不使用任何默认值。HTTP 监听与请求处理在契约 0.3.0 落地后接入；
//! 本进程不向 stdout 输出，也不记录任何 blob 内容。

use std::env;
use std::path::PathBuf;

use meito_core::{ErrorCode, MeitoError};
use meito_link::{load_link_contract, ChannelPolicy, InMemoryChannelStore, LINK_CONTRACT_PATH};

// ===== 入口 =====

/// 程序入口；所有失败只映射为契约退出码，不打印内容。
fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => std::process::exit(error.exit_code() as i32),
    }
}

/// 加载契约并构造存储；HTTP 服务尚未接入。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 参数错误返回 `E_USAGE`；契约缺失或非法返回 `E_CONFIG`。
fn run() -> Result<(), MeitoError> {
    let args: Vec<String> = env::args().skip(1).collect();
    let contract_path = parse_contract_path(&args)?;
    let contract = load_link_contract(&contract_path)?;
    let store = InMemoryChannelStore::new(ChannelPolicy::from_contract(&contract));
    // 骨架阶段先证明策略可构造；接入 HTTP 后由服务层持有该 store。
    std::hint::black_box(&store);
    Ok(())
}

/// 解析 `--contract`；缺省为仓库内契约路径。
///
/// # 参数
/// - `args`: 不含程序名的参数列表。
///
/// # 错误
/// 未知选项或选项缺少取值时返回 `E_USAGE`（退出码 2）。
fn parse_contract_path(args: &[String]) -> Result<PathBuf, MeitoError> {
    let mut path = PathBuf::from(LINK_CONTRACT_PATH);
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--contract" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| MeitoError::new(ErrorCode::Usage, "--contract 缺少取值"))?;
                path = PathBuf::from(value);
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
    Ok(path)
}
