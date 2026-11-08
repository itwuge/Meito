//! File: `contracts/link.json` 加载与校验。
//!
//! 协调服务的所有数值都必须来自契约；本模块只做读取与结构校验，缺失、类型错误或取值
//! 非法统一返回 `E_CONFIG`（退出码 2）。契约文件缺失同样报错，绝不使用内置默认值。
//! 字段名在 `link.json` 0.3.0 落地后以该文件为准；此处的键名是双方已确认的语义集合。

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use meito_core::{ErrorCode, MeitoError};

// ===== 契约常量 =====

/// 仓库内的协调服务契约路径。
pub const LINK_CONTRACT_PATH: &str = "contracts/link.json";

/// 通道 id 在注册请求中的字段名；必须由契约声明。
const FIELD_CHANNEL_ID: &str = "channel_id";

/// 读 token 哈希在注册请求中的字段名。
const FIELD_READ_TOKEN_HASH: &str = "read_token_hash";

/// 写 token 哈希在注册请求中的字段名。
const FIELD_WRITE_TOKEN_HASH: &str = "write_token_hash";

/// 一次性公钥在注册请求中的字段名。
const FIELD_PUBLIC_KEY: &str = "public_key";

// ===== 契约模型 =====

/// 令牌哈希策略；哈希只用于校验，不落日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenHashPolicy {
    /// 哈希算法，契约当前要求 `sha256`。
    pub algorithm: String,
    /// 编码方式，契约当前要求 `hex`。
    pub encoding: String,
    /// 契约必须显式声明哈希不写日志；为 `true` 时拒绝启动。
    pub logged: bool,
}

/// 注册请求必须携带的字段集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationPolicy {
    /// 契约声明的字段名；必须包含通道 id、读写两个 token 哈希与公钥。
    pub fields: Vec<String>,
}

/// 读取语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetSemanticsPolicy {
    /// 语义模式，契约当前要求 `repeatable-until-delete-or-ttl`。
    pub mode: String,
    /// 单条密文最多被读取的次数。
    pub max_reads: u32,
}

/// 限流策略；三个数值全部来自契约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// 单通道每分钟允许的请求数。
    pub per_channel_per_minute: u64,
    /// 单来源 IP 每分钟允许的请求数。
    pub per_ip_per_minute: u64,
    /// 允许的突发请求数。
    pub burst: u64,
}

/// HTTP 状态码映射；七个值全部来自契约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpStatusPolicy {
    /// 错误 token。
    pub bad_token: u16,
    /// 未知或已撤销通道。
    pub unknown_channel: u16,
    /// 密文超过大小上限。
    pub too_large: u16,
    /// 触发限流。
    pub rate_limited: u16,
    /// 方法不允许。
    pub method_not_allowed: u16,
    /// 请求格式错误。
    pub bad_request: u16,
    /// 服务端内部错误。
    pub internal_error: u16,
}

/// `contracts/link.json` 的强类型视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkContract {
    /// 契约版本；只要求非空，版本升级不改变读取逻辑。
    pub contract_version: String,
    /// 通道 TTL（秒）。
    pub channel_ttl_seconds: u64,
    /// 单包密文上限（字节）。
    pub max_ciphertext_bytes: usize,
    /// 令牌哈希策略。
    pub token_hash: TokenHashPolicy,
    /// 注册字段策略。
    pub registration: RegistrationPolicy,
    /// 读取语义。
    pub get_semantics: GetSemanticsPolicy,
    /// 限流策略。
    pub rate_limit: RateLimitPolicy,
    /// HTTP 状态码映射。
    pub http_status: HttpStatusPolicy,
}

// ===== 加载 =====

