//! File: meito-link 公共接口（M3 协调服务骨架）。
//!
//! 本 crate 只负责通道注册与密文短暂交换；它不解析、不记录 blob，也不承载会话字节。
//! 所有数值（TTL、大小上限、读取次数、限流、状态码）都必须来自 `contracts/link.json`，
//! 契约缺失或字段不全时一律 `E_CONFIG`，不得用默认值兜底。

// ===== 模块 =====

mod access_log;
mod auth;
mod contract;
mod rate_limit;
mod router;
mod server;
mod store;

pub use access_log::{append_access_log, record_to_value, AccessLogRecord, ACCESS_LOG_FILENAME};
pub use auth::{constant_time_eq, is_sha256_hex, token_hash, verify_token};
pub use contract::{
    load_link_contract, ChannelIdPolicy, GetSemanticsPolicy, HttpStatusPolicy, LinkContract,
    LoggingPolicy, PackageLimitPolicy, RateLimitPolicy, RegisterPayloadPolicy, TokenHashPolicy,
    TokenIssuancePolicy, LINK_CONTRACT_PATH,
};
pub use rate_limit::RateLimiter;
pub use router::{is_valid_channel_id, route, Route};
pub use server::LinkServer;
pub use store::{
    BlobRead, ChannelCredentials, ChannelPolicy, ChannelStore, InMemoryChannelStore, Registration,
    StoreError,
};
