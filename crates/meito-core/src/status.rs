//! File: `status` 子命令的内核实现。
//!
//! status 只读取 `.meito/state/` 的快照并渲染当前状态、run id、入口 URL 与心跳；
//! 显式指定且不存在的会话返回 `E_ORPHAN`，没有任何会话时展示 `idle` 而不是报错。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::error::{ErrorCode, MeitoError};
use crate::log::is_uuid_v4;
use crate::state::SessionState;
use crate::state_store::{SessionSnapshot, StateStore};

// ===== 结果模型 =====

/// status 输出模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub session_id: Option<String>,
    pub state: SessionState,
    pub terminal: bool,
    pub run_id: Option<String>,
    pub entry: Option<String>,
    pub entry_urls: BTreeMap<String, String>,
    pub last_heartbeat: Option<String>,
    pub p2p_provider: Option<String>,
    pub error_code: Option<String>,
}

impl StatusReport {
    /// 输出机读结果。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn to_json(&self) -> Value {
        let mut urls = Map::new();
        for (key, value) in &self.entry_urls {
            urls.insert(key.clone(), Value::String(value.clone()));
        }
        json!({
            "session_id": self.session_id,
            "state": self.state.as_str(),
            "terminal": self.terminal,
            "run_id": self.run_id,
            "entry": self.entry,
            "entry_urls": Value::Object(urls),
            "last_heartbeat": self.last_heartbeat,
            "p2p_provider": self.p2p_provider,
            "error_code": self.error_code,
        })
    }

    /// 没有任何会话时的 idle 视图。
    ///
    /// # 参数
    /// - 无。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn idle() -> Self {
        Self {
            session_id: None,
            state: SessionState::Idle,
            terminal: false,
            run_id: None,
            entry: None,
            entry_urls: BTreeMap::new(),
            last_heartbeat: None,
            p2p_provider: None,
            error_code: None,
        }
    }

    /// 从快照转换。
    ///
    /// # 参数
    /// - `snapshot`: 状态文件快照。
    ///
    /// # 错误
    /// 不返回错误。
    fn from_snapshot(snapshot: SessionSnapshot) -> Self {
        Self {
            session_id: Some(snapshot.session_id),
            state: snapshot.state,
            terminal: snapshot.state.is_terminal(),
            run_id: snapshot.run_id,
            entry: snapshot.entry,
            entry_urls: snapshot.entry_urls,
            last_heartbeat: snapshot.last_heartbeat,
            p2p_provider: snapshot.p2p_provider,
            error_code: snapshot.error_code,
        }
    }
}

// ===== 读取逻辑 =====

/// 读取指定会话或最近会话的状态。
///
/// # 参数
/// - `root`: 工作目录。
/// - `session`: `None`/`Some("latest")` 取最近快照；显式 UUID 取指定会话。
///
/// # 错误
/// 显式会话 id 非法返回 `E_USAGE`；显式会话不存在返回 `E_ORPHAN`；
/// 状态文件损坏返回 `E_INTERNAL`。
pub fn read_status(root: &Path, session: Option<&str>) -> Result<StatusReport, MeitoError> {
    let store = StateStore::new(root);
    match session {
        Some(value) if value != "latest" => {
            if !is_uuid_v4(value) {
                return Err(MeitoError::new(
                    ErrorCode::Usage,
                    format!("--session 必须是 UUID v4 或 latest，收到 {value}"),
                ));
            }
            match store.load(value)? {
                Some(snapshot) => Ok(StatusReport::from_snapshot(snapshot)),
                None => Err(MeitoError::new(
                    ErrorCode::Orphan,
                    format!("找不到会话 {value} 的状态快照"),
                )),
            }
        }
        _ => match store.latest()? {
            Some(snapshot) => Ok(StatusReport::from_snapshot(snapshot)),
            None => Ok(StatusReport::idle()),
        },
    }
}
