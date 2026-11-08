//! File: 会话状态快照的落盘与读取。
//!
//! 契约没有规定状态文件的具体位置，M2 采用 `.meito/state/<session>.json`：一个会话一个快照，
//! 写入先落临时文件再原子替换，读取失败必须回传错误，不允许静默跳过。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::error::{ErrorCode, MeitoError};
use crate::log::is_uuid_v4;
use crate::state::SessionState;

// ===== 快照模型 =====

/// 供 `status` 与后续命令共用的会话快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub state: SessionState,
    pub run_id: Option<String>,
    pub entry: Option<String>,
    pub entry_urls: BTreeMap<String, String>,
    pub last_heartbeat: Option<String>,
    pub p2p_provider: Option<String>,
    pub error_code: Option<String>,
}

impl SessionSnapshot {
    /// 构造一个处于指定状态的会话快照。
    ///
    /// # 参数
    /// - `session_id`: 会话 id，必须是 UUID v4。
    /// - `state`: 初始状态。
    ///
    /// # 错误
    /// 会话 id 非法返回 `E_USAGE`。
    pub fn new(session_id: &str, state: SessionState) -> Result<Self, MeitoError> {
        if !is_uuid_v4(session_id) {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                format!("会话 id 必须是 UUID v4，收到 {session_id}"),
            ));
        }
        Ok(Self {
            session_id: session_id.to_string(),
            state,
            run_id: None,
            entry: None,
            entry_urls: BTreeMap::new(),
            last_heartbeat: None,
            p2p_provider: None,
            error_code: None,
        })
    }

    /// 输出状态文件 JSON。
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
            "run_id": self.run_id,
            "entry": self.entry,
            "entry_urls": Value::Object(urls),
            "last_heartbeat": self.last_heartbeat,
            "p2p_provider": self.p2p_provider,
            "error_code": self.error_code,
        })
    }

    /// 从状态文件 JSON 还原快照。
    ///
    /// # 参数
    /// - `value`: 状态文件解析出的 JSON 对象。
    ///
    /// # 错误
    /// 缺少必要字段、会话 id 非法或状态名未知时返回 `E_INTERNAL`。
    pub fn from_json(value: &Value) -> Result<Self, MeitoError> {
        let object = value.as_object().ok_or_else(|| {
            MeitoError::new(ErrorCode::Internal, "状态文件根节点必须是 JSON 对象")
        })?;
        let session_id = required_string(object, "session_id")?;
        if !is_uuid_v4(&session_id) {
            return Err(MeitoError::new(
                ErrorCode::Internal,
                format!("状态文件中的会话 id 不是 UUID v4：{session_id}"),
            ));
        }
        let state_name = required_string(object, "state")?;
        let state = SessionState::parse(&state_name).ok_or_else(|| {
            MeitoError::new(
                ErrorCode::Internal,
                format!("状态文件中的状态名未知：{state_name}"),
            )
        })?;
        let mut entry_urls = BTreeMap::new();
        if let Some(urls) = object.get("entry_urls") {
            if let Some(map) = urls.as_object() {
                for (key, child) in map {
                    let url = child.as_str().ok_or_else(|| {
                        MeitoError::new(
                            ErrorCode::Internal,
                            format!("entry_urls.{key} 必须是字符串"),
                        )
                    })?;
                    entry_urls.insert(key.clone(), url.to_string());
                }
            } else if !urls.is_null() {
                return Err(MeitoError::new(
                    ErrorCode::Internal,
                    "entry_urls 必须是对象或 null",
                ));
            }
        }
        let error_code = match optional_string(object, "error_code")? {
            Some(code) if !code.is_empty() => {
                if ErrorCode::parse(&code).is_none() {
                    return Err(MeitoError::new(
                        ErrorCode::Internal,
                        format!("状态文件中的错误码未知：{code}"),
                    ));
                }
                Some(code)
            }
            _ => None,
        };
        Ok(Self {
            session_id,
            state,
            run_id: optional_string(object, "run_id")?,
            entry: optional_string(object, "entry")?,
            entry_urls,
            last_heartbeat: optional_string(object, "last_heartbeat")?,
            p2p_provider: optional_string(object, "p2p_provider")?,
            error_code,
        })
    }
}

/// 读取必填字符串字段，缺失或类型错误时返回 `E_INTERNAL`。
///
/// # 参数
/// - `object`: JSON 对象。
/// - `key`: 字段名。
///
/// # 错误
/// 字段缺失或不是字符串时返回错误。
fn required_string(object: &Map<String, Value>, key: &str) -> Result<String, MeitoError> {
    match object.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(MeitoError::new(
            ErrorCode::Internal,
            format!("状态文件缺少字符串字段 {key}"),
        )),
    }
}

/// 读取可选字符串字段；null 视为未设置。
///
/// # 参数
/// - `object`: JSON 对象。
/// - `key`: 字段名。
///
/// # 错误
/// 字段存在但不是字符串或 null 时返回错误。
fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, MeitoError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value.clone()))
            }
        }
        Some(_) => Err(MeitoError::new(
            ErrorCode::Internal,
            format!("状态文件字段 {key} 必须是字符串或 null"),
        )),
    }
}

// ===== 状态仓库 =====

/// `.meito/state/` 下会话快照的唯一读写入口。
#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
}

