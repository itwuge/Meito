//! File: 入口端口与回环绑定契约（contracts/entry.json）。
//!
//! 端口、回环地址与图形帧缓冲只从契约读取；代码不得另写数值。契约缺失或非法属于配置
//! 错误，统一返回 `E_CONFIG`。

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::error::{ErrorCode, MeitoError};

// ===== 契约常量 =====

/// 入口契约的仓库相对路径。
pub const ENTRY_CONTRACT_PATH: &str = "contracts/entry.json";

/// 唯一允许的回环绑定地址。
const LOOPBACK_ADDRESS: &str = "127.0.0.1";

// ===== 数据结构 =====

/// 入口监听端口；三个端口都必须只绑回环。
///
/// `graphical` 与 `vnc` 是契约 0.3.3 起的 `reserved` 占位：v1 只加载数值，不启动图形链路，
/// 也不得因状态为 `reserved` 就删除字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryPorts {
    /// ttyd 终端入口端口。
    pub terminal: u16,
    /// noVNC/websockify 图形入口端口；v1 预留，不监听。
    pub graphical: u16,
    /// x11vnc 的 VNC 端口；v1 预留，不监听。
    pub vnc: u16,
}

// ===== 加载 =====

/// 读取并校验入口契约，返回三个监听端口。
///
/// # 参数
/// - `root`: 仓库根目录，契约必须位于 `root/contracts/entry.json`。
///
/// # 错误
/// 文件缺失、JSON 非法、端口越界，或绑定不是回环时返回 `E_CONFIG`（退出码 2）。
pub fn load_entry_ports(root: &Path) -> Result<EntryPorts, MeitoError> {
    let path = root.join(ENTRY_CONTRACT_PATH);
    let text = fs::read_to_string(&path).map_err(|error| {
        MeitoError::io(
            ErrorCode::Config,
            format!("读取入口契约 {}", path.display()),
            &error,
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        MeitoError::new(
            ErrorCode::Config,
            format!("解析入口契约 {} 失败：{error}", path.display()),
        )
    })?;
    parse_entry_ports(&value, &path.display().to_string())
}

/// 校验绑定属性与端口范围并构造结果。
///
/// # 参数
/// - `value`: 已解析的契约 JSON。
/// - `origin`: 出错时用于定位的文件名。
///
/// # 错误
/// 绑定非回环、允许通配绑定，或缺少 `ports` 时返回 `E_CONFIG`。
fn parse_entry_ports(value: &Value, origin: &str) -> Result<EntryPorts, MeitoError> {
    let bind = value
        .get("bind")
        .ok_or_else(|| config_error(origin, "缺少 bind 对象"))?;
    let address = bind.get("address").and_then(Value::as_str).unwrap_or("");
    if address != LOOPBACK_ADDRESS {
        return Err(config_error(
            origin,
            format!("bind.address 必须为 {LOOPBACK_ADDRESS}"),
        ));
    }
    if bind.get("loopbackOnly").and_then(Value::as_bool) != Some(true) {
        return Err(config_error(origin, "bind.loopbackOnly 必须为 true"));
    }
    if bind.get("allowWildcardBind").and_then(Value::as_bool) != Some(false) {
        return Err(config_error(origin, "bind.allowWildcardBind 必须为 false"));
    }

    let ports = value
        .get("ports")
        .ok_or_else(|| config_error(origin, "缺少 ports 对象"))?;
    Ok(EntryPorts {
        terminal: port(ports, "terminal", origin)?,
        graphical: port(ports, "graphical", origin)?,
        vnc: port(ports, "vnc", origin)?,
    })
}

/// 读取单个端口并校验取值范围。
///
/// # 参数
/// - `ports`: `ports` 对象。
/// - `key`: 端口名，例如 `terminal`。
/// - `origin`: 出错时用于定位的文件名。
///
/// # 错误
/// 字段缺失、不是整数或不在 `1..=65535` 时返回 `E_CONFIG`。
fn port(ports: &Value, key: &str, origin: &str) -> Result<u16, MeitoError> {
    let number = ports
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| config_error(origin, format!("ports.{key} 必须是整数")))?;
    if !(1..=65535).contains(&number) {
        return Err(config_error(
            origin,
            format!("ports.{key} 必须在 1..=65535，实际 {number}"),
        ));
    }
    Ok(number as u16)
}

/// 构造带文件定位的配置错误。
///
/// # 参数
/// - `origin`: 出错文件。
/// - `message`: 具体原因。
///
/// # 错误
/// 不返回错误；返回已构造的 `E_CONFIG`。
fn config_error(origin: &str, message: impl std::fmt::Display) -> MeitoError {
    MeitoError::new(ErrorCode::Config, format!("入口契约 {origin} {message}"))
}
