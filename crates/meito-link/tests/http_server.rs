//! HTTP 服务用例：token 分离、状态码映射、限流、最大读取次数与访问日志禁项。
//!
//! 所有期望状态码、单包上限、突发容量与最大读取次数都从 `contracts/link.json` 读取；
//! 需要放宽限流时也以契约为基准改写临时副本，不在用例里写死数值。

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;

use meito_link::{
    load_link_contract, token_hash, ChannelPolicy, ChannelStore, InMemoryChannelStore,
    LinkContract, LinkServer,
};
use serde_json::{json, Value};

fn repo_contract_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/link.json")
}

fn repo_connection_package_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/connection-package.json")
}

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("meito-link-{name}-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

/// 以契约为基准放宽突发容量，确保最大读取次数用例不被限流提前截断。
fn relaxed_contract() -> LinkContract {
    let base = load_link_contract(&repo_contract_path()).unwrap();
    let directory = temp_dir("relaxed");
    let mut link: Value =
        serde_json::from_str(&fs::read_to_string(repo_contract_path()).unwrap()).unwrap();
    let burst = base.rate_limit.burst + base.get_semantics.max_reads as u64 + 1;
    link["rateLimit"]["burst"] = json!(burst);
    fs::write(
        directory.join("link.json"),
        serde_json::to_string_pretty(&link).unwrap(),
    )
    .unwrap();
    fs::copy(
        repo_connection_package_path(),
        directory.join("connection-package.json"),
    )
    .unwrap();
    load_link_contract(&directory.join("link.json")).unwrap()
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

struct TestServer {
    address: SocketAddr,
    contract: LinkContract,
    log_path: PathBuf,
}

impl TestServer {
    fn start(contract: LinkContract) -> Self {
        let directory = temp_dir("http");
        let log_path = directory.join("access.jsonl");
        let policy = ChannelPolicy::from_contract(&contract);
        let store: Arc<dyn ChannelStore> = Arc::new(InMemoryChannelStore::new(policy));
        let server = LinkServer::bind(
            "127.0.0.1:0".parse().unwrap(),
            contract.clone(),
            store,
            log_path.clone(),
        )
        .unwrap();
        let address = server.local_addr().unwrap();
        thread::spawn(move || {
            let _ = server.run();
        });
        TestServer {
            address,
            contract,
            log_path,
        }
    }

    fn request(&self, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> HttpResponse {
        send(self.address, method, path, token, body)
    }

    fn send_headers_only(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        content_length: usize,
    ) -> HttpResponse {
        let mut stream = TcpStream::connect(self.address).unwrap();
        let mut text = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {content_length}\r\nConnection: close\r\n"
        );
        if let Some(token) = token {
            text.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        text.push_str("\r\n");
        let _ = stream.write_all(text.as_bytes());
        read_response(&mut stream)
    }

    fn register(&self, channel_id: &str, read_token: &str, write_token: &str) -> HttpResponse {
        let body = json!({
            "channel_id": channel_id,
            "read_token_hash": token_hash(read_token),
            "write_token_hash": token_hash(write_token),
            "public_key": "test-public-key"
        })
        .to_string();
        self.request("POST", "/v1/channels", None, body.as_bytes())
    }

    fn blob_path(channel_id: &str) -> String {
        format!("/v1/channels/{channel_id}/blob")
    }
}

fn send(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &[u8],
) -> HttpResponse {
    let mut stream = TcpStream::connect(address).unwrap();
    let mut text = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(token) = token {
        text.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    text.push_str("\r\n");
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(body);
    let _ = stream.write_all(&bytes);
    read_response(&mut stream)
}

fn read_response(stream: &mut TcpStream) -> HttpResponse {
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(bytes.len());
    let head = String::from_utf8_lossy(&bytes[..split]);
    let status = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let body = if split + 4 <= bytes.len() {
        bytes[split + 4..].to_vec()
    } else {
        Vec::new()
    };
    HttpResponse { status, body }
}

// ===== 正常路径 =====

#[test]
fn register_put_get_delete_roundtrip() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let channel = "roundtrip-channel";
    assert_eq!(
        server.register(channel, "read-token", "write-token").status,
        200
    );
    let payload = b"meito-ciphertext-payload".to_vec();
    let path = TestServer::blob_path(channel);
    assert_eq!(
        server
            .request("PUT", &path, Some("write-token"), &payload)
            .status,
        200
    );
    let fetched = server.request("GET", &path, Some("read-token"), &[]);
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body, payload, "GET 必须原样返回密文");
    assert_eq!(
        server
            .request(
                "DELETE",
                &format!("/v1/channels/{channel}"),
                Some("write-token"),
                &[]
            )
            .status,
        200
    );
    assert_eq!(
        server.request("GET", &path, Some("read-token"), &[]).status,
        server.contract.http_status.unknown_channel
    );
}

// ===== token 错误路径 =====

#[test]
fn read_and_write_tokens_are_enforced_separately() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let channel = "token-channel";
    assert_eq!(
        server.register(channel, "read-token", "write-token").status,
        200
    );
    let path = TestServer::blob_path(channel);
    let payload = b"ciphertext".to_vec();
    let bad = server.contract.http_status.bad_token;
    assert_eq!(
        server
            .request("PUT", &path, Some("read-token"), &payload)
            .status,
        bad,
        "读 token 不得用于写入"
    );
    assert_eq!(
        server.request("PUT", &path, None, &payload).status,
        bad,
        "缺少 token 必须拒绝"
    );
    assert_eq!(
        server
            .request("PUT", &path, Some("write-token"), &payload)
            .status,
        200
    );
    assert_eq!(
        server
            .request("GET", &path, Some("write-token"), &[])
            .status,
        bad,
        "写 token 不得用于读取"
    );
    assert_eq!(
        server.request("GET", &path, Some("read-token"), &[]).status,
        200
    );
}

#[test]
fn unknown_channel_maps_to_contract_status() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let path = TestServer::blob_path("missing-channel");
    let expected = server.contract.http_status.unknown_channel;
    assert_eq!(
        server.request("GET", &path, Some("read-token"), &[]).status,
        expected
    );
    assert_eq!(
        server
            .request(
                "DELETE",
                "/v1/channels/missing-channel",
                Some("write-token"),
                &[]
            )
            .status,
        expected
    );
}

