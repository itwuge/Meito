//! 限流用例：突发容量、每分钟补桶与 IP/通道预算互不干扰。
//!
//! 三个数值全部从 `contracts/link.json` 读取，时间由用例合成，不依赖真实等待。

use std::path::PathBuf;
use std::time::Duration;

use meito_link::{load_link_contract, RateLimiter};

fn contract() -> meito_link::LinkContract {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/link.json");
    load_link_contract(&path).unwrap()
}

#[test]
fn channel_burst_is_consumed_then_refilled() {
    let policy = contract().rate_limit;
    let limiter = RateLimiter::from_contract(&policy);
    for _ in 0..policy.burst {
        assert!(
            limiter.allow_channel("channel-a", Duration::ZERO),
            "突发容量内必须放行"
        );
    }
    assert!(
        !limiter.allow_channel("channel-a", Duration::ZERO),
        "突发容量耗尽后必须拒绝"
    );
    let one_minute = Duration::from_secs(60);
    assert!(
        limiter.allow_channel("channel-a", one_minute),
        "每分钟配额必须按契约补回"
    );
}

#[test]
fn ip_and_channel_budgets_are_independent() {
    let policy = contract().rate_limit;
    let limiter = RateLimiter::from_contract(&policy);
    for _ in 0..policy.burst {
        assert!(limiter.allow_ip("10.0.0.1", Duration::ZERO));
    }
    assert!(!limiter.allow_ip("10.0.0.1", Duration::ZERO));
    assert!(
        limiter.allow_ip("10.0.0.2", Duration::ZERO),
        "不同来源 IP 不得共享预算"
    );
    assert!(
        limiter.allow_channel("channel-b", Duration::ZERO),
        "通道预算不得被 IP 消耗影响"
    );
}
