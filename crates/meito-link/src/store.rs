//! File: meito-link 的存储抽象与内存实现。
//!
//! 服务端只存密文：本模块不解析 blob、不计算摘要、不写日志。TTL、单包上限与最大读取
//! 次数全部来自 `ChannelPolicy`，而该策略必须由 `contracts/link.json` 构造；这里没有
//! 任何内置默认值。时间由调用方传入，便于并发与过期语义用确定性用例验证。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use crate::contract::LinkContract;

// ===== 存储策略 =====

/// 通道存储的数值策略；所有字段都来自契约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPolicy {
    /// 通道存活时长。
    pub ttl: Duration,
    /// 单包密文上限（字节）。
    pub max_ciphertext_bytes: usize,
    /// 单条密文最多可被读取的次数。
    pub max_reads: u32,
}

impl ChannelPolicy {
    /// 从已校验的契约构造存储策略。
    ///
    /// # 参数
    /// - `contract`: 已通过 `load_link_contract` 校验的契约。
    ///
    /// # 错误
    /// 不返回错误；契约加载阶段已保证数值合法。
    pub fn from_contract(contract: &LinkContract) -> Self {
        Self {
            ttl: Duration::from_secs(contract.channel_ttl_seconds),
            max_ciphertext_bytes: contract.max_ciphertext_bytes,
            max_reads: contract.get_semantics.max_reads,
        }
    }
}

// ===== 存储值 =====

/// 一次通道注册；读写 token 只以哈希形式进入服务端。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    /// 不透明通道 id。
    pub channel_id: String,
    /// 读 token 的 SHA256 十六进制摘要。
    pub read_token_hash: String,
    /// 写 token 的 SHA256 十六进制摘要。
    pub write_token_hash: String,
    /// 本地临时 X25519 公钥（base64url 无填充）。
    pub public_key: String,
}

/// 一次成功的密文读取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRead {
    /// 原始密文，服务端不解析。
    pub blob: Vec<u8>,
    /// 本次读取后累计的读取次数。
    pub reads: u32,
}

/// 通道的校验材料；只供服务层比对 token 哈希，不得写日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCredentials {
    /// 读 token 哈希。
    pub read_token_hash: String,
    /// 写 token 哈希。
    pub write_token_hash: String,
    /// 一次性公钥。
    pub public_key: String,
}

/// 存储错误；与 HTTP 状态码的映射在接入服务层时按契约完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// 通道已存在且尚未过期。
    ChannelExists,
    /// 通道不存在。
    NotFound,
    /// 通道已过期。
    Expired,
    /// 密文超过契约上限。
    TooLarge,
    /// 读取次数达到契约上限。
    MaxReadsReached,
}

// ===== 存储抽象 =====

/// 通道与密文的存储接口；实现方可替换为持久化存储而不改变服务层语义。
pub trait ChannelStore: Send + Sync {
    /// 注册一个新通道；同 id 的过期通道可被覆盖。
    ///
    /// # 参数
    /// - `registration`: 通道 id、读写 token 哈希与一次性公钥。
    /// - `now`: 当前时刻，由调用方注入。
    ///
    /// # 错误
    /// 通道已存在且未过期返回 `ChannelExists`。
    fn register(&self, registration: Registration, now: SystemTime) -> Result<(), StoreError>;

    /// 写入或覆盖密文；同一通道重复写入是允许的。
    ///
    /// # 参数
    /// - `channel_id`: 通道 id。
    /// - `blob`: 密文字节；不得被本层解析。
    /// - `now`: 当前时刻。
    ///
    /// # 错误
    /// 通道不存在返回 `NotFound`，已过期返回 `Expired`，超过上限返回 `TooLarge`。
    fn put_ciphertext(
        &self,
        channel_id: &str,
        blob: Vec<u8>,
        now: SystemTime,
    ) -> Result<(), StoreError>;

    /// 读取密文并累计读取次数。
    ///
    /// # 参数
    /// - `channel_id`: 通道 id。
    /// - `now`: 当前时刻。
    ///
    /// # 错误
    /// 通道不存在返回 `NotFound`，已过期返回 `Expired`，达到 `max_reads` 返回
    /// `MaxReadsReached`。
    fn read_ciphertext(&self, channel_id: &str, now: SystemTime) -> Result<BlobRead, StoreError>;

    /// 撤销通道并丢弃密文。
    ///
    /// # 参数
    /// - `channel_id`: 通道 id。
    ///
    /// # 错误
    /// 通道不存在返回 `NotFound`。
    fn delete(&self, channel_id: &str) -> Result<(), StoreError>;