impl StateStore {
    /// 绑定工作目录。
    ///
    /// # 参数
    /// - `root`: 工作目录。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 返回状态目录路径。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn directory(&self) -> PathBuf {
        self.root.join(".meito").join("state")
    }

    /// 原子写入一个会话快照。
    ///
    /// # 参数
    /// - `snapshot`: 待写入快照。
    ///
    /// # 错误
    /// 目录创建、临时写入或替换失败时返回 `E_INTERNAL`。
    pub fn save(&self, snapshot: &SessionSnapshot) -> Result<PathBuf, MeitoError> {
        let directory = self.directory();
        fs::create_dir_all(&directory).map_err(|error| {
            MeitoError::io(
                ErrorCode::Internal,
                format!("创建状态目录 {}", directory.display()),
                &error,
            )
        })?;
        let target = directory.join(format!("{}.json", snapshot.session_id));
        let temporary = directory.join(format!("{}.json.tmp", snapshot.session_id));
        let text = serde_json::to_string_pretty(&snapshot.to_json()).map_err(|error| {
            MeitoError::new(
                ErrorCode::Internal,
                format!("序列化状态快照失败：{}", error),
            )
        })?;
        fs::write(&temporary, text).map_err(|error| {
            MeitoError::io(
                ErrorCode::Internal,
                format!("写入临时状态文件 {}", temporary.display()),
                &error,
            )
        })?;
        if target.exists() {
            fs::remove_file(&target).map_err(|error| {
                MeitoError::io(
                    ErrorCode::Internal,
                    format!("替换状态文件 {} 前删除失败", target.display()),
                    &error,
                )
            })?;
        }
        fs::rename(&temporary, &target).map_err(|error| {
            MeitoError::io(
                ErrorCode::Internal,
                format!("原子替换状态文件 {}", target.display()),
                &error,
            )
        })?;
        Ok(target)
    }

    /// 按会话 id读取快照；不存在时返回 `None`。
    ///
    /// # 参数
    /// - `session_id`: 必须是 UUID v4。
    ///
    /// # 错误
    /// 会话 id 非法返回 `E_USAGE`；文件读取或解析失败返回 `E_INTERNAL`。
    pub fn load(&self, session_id: &str) -> Result<Option<SessionSnapshot>, MeitoError> {
        if !is_uuid_v4(session_id) {
            return Err(MeitoError::new(
                ErrorCode::Usage,
                format!("会话 id 必须是 UUID v4，收到 {session_id}"),
            ));
        }
        let path = self.directory().join(format!("{}.json", session_id));
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).map_err(|error| {
            MeitoError::io(
                ErrorCode::Internal,
                format!("读取状态文件 {}", path.display()),
                &error,
            )
        })?;
        let value: Value = serde_json::from_str(&text).map_err(|error| {
            MeitoError::new(
                ErrorCode::Internal,
                format!("状态文件 {} 不是合法 JSON：{}", path.display(), error),
            )
        })?;
        Ok(Some(SessionSnapshot::from_json(&value)?))
    }

    /// 返回最近修改的会话快照。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 目录读取、元数据读取或快照解析失败时返回 `E_INTERNAL`。
    pub fn latest(&self) -> Result<Option<SessionSnapshot>, MeitoError> {
        let directory = self.directory();
        if !directory.exists() {
            return Ok(None);
        }
        let entries = fs::read_dir(&directory).map_err(|error| {
            MeitoError::io(
                ErrorCode::Internal,
                format!("读取状态目录 {}", directory.display()),
                &error,
            )
        })?;
        let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
        for entry in entries {
            let entry = entry.map_err(|error| {
                MeitoError::io(
                    ErrorCode::Internal,
                    format!("遍历状态目录 {}", directory.display()),
                    &error,
                )
            })?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|error| {
                    MeitoError::io(
                        ErrorCode::Internal,
                        format!("读取状态文件元数据 {}", path.display()),
                        &error,
                    )
                })?;
            let replace = match &newest {
                Some((current, _)) => modified > *current,
                None => true,
            };
            if replace {
                newest = Some((modified, path));
            }
        }
        match newest {
            Some((_, path)) => {
                let text = fs::read_to_string(&path).map_err(|error| {
                    MeitoError::io(
                        ErrorCode::Internal,
                        format!("读取状态文件 {}", path.display()),
                        &error,
                    )
                })?;
                let value: Value = serde_json::from_str(&text).map_err(|error| {
                    MeitoError::new(
                        ErrorCode::Internal,
                        format!("状态文件 {} 不是合法 JSON：{}", path.display(), error),
                    )
                })?;
                let snapshot = SessionSnapshot::from_json(&value)?;
                Ok(Some(snapshot))
            }
            None => Ok(None),
        }
    }

    /// 列出全部会话 id，供 doctor 深检使用。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 目录读取失败时返回 `E_INTERNAL`。
    pub fn list_ids(&self) -> Result<Vec<String>, MeitoError> {
        let directory = self.directory();
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&directory).map_err(|error| {
            MeitoError::io(
                ErrorCode::Internal,
                format!("读取状态目录 {}", directory.display()),
                &error,
            )
        })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                MeitoError::io(
                    ErrorCode::Internal,
                    format!("遍历状态目录 {}", directory.display()),
                    &error,
                )
            })?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                ids.push(stem.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }
}
