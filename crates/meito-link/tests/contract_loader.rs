//! 契约加载器用例：引用解析、数值跟随与非法结构拒绝。
//!
//! 所有期望值都从 `contracts/link.json` 与它引用的 `contracts/connection-package.json`
//! 读取，不在用例里写死 7200 或 16384。

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use meito_core::ErrorCode;
use meito_link::{load_link_contract, ChannelPolicy};
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

fn read_json(path: &PathBuf) -> Value {
    let text = fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn write_contract_with(mutator: impl FnOnce(&mut Value)) -> PathBuf {
    let directory = temp_dir("contract");
    let mut link = read_json(&repo_contract_path());
    mutator(&mut link);
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
    directory.join("link.json")
}

#[test]
fn resolves_package_limit_reference_instead_of_copying_value() {
    let contract = load_link_contract(&repo_contract_path()).unwrap();
    let package = read_json(&repo_connection_package_path());
    let referenced = package["serverHandling"]["maxCiphertextBytes"]
        .as_u64()
        .unwrap();
    assert_eq!(
        contract.package_limit.max_ciphertext_bytes as u64,
        referenced
    );
    assert!(
        contract
            .package_limit
            .max_ciphertext_bytes_ref
            .contains("#serverHandling.maxCiphertextBytes"),
        "引用表达式必须保留被引用路径"
    );
}

#[test]
fn follows_new_values_when_contract_and_reference_change() {
    let base = load_link_contract(&repo_contract_path()).unwrap();
    let new_ttl = base.channel_ttl_seconds + 60;
    let new_limit = base.package_limit.max_ciphertext_bytes as u64 + 1;
    let directory = temp_dir("follow");
    let mut link = read_json(&repo_contract_path());
    link["channelTtlSeconds"] = json!(new_ttl);
    fs::write(
        directory.join("link.json"),
        serde_json::to_string_pretty(&link).unwrap(),
    )
    .unwrap();
    let mut package = read_json(&repo_connection_package_path());
    package["serverHandling"]["maxCiphertextBytes"] = json!(new_limit);
    fs::write(
        directory.join("connection-package.json"),
        serde_json::to_string_pretty(&package).unwrap(),
    )
    .unwrap();

    let contract = load_link_contract(&directory.join("link.json")).unwrap();
    assert_eq!(contract.channel_ttl_seconds, new_ttl);
    assert_eq!(
        contract.package_limit.max_ciphertext_bytes as u64,
        new_limit
    );
    let policy = ChannelPolicy::from_contract(&contract);
    assert_eq!(policy.ttl.as_secs(), new_ttl);
    assert_eq!(policy.max_ciphertext_bytes as u64, new_limit);
}

#[test]
fn rejects_inlined_limit_value() {
    let base = load_link_contract(&repo_contract_path()).unwrap();
    let inlined = base.package_limit.max_ciphertext_bytes as u64;
    let path = write_contract_with(|link| {
        link["packageLimit"] = json!({ "maxCiphertextBytes": inlined });
    });
    let error = load_link_contract(&path).expect_err("不得接受复写数值");
    assert_eq!(error.code(), ErrorCode::Config);
}

#[test]
fn rejects_missing_reference() {
    let path = write_contract_with(|link| {
        link["packageLimit"] = json!({ "note": "missing" });
    });
    let error = load_link_contract(&path).expect_err("缺失引用必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
}

#[test]
fn rejects_unresolvable_reference() {
    let path = write_contract_with(|link| {
        link["packageLimit"] = json!({
            "maxCiphertextBytesRef": "missing-file.json#serverHandling.maxCiphertextBytes"
        });
    });
    let error = load_link_contract(&path).expect_err("不可解析的引用必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
}