/// 从仓库路径加载 `contracts/link.json`。
///
/// # 参数
/// - `path`: 契约文件路径，调用方通常传 `LINK_CONTRACT_PATH`。
///
/// # 错误
/// 文件缺失、JSON 非法、必需字段缺失或取值非法时返回 `E_CONFIG`（退出码 2）。
pub fn load_link_contract(path: &Path) -> Result<LinkContract, MeitoError> {
    let text = fs::read_to_string(path).map_err(|error| {
        MeitoError::new(
            ErrorCode::Config,
            format!("协调服务契约 {} 不可读：{}", path.display(), error),
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        MeitoError::new(
            ErrorCode::Config,
            format!("协调服务契约 {} 不是合法 JSON：{}", path.display(), error),
        )
    })?;
    parse_contract(&value, path)
}

/// 校验并把 JSON 转换为强类型契约。
///
/// # 参数
/// - `value`: 已解析的 JSON。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 必需字段缺失、类型错误或取值非法时返回 `E_CONFIG`。
fn parse_contract(value: &Value, path: &Path) -> Result<LinkContract, MeitoError> {
    let root = require_object(value, "$", path)?;
    let contract_version = require_str(root, "contractVersion", "contractVersion", path)?;
    if contract_version.trim().is_empty() {
        return Err(config_error(path, "contractVersion 不得为空"));
    }
    let channel_ttl_seconds = require_u64(root, "channelTtlSeconds", "channelTtlSeconds", path)?;
    if channel_ttl_seconds == 0 {
        return Err(config_error(path, "channelTtlSeconds 必须大于 0"));
    }
    let max_ciphertext_bytes = require_u64(root, "maxCiphertextBytes", "maxCiphertextBytes", path)?;
    if max_ciphertext_bytes == 0 {
        return Err(config_error(path, "maxCiphertextBytes 必须大于 0"));
    }
    let max_ciphertext_bytes = usize::try_from(max_ciphertext_bytes).map_err(|error| {
        config_error(
            path,
            format!("maxCiphertextBytes 超出本机 usize 范围：{error}"),
        )
    })?;

    let token_hash = parse_token_hash(root, path)?;
    let registration = parse_registration(root, path)?;
    let get_semantics = parse_get_semantics(root, path)?;
    let rate_limit = parse_rate_limit(root, path)?;
    let http_status = parse_http_status(root, path)?;

    Ok(LinkContract {
        contract_version: contract_version.to_string(),
        channel_ttl_seconds,
        max_ciphertext_bytes,
        token_hash,
        registration,
        get_semantics,
        rate_limit,
        http_status,
    })
}

// ===== 分段校验 =====

/// 校验令牌哈希策略。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 字段缺失或违反已确认语义时返回 `E_CONFIG`。
fn parse_token_hash(root: &Map<String, Value>, path: &Path) -> Result<TokenHashPolicy, MeitoError> {
    let object = require_object_field(root, "tokenHash", "tokenHash", path)?;
    let algorithm = require_str(object, "algorithm", "tokenHash.algorithm", path)?;
    if algorithm != "sha256" {
        return Err(config_error(
            path,
            format!("tokenHash.algorithm 只接受 sha256，收到 {algorithm}"),
        ));
    }
    let encoding = require_str(object, "encoding", "tokenHash.encoding", path)?;
    if encoding != "hex" {
        return Err(config_error(
            path,
            format!("tokenHash.encoding 只接受 hex，收到 {encoding}"),
        ));
    }
    let logged = require_bool(object, "logged", "tokenHash.logged", path)?;
    if logged {
        return Err(config_error(
            path,
            "tokenHash.logged 必须为 false：哈希只用于校验，不得写日志",
        ));
    }
    Ok(TokenHashPolicy {
        algorithm: algorithm.to_string(),
        encoding: encoding.to_string(),
        logged,
    })
}

/// 校验注册字段集合；必须同时包含读、写两个 token 哈希。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 缺少字段或缺少读写哈希时返回 `E_CONFIG`。
fn parse_registration(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<RegistrationPolicy, MeitoError> {
    let object = require_object_field(root, "registration", "registration", path)?;
    let fields = require_string_array(object, "fields", "registration.fields", path)?;
    for required in [
        FIELD_CHANNEL_ID,
        FIELD_READ_TOKEN_HASH,
        FIELD_WRITE_TOKEN_HASH,
        FIELD_PUBLIC_KEY,
    ] {
        if !fields.iter().any(|field| field == required) {
            return Err(config_error(
                path,
                format!("registration.fields 必须包含 {required}"),
            ));
        }
    }
    Ok(RegistrationPolicy { fields })
}

/// 校验读取语义。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 模式或最大读取次数非法时返回 `E_CONFIG`。
fn parse_get_semantics(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<GetSemanticsPolicy, MeitoError> {
    let object = require_object_field(root, "getSemantics", "getSemantics", path)?;
    let mode = require_str(object, "mode", "getSemantics.mode", path)?;
    if mode != "repeatable-until-delete-or-ttl" {
        return Err(config_error(
            path,
            format!("getSemantics.mode 只接受 repeatable-until-delete-or-ttl，收到 {mode}"),
        ));
    }
    let max_reads = require_u64(object, "maxReads", "getSemantics.maxReads", path)?;
    if max_reads == 0 {
        return Err(config_error(path, "getSemantics.maxReads 必须大于 0"));
    }
    let max_reads = u32::try_from(max_reads).map_err(|error| {
        config_error(
            path,
            format!("getSemantics.maxReads 超出 u32 范围：{error}"),
        )
    })?;
    Ok(GetSemanticsPolicy {
        mode: mode.to_string(),
        max_reads,
    })
}

/// 校验限流策略。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 任一数值缺失或为 0 时返回 `E_CONFIG`。
fn parse_rate_limit(root: &Map<String, Value>, path: &Path) -> Result<RateLimitPolicy, MeitoError> {
    let object = require_object_field(root, "rateLimit", "rateLimit", path)?;
    let per_channel_per_minute = require_u64(
        object,
        "perChannelPerMinute",
        "rateLimit.perChannelPerMinute",
        path,
    )?;
    let per_ip_per_minute =
        require_u64(object, "perIpPerMinute", "rateLimit.perIpPerMinute", path)?;
    let burst = require_u64(object, "burst", "rateLimit.burst", path)?;
    if per_channel_per_minute == 0 || per_ip_per_minute == 0 || burst == 0 {
        return Err(config_error(path, "rateLimit 的数值必须全部大于 0"));
    }
    Ok(RateLimitPolicy {
        per_channel_per_minute,
        per_ip_per_minute,
        burst,
    })
}

/// 校验 HTTP 状态码映射。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 任一状态码缺失或不在 100–599 时返回 `E_CONFIG`。
fn parse_http_status(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<HttpStatusPolicy, MeitoError> {
    let object = require_object_field(root, "httpStatus", "httpStatus", path)?;
    Ok(HttpStatusPolicy {
        bad_token: require_status(object, "badToken", "httpStatus.badToken", path)?,
        unknown_channel: require_status(
            object,
            "unknownChannel",
            "httpStatus.unknownChannel",
            path,
        )?,
        too_large: require_status(object, "tooLarge", "httpStatus.tooLarge", path)?,
        rate_limited: require_status(object, "rateLimited", "httpStatus.rateLimited", path)?,
        method_not_allowed: require_status(
            object,
            "methodNotAllowed",
            "httpStatus.methodNotAllowed",
            path,
        )?,
        bad_request: require_status(object, "badRequest", "httpStatus.badRequest", path)?,
        internal_error: require_status(object, "internalError", "httpStatus.internalError", path)?,
    })
}

// ===== 取值助手 =====

/// 构造带契约路径的 `E_CONFIG`。
///
/// # 参数
/// - `path`: 契约文件路径。
/// - `detail`: 具体原因。
///
/// # 错误
/// 不返回错误。
fn config_error(path: &Path, detail: impl AsRef<str>) -> MeitoError {
    MeitoError::new(
        ErrorCode::Config,
        format!("协调服务契约 {} 非法：{}", path.display(), detail.as_ref()),
    )
}

/// 把值当对象。
///
/// # 参数
/// - `value`: 待检查的值。
/// - `name`: 字段全名。
/// - `path`: 契约文件路径。
///
/// # 错误
/// 类型不是对象时返回 `E_CONFIG`。
fn require_object<'a>(
    value: &'a Value,
    name: &str,
    path: &Path,
) -> Result<&'a Map<String, Value>, MeitoError> {
    value
        .as_object()
        .ok_or_else(|| config_error(path, format!("{name} 必须是对象")))
}

/// 读取对象字段并当作对象。
///
/// # 参数
/// - `object`: 父对象。
/// - `key`: 字段名。
/// - `name`: 字段全名。
/// - `path`: 契约文件路径。
///
/// # 错误
/// 缺失或类型错误时返回 `E_CONFIG`。
fn require_object_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    name: &str,
    path: &Path,
) -> Result<&'a Map<String, Value>, MeitoError> {
    let value = object
        .get(key)
        .ok_or_else(|| config_error(path, format!("缺少必需字段 {name}")))?;
    require_object(value, name, path)
}

