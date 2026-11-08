//! File: `env` 子命令的内核实现。
//!
//! 环境台账字段顺序与 `contracts/log.json` 的 `envLedgerFields` 一致；
//! 本地没有台账文件时按当前进程信息构造，未知字段写空串或显式“未探测”，不编造版本号。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{ErrorCode, MeitoError};
use crate::log::{env_ledger_path, is_uuid_v4};

// ===== 台账模型 =====

/// 环境台账；所有字段均为字符串以匹配契约 JSON 形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentLedger {
    pub session_id: String,
    pub run_id: String,
    pub image: String,
    pub runner_label: String,
    pub os: String,
    pub meito_version: String,
    pub frpc_version: String,
    pub ttyd_version: String,
    pub x11vnc_version: String,
    pub websockify_version: String,
    pub p2p_provider: String,
    pub p2p_state: String,
    pub udp_probe: String,
    pub ice_candidates: String,
    pub entry_ports: String,
    pub exit_code: String,
    pub error_code: String,
}

impl EnvironmentLedger {
    /// 按当前进程构造台账。
    ///
    /// 未探测的字段写显式标记；不读取任何秘密值，也不推断本机未安装的版本。
    ///
    /// # 参数
    /// - `session_id`: 会话 id；无会话时允许空串。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn from_process(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            run_id: String::new(),
            image: String::new(),
            runner_label: String::new(),
            os: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
            meito_version: env!("CARGO_PKG_VERSION").to_string(),
            frpc_version: String::new(),
            ttyd_version: String::new(),
            x11vnc_version: String::new(),
            websockify_version: String::new(),
            p2p_provider: String::new(),
            p2p_state: "not_started".to_string(),
            udp_probe: "not_probed".to_string(),
            ice_candidates: "none".to_string(),
            entry_ports: String::new(),
            exit_code: "0".to_string(),
            error_code: String::new(),
        }
    }

    /// 按契约顺序返回字段名与值，供文本渲染。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn entries(&self) -> [(&'static str, &str); 17] {
        [
            ("session_id", self.session_id.as_str()),
            ("run_id", self.run_id.as_str()),
            ("image", self.image.as_str()),
            ("runner_label", self.runner_label.as_str()),
            ("os", self.os.as_str()),
            ("meito_version", self.meito_version.as_str()),
            ("frpc_version", self.frpc_version.as_str()),
            ("ttyd_version", self.ttyd_version.as_str()),
            ("x11vnc_version", self.x11vnc_version.as_str()),
            ("websockify_version", self.websockify_version.as_str()),
            ("p2p_provider", self.p2p_provider.as_str()),
            ("p2p_state", self.p2p_state.as_str()),
            ("udp_probe", self.udp_probe.as_str()),
            ("ice_candidates", self.ice_candidates.as_str()),
            ("entry_ports", self.entry_ports.as_str()),
            ("exit_code", self.exit_code.as_str()),
            ("error_code", self.error_code.as_str()),
        ]
    }

    /// 输出机读台账。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        for (key, value) in self.entries() {
            map.insert(key.to_string(), Value::String(value.to_string()));
        }
        Value::Object(map)
    }
}

// ===== 加载逻辑 =====

/// 加载指定会话或最近的环境台账；没有台账时按当前进程构造。
///
/// # 参数
/// - `root`: 工作目录。
/// - `session`: 显式 UUID 或 `latest`/`None`。
///
/// # 错误
/// 显式会话 id 非法返回 `E_USAGE`；台账文件损坏或字段类型错误返回 `E_INTERNAL`。
pub fn load_ledger(root: &Path, session: Option<&str>) -> Result<EnvironmentLedger, MeitoError> {
    match session {
        Some(value) if value != "latest" => {
            if !is_uuid_v4(value) {
                return Err(MeitoError::new(
                    ErrorCode::Usage,
                    format!("--session 必须是 UUID v4 或 latest，收到 {value}"),
                ));
            }
            let path = env_ledger_path(root, value);
            if path.is_file() {
                read_ledger(&path)
            } else {
                Ok(EnvironmentLedger::from_process(value))
            }
        }
        _ => match latest_ledger_file(root)? {
            Some(path) => read_ledger(&path),
            None => Ok(EnvironmentLedger::from_process("")),
        },
    }
}

