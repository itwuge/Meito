//! File: 仓库根 `meito.toml` 的加载与校验。
//!
//! 契约 `contracts/meito-config.schema.json` 要求 `[security] allowed_actors` 与可选的
//! `[network] udp_probe_target`，且未知字段一律拒绝。缺失文件不是错误：调用方必须按
//! fail-closed 处理，把“没有白名单”当成“全部拒绝”，而不是放行。

use std::fs;
use std::path::Path;

use meito_core::{ErrorCode, MeitoError};

use crate::host_port::HostPort;

// ===== 配置常量 =====

/// 仓库根配置文件名称。
pub const CONFIG_FILE_NAME: &str = "meito.toml";

// ===== 配置值 =====

/// 已通过结构校验的仓库配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoConfig {
    allowed_actors: Vec<String>,
    udp_probe_target: Option<HostPort>,
}

impl RepoConfig {
    /// 返回 actor 白名单；缺省为空，调用方必须按 fail-closed 处理。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn allowed_actors(&self) -> &[String] {
        &self.allowed_actors
    }

    /// 返回 UDP 探测目标；未配置时为 `None`。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn udp_probe_target(&self) -> Option<&HostPort> {
        self.udp_probe_target.as_ref()
    }
}

/// 配置加载结果；显式区分“文件缺失”，避免与“空配置”混淆。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLoad {
    /// 仓库根没有 `meito.toml`；调用方应全部拒绝。
    Missing,
    /// 已加载并通过校验。
    Loaded(RepoConfig),
}

// ===== 加载 =====

/// 从显式路径读取 `meito.toml`；供默认仓库根与 `--config` 覆盖共用。
///
/// # 参数
/// - `path`: 配置文件路径。
///
/// # 错误
/// 文件存在但 TOML 非法、含未知字段、`allowed_actors` 不是非空字符串数组，或
/// `udp_probe_target` 不是 `host:port` 时返回 `E_CONFIG`（退出码 2）。
pub fn load_repo_config_file(path: &Path) -> Result<ConfigLoad, MeitoError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        // 文件缺失属于 fail-closed 输入，不是配置语法错误。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigLoad::Missing);
        }
        Err(error) => {
            return Err(MeitoError::io(
                ErrorCode::Config,
                format!("读取配置 {}", path.display()),
                &error,
            ));
        }
    };

    let value: toml::Value = text.parse().map_err(|error| {
        MeitoError::new(
            ErrorCode::Config,
            format!("解析配置 {} 失败：{error}", path.display()),
        )
    })?;
    let table = value.as_table().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("配置 {} 顶层必须是表", path.display()),
        )
    })?;

    reject_unknown_keys(table, &["security", "network"], "顶层", path)?;

    let allowed_actors = load_allowed_actors(table.get("security"), path)?;
    let udp_probe_target = load_udp_target(table.get("network"), path)?;

    Ok(ConfigLoad::Loaded(RepoConfig {
        allowed_actors,
        udp_probe_target,
    }))
}

/// 校验并读取 `[security] allowed_actors`。
///
/// # 参数
/// - `security`: 可选的安全表。
/// - `path`: 配置文件路径，用于错误上下文。
///
/// # 错误
/// 表类型错误、未知字段或元素为空字符串时返回 `E_CONFIG`。
fn load_allowed_actors(
    security: Option<&toml::Value>,
    path: &Path,
) -> Result<Vec<String>, MeitoError> {
    let Some(security) = security else {
        // 缺 security 等同于缺白名单：返回空列表，由调用方拒绝。
        return Ok(Vec::new());
    };
    let table = security.as_table().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("配置 {} 的 security 必须是表", path.display()),
        )
    })?;
    reject_unknown_keys(table, &["allowed_actors"], "security", path)?;

    let Some(actors) = table.get("allowed_actors") else {
        return Ok(Vec::new());
    };
    let array = actors.as_array().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("配置 {} 的 allowed_actors 必须是数组", path.display()),
        )
    })?;

    let mut result = Vec::with_capacity(array.len());
    for entry in array {
        let actor = entry.as_str().ok_or_else(|| {
            MeitoError::new(
                ErrorCode::Config,
                format!("配置 {} 的 allowed_actors 元素必须是字符串", path.display()),
            )
        })?;
        if actor.is_empty() {
            return Err(MeitoError::new(
                ErrorCode::Config,
                format!("配置 {} 的 allowed_actors 元素不得为空", path.display()),
            ));
        }
        result.push(actor.to_string());
    }
    Ok(result)
}