/// 读取字符串字段。
///
/// # 参数
/// - `object`: 父对象。
/// - `key`: 字段名。
/// - `name`: 字段全名。
/// - `path`: 契约文件路径。
///
/// # 错误
/// 缺失或不是字符串时返回 `E_CONFIG`。
fn require_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    name: &str,
    path: &Path,
) -> Result<&'a str, MeitoError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| config_error(path, format!("{name} 必须是非空字符串")))
}

/// 读取非负整数字段。
///
/// # 参数
/// - `object`: 父对象。
/// - `key`: 字段名。
/// - `name`: 字段全名。
/// - `path`: 契约文件路径。
///
/// # 错误
/// 缺失或不是无符号整数时返回 `E_CONFIG`。
fn require_u64(
    object: &Map<String, Value>,
    key: &str,
    name: &str,
    path: &Path,
) -> Result<u64, MeitoError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| config_error(path, format!("{name} 必须是非负整数")))
}

/// 读取布尔字段。
///
/// # 参数
/// - `object`: 父对象。
/// - `key`: 字段名。
/// - `name`: 字段全名。
/// - `path`: 契约文件路径。
///
/// # 错误
/// 缺失或不是布尔值时返回 `E_CONFIG`。
fn require_bool(
    object: &Map<String, Value>,
    key: &str,
    name: &str,
    path: &Path,
) -> Result<bool, MeitoError> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| config_error(path, format!("{name} 必须是布尔值")))
}

