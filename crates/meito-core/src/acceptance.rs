//! File: 验收契约中的轻量性预算（contracts/acceptance.json）。
//!
//! 只读取已定阈值的三个轻量项；`pending-calibration` 项不参与加载，也不在本模块写任何
//! 猜测值。阈值缺失或类型错误统一返回 `E_CONFIG`。

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::error::{ErrorCode, MeitoError};

// ===== 契约常量 =====

/// 验收契约的仓库相对路径。
pub const ACCEPTANCE_CONTRACT_PATH: &str = "contracts/acceptance.json";

// ===== 数据结构 =====

/// v1 已定轻量预算；三个阈值全部来自 `lightweight.metrics`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightweightBudget {
    /// runner 常驻进程数上限。
    pub runner_resident_process_count_max: u64,
    /// 单个 job 新增安装包数上限。
    pub runner_new_package_count_max: u64,
    /// v1 图形组件数上限。
    pub graphical_component_count_max: u64,
}

// ===== 加载 =====

/// 从仓库加载并校验已定轻量预算。
///
/// # 参数
/// - `root`: 仓库根目录，契约必须位于 `root/contracts/acceptance.json`。
///
/// # 错误
/// 文件缺失、JSON 非法、`lightweight` 缺失或三个已定阈值缺失/非整数时返回 `E_CONFIG`
/// （退出码 2）。
pub fn load_lightweight_budget(root: &Path) -> Result<LightweightBudget, MeitoError> {
    let path = root.join(ACCEPTANCE_CONTRACT_PATH);
    let text = fs::read_to_string(&path).map_err(|error| {
        MeitoError::io(
            ErrorCode::Config,
            format!("读取验收契约 {}", path.display()),
            &error,
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        MeitoError::new(
            ErrorCode::Config,
            format!("解析验收契约 {} 失败：{error}", path.display()),
        )
    })?;
    parse_lightweight_budget(&value, &path)
}

/// 读取 `lightweight.metrics` 中三个已定阈值。
///
/// # 参数
/// - `value`: 已解析的验收契约 JSON。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// `lightweight` 缺失、状态不是 `required`、或任一已定阈值缺失/非整数时返回 `E_CONFIG`。
fn parse_lightweight_budget(value: &Value, path: &Path) -> Result<LightweightBudget, MeitoError> {
    let lightweight = value
        .get("lightweight")
        .and_then(Value::as_object)
        .ok_or_else(|| config_error(path, "缺少 lightweight 对象"))?;
    let status = lightweight
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| config_error(path, "lightweight.status 必须是字符串"))?;
    if status != "required" {
        return Err(config_error(
            path,
            format!("lightweight.status 只接受 required，收到 {status}"),
        ));
    }
    let metrics = lightweight
        .get("metrics")
        .and_then(Value::as_object)
        .ok_or_else(|| config_error(path, "缺少 lightweight.metrics 对象"))?;
    Ok(LightweightBudget {
        runner_resident_process_count_max: metric(metrics, "runnerResidentProcessCountMax", path)?,
        runner_new_package_count_max: metric(metrics, "runnerNewPackageCountMax", path)?,
        graphical_component_count_max: metric(metrics, "graphicalComponentCountMax", path)?,
    })
}

/// 读取单个已定阈值。
///
/// # 参数
/// - `metrics`: `lightweight.metrics` 对象。
/// - `key`: 阈值名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 字段缺失或不是非负整数时返回 `E_CONFIG`。
fn metric(metrics: &Map<String, Value>, key: &str, path: &Path) -> Result<u64, MeitoError> {
    metrics
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| config_error(path, format!("lightweight.metrics.{key} 必须是非负整数")))
}

/// 构造带文件定位的配置错误。
///
/// # 参数
/// - `path`: 契约路径。
/// - `message`: 具体原因。
///
/// # 错误
/// 不返回错误；返回已构造的 `E_CONFIG`。
fn config_error(path: &Path, message: impl std::fmt::Display) -> MeitoError {
    MeitoError::new(
        ErrorCode::Config,
        format!("验收契约 {} {message}", path.display()),
    )
}