// ===== 报文与状态码映射 =====

#[test]
fn oversized_body_maps_to_contract_status() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let channel = "large-channel";
    assert_eq!(
        server.register(channel, "read-token", "write-token").status,
        200
    );
    let path = TestServer::blob_path(channel);
    let oversized = server.contract.package_limit.max_ciphertext_bytes + 1;
    let response = server.send_headers_only("PUT", &path, Some("write-token"), oversized);
    assert_eq!(response.status, server.contract.http_status.too_large);
}

#[test]
fn malformed_register_maps_to_contract_status() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let expected = server.contract.http_status.malformed;
    assert_eq!(
        server.request("POST", "/v1/channels", None, b"{").status,
        expected
    );
    let missing = json!({
        "channel_id": "partial-channel",
        "read_token_hash": token_hash("read-token"),
        "public_key": "test-public-key"
    })
    .to_string();
    assert_eq!(
        server
            .request("POST", "/v1/channels", None, missing.as_bytes())
            .status,
        expected
    );
}

#[test]
fn wrong_method_maps_to_contract_status() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let path = TestServer::blob_path("any-channel");
    assert_eq!(
        server
            .request("PATCH", &path, Some("write-token"), &[])
            .status,
        server.contract.http_status.method_not_allowed
    );
}

// ===== 限流与最大读取次数 =====

#[test]
fn burst_exhaustion_maps_to_rate_limited() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let path = TestServer::blob_path("rate-channel");
    let burst = server.contract.rate_limit.burst;
    let mut statuses = Vec::new();
    for _ in 0..(burst + 5) {
        statuses.push(server.request("GET", &path, Some("read-token"), &[]).status);
    }
    assert!(
        statuses.contains(&server.contract.http_status.rate_limited),
        "突发容量耗尽后必须出现限流状态码"
    );
}

#[test]
fn max_reads_is_enforced_after_contract_limit() {
    let server = TestServer::start(relaxed_contract());
    let channel = "max-reads-channel";
    assert_eq!(
        server.register(channel, "read-token", "write-token").status,
        200
    );
    let path = TestServer::blob_path(channel);
    assert_eq!(
        server
            .request("PUT", &path, Some("write-token"), b"ciphertext")
            .status,
        200
    );
    for _ in 0..server.contract.get_semantics.max_reads {
        assert_eq!(
            server.request("GET", &path, Some("read-token"), &[]).status,
            200
        );
    }
    assert_eq!(
        server.request("GET", &path, Some("read-token"), &[]).status,
        server.contract.http_status.rate_limited
    );
}

// ===== 访问日志 =====

#[test]
fn access_log_only_records_allowed_fields() {
    let server = TestServer::start(load_link_contract(&repo_contract_path()).unwrap());
    let channel = "log-channel";
    let sentinel = b"MEITO-TEST-BLOB-SENTINEL";
    assert_eq!(
        server.register(channel, "read-token", "write-token").status,
        200
    );
    let path = TestServer::blob_path(channel);
    assert_eq!(
        server
            .request("PUT", &path, Some("write-token"), sentinel)
            .status,
        200
    );
    assert_eq!(
        server.request("GET", &path, Some("read-token"), &[]).status,
        200
    );

    let text = fs::read_to_string(&server.log_path).unwrap();
    assert!(
        !text.contains("MEITO-TEST-BLOB-SENTINEL"),
        "访问日志不得出现密文片段"
    );
    for secret in ["read-token", "write-token"] {
        assert!(!text.contains(secret), "访问日志不得出现原始令牌");
    }
    let allowed: BTreeSet<String> = server
        .contract
        .logging
        .allowed_fields
        .iter()
        .cloned()
        .collect();
    let mut sizes = Vec::new();
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        let object = value.as_object().unwrap();
        let keys: BTreeSet<String> = object.keys().cloned().collect();
        assert_eq!(keys, allowed, "访问日志字段必须与契约白名单一致");
        assert!(value["status"].is_u64());
        if let Some(size) = value["size"].as_u64() {
            sizes.push(size);
        }
    }
    assert!(
        sizes.contains(&(sentinel.len() as u64)),
        "写入请求必须记录密文大小"
    );
}
