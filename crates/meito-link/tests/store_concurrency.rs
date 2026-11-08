//! 存储并发语义用例：重复写入、写后读、过期后读、DELETE 后读，外加最大读取次数。
//!
//! TTL、单包上限与最大读取次数全部从 `contracts/link.json` 读取，用例不写死数值。

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use meito_link::{
    load_link_contract, token_hash, ChannelPolicy, ChannelStore, InMemoryChannelStore,
    LinkContract, Registration, StoreError,
};

const CHANNEL_ID: &str = "concurrent-channel";

fn contract() -> LinkContract {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/link.json");
    load_link_contract(&path).unwrap()
}

struct Fixture {
    store: Arc<InMemoryChannelStore>,
    ttl: Duration,
    max_bytes: usize,
    max_reads: u32,
}

fn fixture() -> Fixture {
    let contract = contract();
    let policy = ChannelPolicy::from_contract(&contract);
    Fixture {
        ttl: policy.ttl,
        max_bytes: policy.max_ciphertext_bytes,
        max_reads: policy.max_reads,
        store: Arc::new(InMemoryChannelStore::new(policy)),
    }
}

fn registration() -> Registration {
    Registration {
        channel_id: CHANNEL_ID.to_string(),
        read_token_hash: token_hash("read-token"),
        write_token_hash: token_hash("write-token"),
        public_key: "unused-test-public-key".to_string(),
    }
}

#[test]
fn duplicate_writes_are_concurrency_safe() {
    let fixture = fixture();
    let now = SystemTime::now();
    fixture.store.register(registration(), now).unwrap();
    let worker_count = 8;
    let handles: Vec<_> = (0..worker_count)
        .map(|index| {
            let store = Arc::clone(&fixture.store);
            thread::spawn(move || {
                let blob = vec![index as u8; index + 1];
                store.put_ciphertext(CHANNEL_ID, blob, now)
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), Ok(()));
    }
    let read = fixture.store.read_ciphertext(CHANNEL_ID, now).unwrap();
    assert!(!read.blob.is_empty(), "重复写入后必须仍有可读密文");
    assert_eq!(read.reads, 1, "读取计数从覆盖写入后重新开始");
}

#[test]
fn write_then_read_returns_exact_ciphertext() {
    let fixture = fixture();
    let now = SystemTime::now();
    fixture.store.register(registration(), now).unwrap();
    let payload = vec![0x5au8; fixture.max_bytes];
    fixture
        .store
        .put_ciphertext(CHANNEL_ID, payload.clone(), now)
        .unwrap();
    let read = fixture.store.read_ciphertext(CHANNEL_ID, now).unwrap();
    assert_eq!(read.blob, payload, "服务端必须原样返回密文");
    assert_eq!(read.reads, 1);
}

#[test]
fn read_after_expiry_is_rejected() {
    let fixture = fixture();
    let now = SystemTime::now();
    fixture.store.register(registration(), now).unwrap();
    fixture
        .store
        .put_ciphertext(CHANNEL_ID, vec![1u8; 4], now)
        .unwrap();
    let just_before = now + fixture.ttl - Duration::from_secs(1);
    assert!(fixture
        .store
        .read_ciphertext(CHANNEL_ID, just_before)
        .is_ok());
    let at_expiry = now + fixture.ttl;
    assert_eq!(
        fixture.store.read_ciphertext(CHANNEL_ID, at_expiry),
        Err(StoreError::Expired)
    );
}

#[test]
fn read_after_delete_is_not_found() {
    let fixture = fixture();
    let now = SystemTime::now();
    fixture.store.register(registration(), now).unwrap();
    fixture
        .store
        .put_ciphertext(CHANNEL_ID, vec![2u8; 4], now)
        .unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&fixture.store);
            thread::spawn(move || store.delete(CHANNEL_ID, now))
        })
        .collect();
    let mut deleted = 0;
    for handle in handles {
        if handle.join().unwrap() == Ok(()) {
            deleted += 1;
        }
    }
    assert_eq!(deleted, 1, "并发 DELETE 只能有一个成功");
    assert_eq!(
        fixture.store.read_ciphertext(CHANNEL_ID, now),
        Err(StoreError::NotFound)
    );
}

#[test]
fn reads_stop_at_contract_limit() {
    let fixture = fixture();
    let now = SystemTime::now();
    fixture.store.register(registration(), now).unwrap();
    fixture
        .store
        .put_ciphertext(CHANNEL_ID, vec![3u8; 4], now)
        .unwrap();
    for expected in 1..=fixture.max_reads {
        let read = fixture.store.read_ciphertext(CHANNEL_ID, now).unwrap();
        assert_eq!(read.reads, expected);
    }
    assert_eq!(
        fixture.store.read_ciphertext(CHANNEL_ID, now),
        Err(StoreError::MaxReadsReached)
    );
}