/// 校验并读取 `[network] udp_probe_target`。
///
/// # 参数
/// - `network`: 可选的网络表。
/// - `path`: 配置文件路径，用于错误上下文。
///
/// # 错误
/// 表类型错误、未知字段或目标不是 `host:port` 时返回 `E_CONFIG`。
fn load_udp_target(
    network: Option<&toml::Value>,
    path: &Path,
) -> Result<Option<HostPort>, MeitoError> {
    let Some(network) = network else {
        return Ok(None);
    };
    let table = network.as_table().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("配置 {} 的 network 必须是表", path.display()),
        )
    })?;
    reject_unknown_keys(table, &["udp_probe_target"], "network", path)?;

    let Some(target) = table.get("udp_probe_target") else {
        return Ok(None);
    };
    let text = target.as_str().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("配置 {} 的 udp_probe_target 必须是字符串", path.display()),
        )
    })?;
    HostPort::parse(text).map(Some)
}

/// 拒绝 schema 未声明的字段。
///
/// # 参数
/// - `table`: 待检查的 TOML 表。
/// - `allowed`: 该层允许出现的键。
/// - `section`: 出错时展示的层级名。
/// - `path`: 配置文件路径，用于错误上下文。
///
/// # 错误
/// 出现未知键时返回 `E_CONFIG`。
fn reject_unknown_keys(
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
    section: &str,
    path: &Path,
) -> Result<(), MeitoError> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(MeitoError::new(
                ErrorCode::Config,
                format!("配置 {} 的 {section} 含未知字段 {key}", path.display()),
            ));
        }
    }
    Ok(())
}

// ===== 单元测试 =====

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{load_repo_config_file, ConfigLoad};
    use meito_core::ErrorCode;

    fn temp_config(text: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("meito-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("创建临时目录");
        let path = directory.join("meito.toml");
        fs::write(&path, text).expect("写入配置");
        path
    }

    /// 合法配置应读出白名单与 UDP 目标。
    #[test]
    fn loads_valid_config() {
        let path = temp_config(
            "[security]\nallowed_actors = [\"alice\"]\n[network]\nudp_probe_target = \"example.test:9000\"\n",
        );
        let loaded = load_repo_config_file(&path).expect("应加载成功");
        match loaded {
            ConfigLoad::Loaded(config) => {
                assert_eq!(config.allowed_actors().len(), 1);
                assert_eq!(config.allowed_actors()[0], "alice");
                assert_eq!(
                    config.udp_probe_target().map(|target| target.to_string()),
                    Some("example.test:9000".to_string())
                );
            }
            ConfigLoad::Missing => panic!("不应是缺失"),
        }
        drop(fs::remove_dir_all(path.parent().expect("有父目录")));
    }

    /// 文件缺失必须返回 `Missing`，而不是错误或空配置放行。
    #[test]
    fn reports_missing_config() {
        let directory = std::env::temp_dir().join(format!("meito-config-{}", uuid::Uuid::new_v4()));
        let path = directory.join("meito.toml");
        assert_eq!(
            load_repo_config_file(&path).expect("缺失不是错误"),
            ConfigLoad::Missing
        );
    }

    /// 未知字段、错误类型、空元素与坏目标都必须走 `E_CONFIG`。
    #[test]
    fn rejects_invalid_configs() {
        let cases = [
            "[security]\nallowed_actors = [\"alice\"]\nextra = true\n",
            "[security]\nallowed_actors = \"alice\"\n",
            "[security]\nallowed_actors = [\"\"]\n",
            "[security]\nallowed_actors = [\"alice\"]\n[network]\nudp_probe_target = \"no-port\"\n",
        ];
        for text in cases {
            let path = temp_config(text);
            let error = load_repo_config_file(&path).expect_err("应拒绝");
            assert_eq!(error.code(), ErrorCode::Config, "配置：{text}");
            drop(fs::remove_dir_all(path.parent().expect("有父目录")));
        }
    }
}
