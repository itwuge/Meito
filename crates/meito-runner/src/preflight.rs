//! File: runner 预检结构：出站 UDP 探测、DNS 与磁盘可写探针。
//!
//! 契约 `contracts/udp-probe.json` 固定报文与重试：`MEITOUDP` + 8 字节 nonce，
//! 3 次尝试、每次 1000ms。三种 UDP 失败原因必须各自可区分：
//! 目标未配置（`E_CONFIG`）、DNS 解析失败（`E_PREFLIGHT`）、超时未回执（`E_UDP_BLOCKED`）。

use std::fs;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant};

use meito_core::{ErrorCode, MeitoError};
use uuid::Uuid;

use crate::host_port::HostPort;

// ===== 契约常量 =====

/// 探测报文 magic，必须 8 字节。
pub const UDP_MAGIC: &[u8; 8] = b"MEITOUDP";

/// 契约允许的尝试次数。
pub const PROBE_ATTEMPTS: u32 = 3;

/// 每次尝试的等待上限。
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1000);

/// 接收缓冲区；回执必须与请求等长。
const RECEIVE_BUFFER_BYTES: usize = 64;

// ===== 结果类型 =====

/// UDP 探测结果；失败原因互斥，便于日志直接区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpOutcome {
    /// 收到同 nonce 回执。
    Ok,
    /// 目标未配置。
    TargetNotConfigured,
    /// 目标 host 无法解析。
    DnsResolutionFailed,
    /// 尝试耗尽仍未收到正确回执。
    Timeout,
}

impl UdpOutcome {
    /// 返回日志中的稳定字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::TargetNotConfigured => "target_not_configured",
            Self::DnsResolutionFailed => "dns_resolution_failed",
            Self::Timeout => "timeout",
        }
    }

    /// 返回该结果对应的错误码；成功时为 `None`。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn error_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Ok => None,
            Self::TargetNotConfigured => Some(ErrorCode::Config),
            Self::DnsResolutionFailed => Some(ErrorCode::Preflight),
            Self::Timeout => Some(ErrorCode::UdpBlocked),
        }
    }
}

/// UDP 探测报告。
#[derive(Debug, Clone)]
pub struct UdpProbeReport {
    /// 目标文本；未配置时为 `None`。
    pub target: Option<String>,
    /// 结果。
    pub outcome: UdpOutcome,
    /// 实际尝试次数。
    pub attempts_used: u32,
    /// 总耗时（毫秒）。
    pub duration_ms: u64,
    /// 失败细节；成功或未配置时可能为 `None`。
    pub detail: Option<String>,
}

impl UdpProbeReport {
    /// 把失败结果转成错误；成功返回 `None`。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 失败时返回对应错误码，调用方应把退出码透传给脚本。
    pub fn error(&self) -> Option<MeitoError> {
        let code = self.outcome.error_code()?;
        let detail = self.detail.as_deref().unwrap_or("无细节");
        let message = match self.outcome {
            UdpOutcome::TargetNotConfigured => "UDP 探测目标未配置".to_string(),
            UdpOutcome::DnsResolutionFailed => format!("UDP 目标 DNS 解析失败：{detail}"),
            UdpOutcome::Timeout => {
                format!("UDP 探测 {} 次均未收到回执：{detail}", self.attempts_used)
            }
            UdpOutcome::Ok => unreachable!("Ok 不产生错误"),
        };
        Some(MeitoError::new(code, message))
    }
}

/// 单步结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    /// 通过。
    Ok,
    /// 未启用（例如没有提供 DNS host）。
    Skipped,
    /// 失败。
    Failed,
}