    /// 返回通道的 token 哈希与公钥，供服务层校验请求 token。
    ///
    /// # 参数
    /// - `channel_id`: 通道 id。
    ///
    /// # 错误
    /// 不返回错误；通道不存在返回 `None`。
    fn credentials(&self, channel_id: &str) -> Option<ChannelCredentials>;

    /// 清理所有已过期通道。
    ///
    /// # 参数
    /// - `now`: 当前时刻。
    ///
    /// # 错误
    /// 不返回错误。
    fn purge_expired(&self, now: SystemTime) -> usize;
}

// ===== 内存实现 =====

/// 进程内通道存储；并发安全，进程退出即全部丢失。
#[derive(Debug)]
pub struct InMemoryChannelStore {
    policy: ChannelPolicy,
    channels: Mutex<HashMap<String, ChannelState>>,
}

impl InMemoryChannelStore {
    /// 用契约策略构造空存储。
    ///
    /// # 参数
    /// - `policy`: 由契约构造的通道策略。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn new(policy: ChannelPolicy) -> Self {
        Self {
            policy,
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// 获取锁；锁中毒时继续使用内部状态，避免单个 panic 让服务不可用。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    fn lock(&self) -> MutexGuard<'_, HashMap<String, ChannelState>> {
        match self.channels.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 计算新通道的过期时刻。
    ///
    /// # 参数
    /// - `now`: 注册时刻。
    ///
    /// # 错误
    /// 不返回错误；时间溢出时退化为 `now`，使通道立即过期而不无限存活。
    fn expiry(&self, now: SystemTime) -> SystemTime {
        now.checked_add(self.policy.ttl).unwrap_or(now)
    }
}

/// 单通道的内部状态；blob 只在内存中短暂存在。
#[derive(Debug)]
struct ChannelState {
    read_token_hash: String,
    write_token_hash: String,
    public_key: String,
    blob: Option<Vec<u8>>,
    reads: u32,
    expires_at: SystemTime,
}

impl ChannelStore for InMemoryChannelStore {
    fn register(&self, registration: Registration, now: SystemTime) -> Result<(), StoreError> {
        let expires_at = self.expiry(now);
        let mut channels = self.lock();
        if let Some(existing) = channels.get(&registration.channel_id) {
            if now < existing.expires_at {
                return Err(StoreError::ChannelExists);
            }
            channels.remove(&registration.channel_id);
        }
        channels.insert(
            registration.channel_id,
            ChannelState {
                read_token_hash: registration.read_token_hash,
                write_token_hash: registration.write_token_hash,
                public_key: registration.public_key,
                blob: None,
                reads: 0,
                expires_at,
            },
        );
        Ok(())
    }

    fn put_ciphertext(
        &self,
        channel_id: &str,
        blob: Vec<u8>,
        now: SystemTime,
    ) -> Result<(), StoreError> {
        if blob.len() > self.policy.max_ciphertext_bytes {
            return Err(StoreError::TooLarge);
        }
        let mut channels = self.lock();
        match channels.get_mut(channel_id) {
            None => Err(StoreError::NotFound),
            Some(state) if now >= state.expires_at => Err(StoreError::Expired),
            Some(state) => {
                state.blob = Some(blob);
                state.reads = 0;
                Ok(())
            }
        }
    }

    fn read_ciphertext(&self, channel_id: &str, now: SystemTime) -> Result<BlobRead, StoreError> {
        let mut channels = self.lock();
        let state = match channels.get_mut(channel_id) {
            None => return Err(StoreError::NotFound),
            Some(state) if now >= state.expires_at => return Err(StoreError::Expired),
            Some(state) => state,
        };
        let blob = state.blob.as_ref().ok_or(StoreError::NotFound)?;
        if state.reads >= self.policy.max_reads {
            return Err(StoreError::MaxReadsReached);
        }
        state.reads += 1;
        Ok(BlobRead {
            blob: blob.clone(),
            reads: state.reads,
        })
    }

    fn delete(&self, channel_id: &str) -> Result<(), StoreError> {
        match self.lock().remove(channel_id) {
            Some(_) => Ok(()),
            None => Err(StoreError::NotFound),
        }
    }

    fn credentials(&self, channel_id: &str) -> Option<ChannelCredentials> {
        self.lock().get(channel_id).map(|state| ChannelCredentials {
            read_token_hash: state.read_token_hash.clone(),
            write_token_hash: state.write_token_hash.clone(),
            public_key: state.public_key.clone(),
        })
    }

    fn purge_expired(&self, now: SystemTime) -> usize {
        let mut channels = self.lock();
        let before = channels.len();
        channels.retain(|_, state| now < state.expires_at);
        before - channels.len()
    }
}
