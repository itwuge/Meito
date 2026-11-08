//! File: 入口端口契约测试——端口值、回环绑定与非法输入拒绝。
//!
//! 断言直接来自 `contracts/entry.json`；实现不得在代码里另写端口。

use std::path::{Path, PathBuf};

use meito_core::{load_entry_ports, ErrorCode, ENTRY_CONTRACT_PATH};

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

/// 把仓库契约复制到临时目录后替换一处文本。
fn copy_contract(root: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(repository_root().join(ENTRY_CONTRACT_PATH))
        .expect("读取仓库入口契约");
    let replaced = text.replace(from, to);
    std::fs::write(root.join(ENTRY_CONTRACT_PATH), replaced).expect("写入临时契约");
}

// ===== 用例 =====

#[test]
fn ports_match_contract() {
    let ports = load_entry_ports(&repository_root()).expect("加载入口契约");
    assert_eq!(ports.terminal, 7681, "terminal 端口必须与契约一致");
    assert_eq!(ports.graphical, 6080, "graphical 端口必须与契约一致");
    assert_eq!(ports.vnc, 5900, "vnc 端口必须与契约一致");
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
fn missing_contract_is_rejected() {
    let root = temp_root("missing");
    let error = load_entry_ports(&root).expect_err("缺少契约必须被拒绝");
    assert_eq!(error.code(), ErrorCode::Config);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}