impl StepOutcome {
    /// 返回日志中的稳定字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

/// DNS 或磁盘步骤报告。
#[derive(Debug, Clone)]
pub struct StepReport {
    /// 结果。
    pub outcome: StepOutcome,
    /// 耗时（毫秒）。
    pub duration_ms: u64,
    /// 细节；失败时必填。
    pub detail: Option<String>,
}

/// 完整预检报告；各步骤独立记录，便于逐条落日志。
#[derive(Debug, Clone)]
pub struct PreflightReport {
    /// UDP 探测。
    pub udp: UdpProbeReport,
    /// DNS 解析。
    pub dns: StepReport,
    /// 磁盘可写探针。
    pub disk: StepReport,
}

impl PreflightReport {
    /// 按 UDP → DNS → 磁盘的顺序返回第一个错误。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 任一失败时返回对应错误；全部通过返回 `None`。
    pub fn error(&self) -> Option<MeitoError> {
        if let Some(error) = self.udp.error() {
            return Some(error);
        }
        if self.dns.outcome == StepOutcome::Failed {
            return Some(MeitoError::new(
                ErrorCode::Preflight,
                format!(
                    "DNS 预检失败：{}",
                    self.dns.detail.as_deref().unwrap_or("无细节")
                ),
            ));
        }
        if self.disk.outcome == StepOutcome::Failed {
            return Some(MeitoError::new(
                ErrorCode::Preflight,
                format!(
                    "磁盘预检失败：{}",
                    self.disk.detail.as_deref().unwrap_or("无细节")
                ),
            ));
        }
        None
    }
}

// ===== UDP 探测 =====

/// 执行 UDP 可达性探测。
///
/// # 参数
/// - `target`: 已校验的目标；`None` 表示配置未提供。
/// - `attempts`: 尝试次数，调用方传 `PROBE_ATTEMPTS`。
/// - `per_attempt`: 每次等待上限，调用方传 `PROBE_TIMEOUT`。
///
/// # 错误
/// 不返回错误；失败原因由 `UdpProbeReport::outcome` 区分。
pub fn probe_udp(
    target: Option<&HostPort>,
    attempts: u32,
    per_attempt: Duration,
) -> UdpProbeReport {
    let started = Instant::now();
    let Some(target) = target else {
        return UdpProbeReport {
            target: None,
            outcome: UdpOutcome::TargetNotConfigured,
            attempts_used: 0,
            duration_ms: 0,
            detail: Some("meito.toml 未配置 network.udp_probe_target".to_string()),
        };
    };

    let resolved: Vec<SocketAddr> = match (target.host(), target.port()).to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(error) => {
            return UdpProbeReport {
                target: Some(target.to_string()),
                outcome: UdpOutcome::DnsResolutionFailed,
                attempts_used: 0,
                duration_ms: elapsed_ms(started),
                detail: Some(error.to_string()),
            };
        }
    };
    let Some(address) = resolved.first().copied() else {
        return UdpProbeReport {
            target: Some(target.to_string()),
            outcome: UdpOutcome::DnsResolutionFailed,
            attempts_used: 0,
            duration_ms: elapsed_ms(started),
            detail: Some("解析结果为空".to_string()),
        };
    };

    // 绑定地址必须与目标地址族一致，否则 Windows 会拒绝发送。
    let bind_address = match address {
        SocketAddr::V4(_) => "0.0.0.0:0",
        SocketAddr::V6(_) => "[::]:0",
    };
    let socket = match UdpSocket::bind(bind_address) {
        Ok(socket) => socket,
        Err(error) => {
            return UdpProbeReport {
                target: Some(target.to_string()),
                outcome: UdpOutcome::Timeout,
                attempts_used: 0,
                duration_ms: elapsed_ms(started),
                detail: Some(format!("绑定本地 UDP 套接字失败：{error}")),
            };
        }
    };
    if let Err(error) = socket.set_read_timeout(Some(per_attempt)) {
        return UdpProbeReport {
            target: Some(target.to_string()),
            outcome: UdpOutcome::Timeout,
            attempts_used: 0,
            duration_ms: elapsed_ms(started),
            detail: Some(format!("设置读取超时失败：{error}")),
        };
    }

