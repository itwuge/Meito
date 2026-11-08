//! File: 协调服务的令牌摘要与常量时间比较。
//!
//! `contracts/link.json` 要求 `sha256` + `hex`，且摘要只用于校验调用方令牌、不得写日志。
//! 本模块只做纯计算，不产生任何日志，也不保存原始令牌。

use sha2::{Digest, Sha256};

// ===== 摘要 =====

/// 计算令牌的 SHA-256 小写十六进制摘要。
///
/// # 参数
/// - `token`: 调用方提交的原始令牌。
///
/// # 错误
/// 不返回错误。
pub fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

/// 以常量时间比较两个字符串。
///
/// # 参数
/// - `expected`: 期望值，通常来自通道注册时提交的摘要。
/// - `actual`: 实际值，通常来自本次请求令牌的摘要。
///
/// # 错误
/// 不返回错误；长度不同或任一字节不同都返回 `false`。
pub fn constant_time_eq(expected: &str, actual: &str) -> bool {
    let left = expected.as_bytes();
    let right = actual.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

/// 校验调用方令牌是否与注册时提交的摘要一致。
///
/// # 参数
/// - `presented`: 本次请求携带的原始令牌。
/// - `expected_hash`: 注册时提交的 SHA-256 十六进制摘要。
///
/// # 错误
/// 不返回错误；不一致返回 `false`。
pub fn verify_token(presented: &str, expected_hash: &str) -> bool {
    constant_time_eq(&token_hash(presented), expected_hash)
}

/// 判断字符串是否是 64 位小写十六进制摘要。
///
/// # 参数
/// - `value`: 待校验的摘要字符串。
///
/// # 错误
/// 不返回错误；不是合法摘要返回 `false`。
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