/// 找到最近修改的 `logs/env-session-*.json`。
///
/// # 参数
/// - `root`: 工作目录。
///
/// # 错误
/// 日志目录或文件元数据读取失败时返回 `E_INTERNAL`。
fn latest_ledger_file(root: &Path) -> Result<Option<PathBuf>, MeitoError> {
    let directory = root.join("logs");
    if !directory.is_dir() {
        return Ok(None);
    }
    let entries = fs::read_dir(&directory).map_err(|error| {
        MeitoError::io(
            ErrorCode::Internal,
            format!("读取日志目录 {}", directory.display()),
            &error,
        )
    })?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(|error| {
            MeitoError::io(
                ErrorCode::Internal,
                format!("遍历日志目录 {}", directory.display()),
                &error,
            )
        })?;
        let path = entry.path();
        let file_name = match path.file_name().and_then(|value| value.to_str()) {
            Some(value) => value,
            None => continue,
        };
        if !file_name.starts_with("env-session-") || !file_name.ends_with(".json") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|error| {
                MeitoError::io(
                    ErrorCode::Internal,
                    format!("读取台账文件元数据 {}", path.display()),
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
    Ok(newest.map(|(_, path)| path))
}

/// 读取并校验一份台账文件。
///
/// # 参数
/// - `path`: 台账路径。
///
/// # 错误
/// 读取失败、JSON 非法、会话 id 非法或字段类型错误时返回 `E_INTERNAL`。
fn read_ledger(path: &Path) -> Result<EnvironmentLedger, MeitoError> {
    let text = fs::read_to_string(path).map_err(|error| {
        MeitoError::io(
            ErrorCode::Internal,
            format!("读取环境台账 {}", path.display()),
            &error,
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        MeitoError::new(
            ErrorCode::Internal,
            format!("环境台账 {} 不是合法 JSON：{}", path.display(), error),
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Internal,
            format!("环境台账 {} 根节点必须是对象", path.display()),
        )
    })?;
    let session_id = read_field(object, "session_id", path)?;
    if !session_id.is_empty() && !is_uuid_v4(&session_id) {
        return Err(MeitoError::new(
            ErrorCode::Internal,
            format!("环境台账 {} 的 session_id 不是 UUID v4", path.display()),
        ));
    }
    Ok(EnvironmentLedger {
        session_id,
        run_id: read_field(object, "run_id", path)?,
        image: read_field(object, "image", path)?,
        runner_label: read_field(object, "runner_label", path)?,
        os: read_field(object, "os", path)?,
        meito_version: read_field(object, "meito_version", path)?,
        frpc_version: read_field(object, "frpc_version", path)?,
        ttyd_version: read_field(object, "ttyd_version", path)?,
        x11vnc_version: read_field(object, "x11vnc_version", path)?,
        websockify_version: read_field(object, "websockify_version", path)?,
        p2p_provider: read_field(object, "p2p_provider", path)?,
        p2p_state: read_field(object, "p2p_state", path)?,
        udp_probe: read_field(object, "udp_probe", path)?,
        ice_candidates: read_field(object, "ice_candidates", path)?,
        entry_ports: read_field(object, "entry_ports", path)?,
        exit_code: read_field(object, "exit_code", path)?,
        error_code: read_field(object, "error_code", path)?,
    })
}

/// 读取一个台账字段；缺失/null 视为空串，数字与布尔转为字符串。
///
/// # 参数
/// - `object`: 台账对象。
/// - `key`: 字段名。
/// - `path`: 文件路径，仅用于错误信息。
///
/// # 错误
/// 字段类型既不是字符串也不是数字/布尔时返回 `E_INTERNAL`。
fn read_field(object: &Map<String, Value>, key: &str, path: &Path) -> Result<String, MeitoError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Number(value)) => Ok(value.to_string()),
        Some(Value::Bool(value)) => Ok(value.to_string()),
        Some(_) => Err(MeitoError::new(
            ErrorCode::Internal,
            format!("环境台账 {} 字段 {key} 必须是标量", path.display()),
        )),
    }
}