    let mut last_detail = String::new();
    let mut attempts_used = 0;
    for attempt in 1..=attempts {
        attempts_used = attempt;
        let packet = build_probe_packet();
        if let Err(error) = socket.send_to(&packet, address) {
            last_detail = format!("发送失败：{error}");
            continue;
        }
        let mut buffer = [0u8; RECEIVE_BUFFER_BYTES];
        match socket.recv_from(&mut buffer) {
            // 回执必须与请求逐字节一致；端口复用或错误目标不算成功。
            Ok((size, _peer)) if size == packet.len() && buffer[..size] == packet => {
                return UdpProbeReport {
                    target: Some(target.to_string()),
                    outcome: UdpOutcome::Ok,
                    attempts_used,
                    duration_ms: elapsed_ms(started),
                    detail: None,
                };
            }
            Ok((size, peer)) => {
                last_detail = format!("收到不匹配回执：{size} 字节，来自 {peer}");
            }
            Err(error) => {
                last_detail = format!("等待回执失败：{error}");
            }
        }
    }

    UdpProbeReport {
        target: Some(target.to_string()),
        outcome: UdpOutcome::Timeout,
        attempts_used,
        duration_ms: elapsed_ms(started),
        detail: Some(last_detail),
    }
}

/// 构造 `MEITOUDP` + 8 字节随机 nonce。
///
/// # 参数
/// - 无。
///
/// # 错误
/// 不返回错误。
fn build_probe_packet() -> Vec<u8> {
    let mut packet = Vec::with_capacity(UDP_MAGIC.len() + 8);
    packet.extend_from_slice(UDP_MAGIC);
    packet.extend_from_slice(&Uuid::new_v4().as_bytes()[..8]);
    packet
}

// ===== DNS 与磁盘 =====

/// 解析 DNS 目标；未提供 host 时跳过。
///
/// # 参数
/// - `host`: 可选的 host 或 IP。
///
/// # 错误
/// 不返回错误；失败由 `StepReport::outcome` 表示。
pub fn check_dns(host: Option<&str>) -> StepReport {
    let started = Instant::now();
    let Some(host) = host.filter(|value| !value.trim().is_empty()) else {
        return StepReport {
            outcome: StepOutcome::Skipped,
            duration_ms: 0,
            detail: None,
        };
    };
    // 端口 0 仅用于触发解析，不会建立连接。
    match (host, 0u16).to_socket_addrs() {
        Ok(addresses) => {
            let mut addresses = addresses;
            if addresses.next().is_some() {
                StepReport {
                    outcome: StepOutcome::Ok,
                    duration_ms: elapsed_ms(started),
                    detail: None,
                }
            } else {
                StepReport {
                    outcome: StepOutcome::Failed,
                    duration_ms: elapsed_ms(started),
                    detail: Some(format!("{host} 解析结果为空")),
                }
            }
        }
        Err(error) => StepReport {
            outcome: StepOutcome::Failed,
            duration_ms: elapsed_ms(started),
            detail: Some(format!("{host} 解析失败：{error}")),
        },
    }
}

/// 在指定目录做一次写-删探针。
///
/// # 参数
/// - `path`: 必须是已存在的目录。
///
/// # 错误
/// 不返回错误；失败由 `StepReport::outcome` 表示。
pub fn probe_disk(path: &Path) -> StepReport {
    let started = Instant::now();
    let probe_name = format!(".meito-preflight-{}.tmp", Uuid::new_v4());
    let probe_path = path.join(probe_name);
    if let Err(error) = fs::write(&probe_path, b"meito-preflight") {
        return StepReport {
            outcome: StepOutcome::Failed,
            duration_ms: elapsed_ms(started),
            detail: Some(format!("写入 {} 失败：{error}", probe_path.display())),
        };
    }
    if let Err(error) = fs::remove_file(&probe_path) {
        return StepReport {
            outcome: StepOutcome::Failed,
            duration_ms: elapsed_ms(started),
            detail: Some(format!("删除 {} 失败：{error}", probe_path.display())),
        };
    }
    StepReport {
        outcome: StepOutcome::Ok,
        duration_ms: elapsed_ms(started),
        detail: None,
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
