//! File: tools.lock.json 加载与校验。
//!
//! 远端引导只允许安装锁定清单里状态为 `pinned` 的工具；任何 `unpinned` 条目都必须在
//! 安装前被 `E_UNPINNED_TOOL` 拒绝，避免退化成 `curl | bash`。清单缺失或结构非法属于
//! 配置错误，统一走 `E_CONFIG`，与 `contracts/tools-lock.schema.json` 对齐。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::error::{ErrorCode, MeitoError};

// ===== 清单常量 =====

/// 仓库根目录下的锁定清单文件名。
pub const TOOLS_LOCK_PATH: &str = "tools.lock.json";

/// 当前唯一受支持的清单版本；schema 用 enum 固定为 1，更高或更低版本都必须拒绝。
const SUPPORTED_LOCK_VERSION: u64 = 1;

// ===== 条目状态 =====

/// 工具条目的钉死状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinStatus {
    /// 版本、URL 与 SHA256 均已钉死，允许安装。
    Pinned,
    /// 尚未钉死，任何安装请求都必须失败。
    Unpinned,
}

impl PinStatus {
    /// 返回清单中使用的稳定字符串。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Unpinned => "unpinned",
        }
    }

    /// 解析清单中的状态字符串。
    ///
    /// # 参数
    /// - `value`: `pinned` 或 `unpinned`。
    ///
    /// # 错误
    /// 未知值返回 `None`；调用方必须报 `E_CONFIG`，不得默认放行。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pinned" => Some(Self::Pinned),
            "unpinned" => Some(Self::Unpinned),
            _ => None,
        }
    }
}

// ===== 工具条目 =====

/// 单个工具的锁定信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPin {
    name: String,
    version: Option<String>,
    url: Option<String>,
    sha256: Option<String>,
    status: PinStatus,
}

impl ToolPin {
    /// 返回工具名（清单键）。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 返回钉死版本；`unpinned` 条目为 `None`。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// 返回下载地址；`unpinned` 条目为 `None`。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// 返回小写或大写十六进制的 SHA256；`unpinned` 条目为 `None`。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    /// 返回钉死状态。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn status(&self) -> PinStatus {
        self.status
    }

    /// 判断条目是否允许安装。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn is_pinned(&self) -> bool {
        self.status == PinStatus::Pinned
    }
}

// ===== 锁定清单 =====

/// 已解析并通过校验的锁定清单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolsLock {
    lock_version: u64,
    tools: BTreeMap<String, ToolPin>,
}

impl ToolsLock {
    /// 返回清单版本号。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn lock_version(&self) -> u64 {
        self.lock_version
    }

    /// 按名称查找条目。
    ///
    /// # 参数
    /// - `name`: 清单键，例如 `frpc`。
    ///
    /// # 错误
    /// 不返回错误；缺失时返回 `None`，安装前应由 `require_pinned` 拒绝。
    pub fn get(&self, name: &str) -> Option<&ToolPin> {
        self.tools.get(name)
    }

    /// 按名称取得允许安装的条目。
    ///
    /// # 参数
    /// - `name`: 清单键，例如 `frpc`。
    ///
    /// # 错误
    /// 条目缺失返回 `E_CONFIG`（退出码 2，清单非法）；状态为 `unpinned` 时返回
    /// `E_UNPINNED_TOOL`（退出码 4），拒绝安装。
    pub fn require_pinned(&self, name: &str) -> Result<&ToolPin, MeitoError> {
        let entry = self.tools.get(name).ok_or_else(|| {
            MeitoError::new(
                ErrorCode::Config,
                format!("tools.lock.json 缺少工具 {name}"),
            )
        })?;
        if !entry.is_pinned() {
            return Err(MeitoError::new(
                ErrorCode::UnpinnedTool,
                format!("工具 {name} 状态为 unpinned，拒绝安装"),
            ));
        }
        Ok(entry)
    }

    /// 遍历全部条目，按名称升序。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn entries(&self) -> impl Iterator<Item = &ToolPin> {
        self.tools.values()
    }
}

// ===== 解析 =====

