//! File: `host:port` 目标的解析与校验。
//!
//! 契约 `contracts/udp-probe.json` 与 `contracts/meito-config.schema.json` 都要求目标形如
//! `host:port`，且 host 只含 `A-Za-z0-9_.-`、端口为 1–5 位数字。集中在这里校验，避免
//! 配置加载与命令行覆盖各写一套规则。

use std::fmt;

use meito_core::{ErrorCode, MeitoError};

// ===== 目标 =====

/// 已校验的 `host:port` 目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPort {
    host: String,
    port: u16,
}

impl HostPort {
    /// 解析并校验 `host:port`。
    ///
    /// # 参数
    /// - `value`: 形如 `example.test:9000` 的字符串。
    ///
    /// # 错误
    /// 形状不符、host 含非法字符或端口为零/越界时返回 `E_CONFIG`（退出码 2）。
    pub fn parse(value: &str) -> Result<Self, MeitoError> {
        let (host, port_text) = value.rsplit_once(':').ok_or_else(|| {
            MeitoError::new(
                ErrorCode::Config,
                format!("UDP 目标必须是 host:port，收到 {value}"),
            )
        })?;
        if host.is_empty()
            || !host.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
            })
        {
            return Err(MeitoError::new(
                ErrorCode::Config,
                format!("UDP 目标 host 非法：{value}"),
            ));
        }
        if port_text.is_empty()
            || port_text.len() > 5
            || !port_text.chars().all(|c| c.is_ascii_digit())
        {
            return Err(MeitoError::new(
                ErrorCode::Config,
                format!("UDP 目标端口非法：{value}"),
            ));
        }
        let port: u16 = port_text.parse().map_err(|error| {
            MeitoError::new(
                ErrorCode::Config,
                format!("UDP 目标端口越界：{value}（{error}）"),
            )
        })?;
        if port == 0 {
            return Err(MeitoError::new(
                ErrorCode::Config,
                format!("UDP 目标端口不得为 0：{value}"),
            ));
        }
        Ok(Self {
            host: host.to_string(),
            port,
        })
    }

    /// 返回主机名或 IP。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn host(&self) -> &str {
        &self.host
    }

    /// 返回端口。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.port)
    }
}

// ===== 单元测试 =====

#[cfg(test)]
mod tests {
    use super::HostPort;
    use meito_core::ErrorCode;

    /// 合法 `host:port` 应保留 host 与端口。
    #[test]
    fn parses_valid_target() {
        let target = HostPort::parse("example.test:9000").expect("应解析成功");
        assert_eq!(target.host(), "example.test");
        assert_eq!(target.port(), 9000);
        assert_eq!(target.to_string(), "example.test:9000");
    }

    /// 契约 pattern 之外的形状必须走 `E_CONFIG`。
    #[test]
    fn rejects_invalid_targets() {
        for value in [
            "example.test",
            ":9000",
            "exa mple:9000",
            "h:0",
            "h:65536",
            "h:9a",
            "h:",
        ] {
            let error = HostPort::parse(value).expect_err("应拒绝");
            assert_eq!(error.code(), ErrorCode::Config, "输入 {value}");
        }
    }
}
