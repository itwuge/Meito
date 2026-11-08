//! File: tools.lock.json 加载与校验的运行用例。
//!
//! 验证两类语义：锁定条目可安装；任何 `unpinned`、缺字段或非法 SHA256 都必须在安装前
//! 被 `E_BOOTSTRAP`（退出码 4）拒绝。

use std::path::{Path, PathBuf};

use meito_core::{load_tools_lock, new_session_id, ErrorCode, PinStatus, TOOLS_LOCK_PATH};
use serde_json::json;

// ===== 临时目录 =====

/// 测试用临时目录，析构时清理。
struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    /// 创建唯一临时目录。
    ///
    /// # 参数
    /// - `label`: 目录名前缀，便于定位失败用例。
    ///
    /// # 错误
    /// 目录创建失败时 panic。
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("meito-lock-{}-{}", label, new_session_id()));
        std::fs::create_dir_all(&path).expect("创建测试目录");
        Self { path }
    }

    /// 返回目录路径。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    fn path(&self) -> &Path {
        &self.path
    }

    /// 写入锁定清单。
    ///
    /// # 参数
    /// - `value`: 清单 JSON。
    ///
    /// # 错误
    /// 文件写入失败时 panic。
    fn write_lock(&self, value: &serde_json::Value) {
        let text = serde_json::to_string_pretty(value).expect("序列化清单");
        std::fs::write(self.path.join(TOOLS_LOCK_PATH), text).expect("写入清单");
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!("清理临时目录 {} 失败：{error}", self.path.display());
        }
    }
}

// ===== 合法清单 =====

#[test]
fn pinned_entry_can_be_installed() {
    let directory = TempDirectory::new("pinned");
    directory.write_lock(&json!({
        "lockVersion": 1,
        "tools": {
            "frpc": {
                "version": "0.58.0",
                "url": "https://example.test/frpc",
                "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "status": "pinned"
            }
        }
    }));

    let lock = load_tools_lock(directory.path()).expect("合法清单应加载");
    assert_eq!(lock.lock_version(), 1);
    let entry = lock.require_pinned("frpc").expect("pinned 条目允许安装");
    assert_eq!(entry.name(), "frpc");
    assert_eq!(entry.version(), Some("0.58.0"));
    assert_eq!(entry.status(), PinStatus::Pinned);
    assert!(entry.url().is_some());
    assert!(entry.sha256().is_some());
}

#[test]
fn repository_lock_file_is_valid_but_all_unpinned() {
    // 仓库实际清单必须能被加载，且当前阶段全部是 unpinned，安装必须被拒绝。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lock = load_tools_lock(&root).expect("仓库 tools.lock.json 应合法");
    assert!(lock.entries().count() >= 5, "至少覆盖五个远端工具");
    for entry in lock.entries() {
        assert_eq!(
            entry.status(),
            PinStatus::Unpinned,
            "未提供真实版本/哈希前不得标记 pinned：{}",
            entry.name()
        );
    }
    let error = lock
        .require_pinned("frpc")
        .expect_err("unpinned 条目必须拒绝安装");
    assert_eq!(error.code(), ErrorCode::UnpinnedTool);
    assert_eq!(error.exit_code(), 4);
}

// ===== 拒绝安装 =====

#[test]
fn unpinned_entry_is_rejected_before_install() {
    let directory = TempDirectory::new("unpinned");
    directory.write_lock(&json!({
        "lockVersion": 1,
        "tools": {
            "ttyd": { "version": null, "url": null, "sha256": null, "status": "unpinned" }
        }
    }));

    let lock = load_tools_lock(directory.path()).expect("unpinned 清单本身可加载");
    let error = lock
        .require_pinned("ttyd")
        .expect_err("unpinned 必须拒绝安装");
    assert_eq!(error.code(), ErrorCode::UnpinnedTool);
    assert_eq!(error.exit_code(), 4);
    assert!(error.message().contains("unpinned"));
}

#[test]
fn missing_tool_is_rejected() {
    let directory = TempDirectory::new("missing-tool");
    directory.write_lock(&json!({
        "lockVersion": 1,
        "tools": {
            "frpc": { "status": "unpinned" }
        }
    }));

    let lock = load_tools_lock(directory.path()).expect("清单可加载");
    let error = lock.require_pinned("ttyd").expect_err("缺少工具必须拒绝");
    assert_eq!(error.code(), ErrorCode::Config);
    assert!(error.message().contains("缺少工具"));
}

// ===== 非法清单 =====

#[test]
fn missing_lock_file_is_rejected() {
    let directory = TempDirectory::new("missing-file");
    let error = load_tools_lock(directory.path()).expect_err("缺少清单必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
    assert_eq!(error.exit_code(), 2);
}

#[test]
fn broken_sha256_is_rejected() {
    let directory = TempDirectory::new("bad-sha");
    directory.write_lock(&json!({
        "lockVersion": 1,
        "tools": {
            "frpc": {
                "version": "0.58.0",
                "url": "https://example.test/frpc",
                "sha256": "not-a-hash",
                "status": "pinned"
            }
        }
    }));

    let error = load_tools_lock(directory.path()).expect_err("非法 SHA256 必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
    assert!(error.message().contains("64 位十六进制"));
}

#[test]
fn pinned_entry_requires_all_fields() {
    let directory = TempDirectory::new("incomplete");
    directory.write_lock(&json!({
        "lockVersion": 1,
        "tools": {
            "frpc": {
                "version": "0.58.0",
                "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "status": "pinned"
            }
        }
    }));

    let error = load_tools_lock(directory.path()).expect_err("pinned 缺 URL 必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
    assert!(error.message().contains("url"));
}

#[test]
fn unknown_status_is_rejected() {
    let directory = TempDirectory::new("bad-status");
    directory.write_lock(&json!({
        "lockVersion": 1,
        "tools": {
            "frpc": { "status": "maybe" }
        }
    }));

    let error = load_tools_lock(directory.path()).expect_err("未知状态必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
    assert!(error.message().contains("未知"));
}

#[test]
fn unsupported_lock_version_is_rejected() {
    let directory = TempDirectory::new("bad-version");
    directory.write_lock(&json!({
        "lockVersion": 2,
        "tools": {
            "frpc": { "status": "unpinned" }
        }
    }));

    let error = load_tools_lock(directory.path()).expect_err("未知版本必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
    assert!(error.message().contains("不受支持"));
}

#[test]
fn malformed_json_is_rejected() {
    let directory = TempDirectory::new("malformed");
    std::fs::write(directory.path().join(TOOLS_LOCK_PATH), "{ not json").expect("写入非法 JSON");

    let error = load_tools_lock(directory.path()).expect_err("非法 JSON 必须报错");
    assert_eq!(error.code(), ErrorCode::Config);
}