/// 从仓库根目录读取并校验 `tools.lock.json`。
///
/// # 参数
/// - `root`: 仓库根目录，清单必须位于该目录下。
///
/// # 错误
/// 文件缺失、JSON 非法、版本不受支持、条目缺字段或 SHA256 形状非法时返回
/// `E_CONFIG`（退出码 4）；不会回退到任何默认版本。
pub fn load_tools_lock(root: &Path) -> Result<ToolsLock, MeitoError> {
    let path = root.join(TOOLS_LOCK_PATH);
    let text = fs::read_to_string(&path).map_err(|error| {
        MeitoError::io(
            ErrorCode::Config,
            format!("读取锁定清单 {}", path.display()),
            &error,
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        MeitoError::new(
            ErrorCode::Config,
            format!("解析锁定清单 {} 失败：{error}", path.display()),
        )
    })?;
    parse_tools_lock(&value, &path.display().to_string())
}

/// 校验 JSON 结构并构造清单。
///
/// # 参数
/// - `value`: 已解析的 JSON。
/// - `origin`: 出错时用于定位的文件名。
///
/// # 错误
/// 结构、版本或条目字段不符合约定时返回 `E_CONFIG`。
fn parse_tools_lock(value: &Value, origin: &str) -> Result<ToolsLock, MeitoError> {
    let object = value.as_object().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("锁定清单 {origin} 顶层必须是对象"),
        )
    })?;

    let lock_version = object
        .get("lockVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            MeitoError::new(
                ErrorCode::Config,
                format!("锁定清单 {origin} 缺少整数 lockVersion"),
            )
        })?;
    // schema 用 enum 固定为 1；不接受 0 或更高版本，避免误读不存在的语义。
    if lock_version != SUPPORTED_LOCK_VERSION {
        return Err(MeitoError::new(
            ErrorCode::Config,
            format!("锁定清单 {origin} 版本 {lock_version} 不受支持"),
        ));
    }

    let tools = object
        .get("tools")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            MeitoError::new(
                ErrorCode::Config,
                format!("锁定清单 {origin} 缺少 tools 对象"),
            )
        })?;
    if tools.is_empty() {
        return Err(MeitoError::new(
            ErrorCode::Config,
            format!("锁定清单 {origin} 的 tools 为空，无可安装工具"),
        ));
    }

    let mut parsed = BTreeMap::new();
    for (name, entry) in tools {
        let pin = parse_entry(name, entry, origin)?;
        parsed.insert(name.clone(), pin);
    }

    Ok(ToolsLock {
        lock_version,
        tools: parsed,
    })
}

/// 校验单个工具条目。
///
/// # 参数
/// - `name`: 清单键。
/// - `value`: 条目 JSON。
/// - `origin`: 出错时用于定位的文件名。
///
/// # 错误
/// 状态未知、`pinned` 条目缺版本/URL/SHA256，或 SHA256 不是 64 位十六进制时返回
/// `E_CONFIG`。
fn parse_entry(name: &str, value: &Value, origin: &str) -> Result<ToolPin, MeitoError> {
    let object = value.as_object().ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("锁定清单 {origin} 的工具 {name} 必须是对象"),
        )
    })?;

    let status_text = object
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            MeitoError::new(
                ErrorCode::Config,
                format!("锁定清单 {origin} 的工具 {name} 缺少 status"),
            )
        })?;
    let status = PinStatus::parse(status_text).ok_or_else(|| {
        MeitoError::new(
            ErrorCode::Config,
            format!("锁定清单 {origin} 的工具 {name} 状态 {status_text} 未知"),
        )
    })?;

    let version = optional_text(object, "version", name, origin)?;
    let url = optional_text(object, "url", name, origin)?;
    let sha256 = optional_text(object, "sha256", name, origin)?;

    // pinned 是安装许可，三个字段缺一不可；unpinned 只占位，不要求字段。
    if status == PinStatus::Pinned {
        require_field(&version, "version", name, origin)?;
        require_field(&url, "url", name, origin)?;
        let hash = require_field(&sha256, "sha256", name, origin)?;
        if !is_sha256_hex(hash) {
            return Err(MeitoError::new(
                ErrorCode::Config,
                format!("锁定清单 {origin} 的工具 {name} 的 sha256 不是 64 位十六进制"),
            ));
        }
    }

    Ok(ToolPin {
        name: name.to_string(),
        version,
        url,
        sha256,
        status,
    })
}

/// 读取可选字符串字段，显式 `null` 视为缺失。
///
/// # 参数
/// - `object`: 工具条目对象。
/// - `field`: 字段名。
/// - `name`: 工具名，用于错误上下文。
/// - `origin`: 文件名，用于错误上下文。
///
/// # 错误
/// 字段存在但不是字符串也不是 `null` 时返回 `E_CONFIG`。
fn optional_text(
    object: &serde_json::Map<String, Value>,
    field: &str,
    name: &str,
    origin: &str,
) -> Result<Option<String>, MeitoError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(MeitoError::new(
            ErrorCode::Config,
            format!("锁定清单 {origin} 的工具 {name} 的 {field} 必须是字符串或 null"),
        )),
    }
}

/// 断言字段非空。
///
/// # 参数
/// - `value`: 已读取的可选字段。
/// - `field`: 字段名。
/// - `name`: 工具名，用于错误上下文。
/// - `origin`: 文件名，用于错误上下文。
///
/// # 错误
/// 字段缺失或为空字符串时返回 `E_CONFIG`。
fn require_field<'a>(
    value: &'a Option<String>,
    field: &str,
    name: &str,
    origin: &str,
) -> Result<&'a str, MeitoError> {
    match value.as_deref() {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(MeitoError::new(
            ErrorCode::Config,
            format!("锁定清单 {origin} 的 pinned 工具 {name} 缺少 {field}"),
        )),
    }
}

/// 判断字符串是否为 64 位小写十六进制 SHA256。
///
/// # 参数
/// - `value`: 待校验字符串。
///
/// # 错误
/// 不返回错误。
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|character| matches!(character, '0'..='9' | 'a'..='f'))
}
