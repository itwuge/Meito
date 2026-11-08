//! File: 协调服务的令牌桶限流。
//!
//! 三个数值（每通道每分钟、每来源 IP 每分钟、突发容量）全部由 `contracts/link.json` 注入，
//! 本模块不内置任何默认值。当前时刻由调用方传入，用例可用合成时间验证边界而不依赖睡眠。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::contract::RateLimitPolicy;

// ===== 令牌桶 =====

/// 单个限流键的桶状态。
#[derive(Debug)]
struct Bucket {
    tokens: f64,
    updated: Duration,
}

/// 按通道与来源 IP 分别限流的令牌桶集合。
#[derive(Debug)]
pub struct RateLimiter {
    channel_per_second: f64,
    ip_per_second: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// 从已校验的契约构造限流器。
    ///
    /// # 参数
    /// - `policy`: 契约中的 `rateLimit` 段。
    ///
    /// # 错误
    /// 不返回错误；契约加载阶段已保证三个数值大于 0。
    pub fn from_contract(policy: &RateLimitPolicy) -> Self {
        Self {
            channel_per_second: policy.per_channel_per_minute as f64 / 60.0,
            ip_per_second: policy.per_ip_per_minute as f64 / 60.0,
            burst: policy.burst as f64,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// 判断来源 IP 是否还有配额；通过则扣减一个令牌。
    ///
    /// # 参数
    /// - `ip`: 来源地址字符串。
    /// - `now`: 自服务启动以来的时长，由调用方注入。
    ///
    /// # 错误
    /// 不返回错误；无配额返回 `false`。
    pub fn allow_ip(&self, ip: &str, now: Duration) -> bool {
        self.allow(&format!("ip:{ip}"), self.ip_per_second, now)
    }

    /// 判断通道是否还有配额；通过则扣减一个令牌。
    ///
    /// # 参数
    /// - `channel_id`: 不透明通道 id。
    /// - `now`: 自服务启动以来的时长，由调用方注入。
    ///
    /// # 错误
    /// 不返回错误；无配额返回 `false`。
    pub fn allow_channel(&self, channel_id: &str, now: Duration) -> bool {
        self.allow(
            &format!("channel:{channel_id}"),
            self.channel_per_second,
            now,
        )
    }

    /// 对指定键执行一次令牌扣减。
    ///
    /// # 参数
    /// - `key`: 限流键，调用方保证 IP 与通道前缀不冲突。
    /// - `rate_per_second`: 每秒补充的令牌数。
    /// - `now`: 自服务启动以来的时长。
    ///
    /// # 错误
    /// 不返回错误；无配额返回 `false`。
    fn allow(&self, key: &str, rate_per_second: f64, now: Duration) -> bool {
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let capacity = self.burst;
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: capacity,
            updated: now,
        });
        let elapsed = now.saturating_sub(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate_per_second).min(capacity);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