/// 读取字符串数组字段。
///
/// # 参数
/// - `object`: 父对象。
/// - `key`: 字段名。
/// - `name`: 字段全名。
/// - `path`: 契约文件路径。
///
/// # 错误
/// 缺失、不是数组或元素不是字符串时返回 `E_CONFIG`。
fn require_string_array(
    object: &Map<String, Value>,
    key: &str,
    name: &str,
    path: &Path,
) -> Result<Vec<String>, MeitoError> {
    let array = object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| config_error(path, format!("{name} 必须是字符串数组")))?;
    let mut values = Vec::with_capacity(array.len());
    for item in array {
        let text = item
            .as_str()
            .ok_or_else(|| config_error(path, format!("{name} 的元素必须是字符串")))?;
        values.push(text.to_string());
    }
    Ok(values)
}

/// 读取合法的 HTTP 状态码。
///
/// # 参数
/// - `object`: 父对象。
/// - `key`: 字段名。
/// - `name`: 字段全名。
/// - `path`: 契约文件路径。
///
/// # 错误
/// 缺失、超出 u16 或不在 100–599 时返回 `E_CONFIG`。
fn require_status(
    object: &Map<String, Value>,
    key: &str,
    name: &str,
    path: &Path,
) -> Result<u16, MeitoError> {
    let value = require_u64(object, key, name, path)?;
    let status = u16::try_from(value)
        .map_err(|error| config_error(path, format!("{name} 超出 u16 范围：{error}")))?;
    if !(100..=599).contains(&status) {
        return Err(config_error(
            path,
            format!("{name} 必须是 100–599 的 HTTP 状态码，收到 {status}"),
        ));
    }
    Ok(status)
}
