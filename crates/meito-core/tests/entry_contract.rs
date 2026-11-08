//! File: 入口端口契约测试——端口值、回环绑定与非法输入拒绝。
//!
//! 期望值一律从 `contracts/entry.json` 直接解析取得，不在测试里复写端口常量；实现不得另写
//! 端口，改写契约后加载器必须跟着变。

use std::path::{Path, PathBuf};

use meito_core::{load_entry_ports, EntryPorts, ErrorCode, ENTRY_CONTRACT_PATH};
use serde_json::{json, Value};

// ===== 测试辅助 =====

/// 返回仓库根目录。
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("解析仓库根目录")
}

/// 生成独立的临时目录，避免并发用例互相覆盖。
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("meito-entry-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("contracts")).expect("创建临时契约目录");
    root
}

/// 读取仓库契约原文。
fn contract_text() -> String {
    std::fs::read_to_string(repository_root().join(ENTRY_CONTRACT_PATH)).expect("读取仓库入口契约")
}

/// 把仓库契约复制到临时目录后替换一处文本。
fn copy_contract(root: &Path, from: &str, to: &str) {
    let replaced = contract_text().replace(from, to);
    std::fs::write(root.join(ENTRY_CONTRACT_PATH), replaced).expect("写入临时契约");
}

/// 直接从契约 JSON 取出三个端口，作为测试的期望值来源。
fn expected_ports() -> EntryPorts {
    let value: Value = serde_json::from_str(&contract_text()).expect("解析仓库入口契约");
    let port = |key: &str| value["ports"][key].as_u64().expect("ports 字段必须是整数") as u16;
    EntryPorts {
        terminal: port("terminal"),
        graphical: port("graphical"),
        vnc: port("vnc"),
    }
}

// ===== 用例 =====

#[test]
fn ports_match_contract() {
    let ports = load_entry_ports(&repository_root()).expect("加载入口契约");
    assert_eq!(
        ports,
        expected_ports(),
        "加载结果必须与 contracts/entry.json 的 ports 一致"
    );
}

#[test]
fn loader_follows_rewritten_ports() {
    let root = temp_root("rewritten");
    let mut value: Value = serde_json::from_str(&contract_text()).expect("解析仓库入口契约");
    value["ports"] = json!({
        "terminal": 17001,
        "graphical": 16080,
        "vnc": 15900
    });
    std::fs::write(
        root.join(ENTRY_CONTRACT_PATH),
        serde_json::to_string(&value).expect("序列化临时契约"),
    )
    .expect("写入临时契约");
    let ports = load_entry_ports(&root).expect("加载改写后的入口契约");
    assert_eq!(
        ports,
        EntryPorts {
            terminal: 17001,
            graphical: 16080,
            vnc: 15900,
        },
        "加载器必须读取契约值，不得写死端口"
    );
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn wildcard_bind_is_rejected() {
    let root = temp_root("wildcard");
    copy_contract(&root, "\"127.0.0.1\"", "\"0.0.0.0\"");
    let error = load_entry_ports(&root).expect_err("通配绑定必须被拒绝");
    assert_eq!(error.code(), ErrorCode::Config);
    assert_eq!(error.exit_code(), 2);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn graphical_entry_remains_reserved() {
    let value: Value = serde_json::from_str(&contract_text()).expect("解析仓库入口契约");
    assert_eq!(
        value["graphical"]["status"].as_str(),
        Some("reserved"),
        "v1 图形入口必须保持 reserved"
    );
    for key in [
        "ports.graphical",
        "ports.vnc",
        "graphical.framebuffer",
        "graphical.vncPasswordChars",
        "graphical.vncPasswordDesOnly",
    ] {
        assert_eq!(
            value["requiredness"][key].as_str(),
            Some("reserved"),
            "{key} 必须保持 reserved"
        );
    }
    assert_eq!(
        value["requiredness"]["ports.terminal"].as_str(),
        Some("required"),
        "终端入口必须是 required"
    );
    assert_eq!(
        value["requiredness"]["bind.address"].as_str(),
        Some("required"),
        "回环绑定必须是 required"
    );
}

#[test]
fn missing_contract_is_rejected() {
    let root = temp_root("missing");
    let error = load_entry_ports(&root).expect_err("缺少契约必须被拒绝");
    assert_eq!(error.code(), ErrorCode::Config);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}
