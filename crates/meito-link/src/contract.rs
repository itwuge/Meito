//! File: `contracts/link.json` 加载与校验。
//!
//! 协调服务的所有数值都必须来自契约；本模块只做读取与结构校验，缺失、类型错误或取值
//! 非法统一返回 `E_CONFIG`（退出码 2）。契约文件缺失同样报错，绝不使用内置默认值。
//! 单包上限以引用形式声明，加载器必须解析 `connection-package.json#...`，不得复制数值。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use meito_core::{ErrorCode, MeitoError};

// ===== 契约常量 =====

/// 仓库内的协调服务契约路径。
pub const LINK_CONTRACT_PATH: &str = "contracts/link.json";

/// 注册请求必须携带的字段名；必须由契约声明。
const FIELD_CHANNEL_ID: &str = "channel_id";

/// 读 token 哈希在注册请求中的字段名。
const FIELD_READ_TOKEN_HASH: &str = "read_token_hash";

/// 写 token 哈希在注册请求中的字段名。
const FIELD_WRITE_TOKEN_HASH: &str = "write_token_hash";

/// 一次性公钥在注册请求中的字段名。
const FIELD_PUBLIC_KEY: &str = "public_key";

/// 日志白名单字段；多一个或少一个都视为契约非法。
const LOG_ALLOWED_FIELDS: [&str; 3] = ["channel_id", "size", "status"];

/// 日志禁止内容；任一缺失都视为契约非法。
const LOG_FORBIDDEN_CONTENT: [&str; 5] = ["blob", "ciphertext", "digest", "hash", "sha256"];

// ===== 契约模型 =====

/// 通道 id 策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelIdPolicy {
    /// 服务端不得解析 id 的业务含义。
    pub opaque: bool,
    /// 最大字节长度。
    pub max_length: usize,
    /// 服务端不得按 id 内部结构做路由。
    pub parsed_by_server: bool,
}

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

/// 读写 token 的签发语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenIssuancePolicy {
    /// 读 token 签发方式。
    pub read_issuance: String,
    /// 写 token 签发方式。
    pub write_issuance: String,
    /// 读 token 是否只能使用一次；契约要求 `false`（每通道签发一次，可多次读取）。
    pub read_one_time: bool,
}

/// 注册请求必须携带的字段集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterPayloadPolicy {
    /// 契约声明的必需字段名。
    pub required: Vec<String>,
}

/// 单包上限策略；数值通过引用解析，不在本文件复写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageLimitPolicy {
    /// 原始引用表达式。
    pub max_ciphertext_bytes_ref: String,
    /// 解析引用后得到的上限（字节）。
    pub max_ciphertext_bytes: usize,
}

/// 读取语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetSemanticsPolicy {
    /// 允许重复读取的终止条件；必须包含 `delete` 与 `ttl`。
    pub repeatable_until: Vec<String>,
    /// 单条密文最多被读取的次数。
    pub max_reads: u32,
    /// 读 token 的含义说明。
    pub read_token_meaning: String,
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

/// 鉴权头策略；请求头名称与方案由契约冻结。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthHeaderPolicy {
    /// 请求头名称，例如 `Authorization`。
    pub name: String,
    /// 鉴权方案，例如 `Bearer`。
    pub scheme: String,
    /// 需要携带鉴权头的操作名。
    pub applies_to: Vec<String>,
}

/// 单个操作使用的鉴权角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationAuth {
    /// 不携带令牌。
    None,
    /// 使用读 token。
    ReadToken,
    /// 使用写 token。
    WriteToken,
}

impl OperationAuth {
    /// 返回该角色是否要求写 token。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn uses_write_token(&self) -> bool {
        matches!(self, Self::WriteToken)
    }
}

/// 单个操作的策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationPolicy {
    /// HTTP 方法。
    pub method: String,
    /// 鉴权角色。
    pub auth: OperationAuth,
}

/// 操作清单与重复注册语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsPolicy {
    /// 注册操作。
    pub register: OperationPolicy,
    /// 写入操作。
    pub put: OperationPolicy,
    /// 读取操作。
    pub get: OperationPolicy,
    /// 撤销操作。
    pub delete: OperationPolicy,
    /// 存活通道重复注册时使用的状态键。
    pub duplicate_live_channel: String,
}

/// HTTP 状态码映射；九个值全部来自契约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpStatusPolicy {
    /// 成功响应。
    pub success: u16,
    /// 存活通道重复注册。
    pub duplicate_channel: u16,
    /// 错误 token。
    pub bad_token: u16,
    /// 未知或已撤销通道。
    pub unknown_channel: u16,
    /// 密文超过大小上限。
    pub too_large: u16,
    /// 触发限流或读取次数耗尽。
    pub rate_limited: u16,
    /// 方法不允许。
    pub method_not_allowed: u16,
    /// 报文非法。
    pub malformed: u16,
    /// 服务端内部错误。
    pub internal: u16,
}

/// 日志禁项策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingPolicy {
    /// 允许写入访问日志的字段。
    pub allowed_fields: Vec<String>,
    /// 是否只允许白名单字段。
    pub only_allowed_fields: bool,
    /// 禁止出现在日志中的内容关键字。
    pub forbidden_content: Vec<String>,
    /// blob 是否允许写日志；契约要求 `false`。
    pub blob_logged: bool,
    /// 摘要是否允许写日志；契约要求 `false`。
    pub hash_logged: bool,
}

/// `contracts/link.json` 的强类型视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkContract {
    /// 契约版本；只要求非空，版本升级不改变读取逻辑。
    pub contract_version: String,
    /// 通道 id 策略。
    pub channel_id: ChannelIdPolicy,
    /// 通道 TTL（秒）。
    pub channel_ttl_seconds: u64,
    /// 令牌哈希策略。
    pub token_hash: TokenHashPolicy,
    /// 读写 token 签发语义。
    pub tokens: TokenIssuancePolicy,
    /// 注册字段策略。
    pub register_payload: RegisterPayloadPolicy,
    /// 单包上限（含引用解析结果）。
    pub package_limit: PackageLimitPolicy,
    /// 读取语义。
    pub get_semantics: GetSemanticsPolicy,
    /// 限流策略。
    pub rate_limit: RateLimitPolicy,
    /// 鉴权头策略。
    pub auth_header: AuthHeaderPolicy,
    /// 操作与重复注册策略。
    pub operations: OperationsPolicy,
    /// HTTP 状态码映射。
    pub http_status: HttpStatusPolicy,
    /// 日志禁项。
    pub logging: LoggingPolicy,
}

// ===== 加载入口 =====

/// 从仓库路径加载 `contracts/link.json` 并解析全部引用。
///
/// # 参数
/// - `path`: 契约文件路径，调用方通常传 `LINK_CONTRACT_PATH`。
///
/// # 错误
/// 文件缺失、JSON 非法、必需字段缺失、引用不可解析或取值非法时返回 `E_CONFIG`（退出码 2）。
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
/// - `path`: 仅用于错误信息与引用解析基准目录。
///
/// # 错误
/// 必需字段缺失、引用不可解析或取值非法时返回 `E_CONFIG`。
fn parse_contract(value: &Value, path: &Path) -> Result<LinkContract, MeitoError> {
    let root = require_object(value, "$", path)?;
    let contract_version = require_str(root, "contractVersion", "contractVersion", path)?;
    if contract_version.trim().is_empty() {
        return Err(config_error(path, "contractVersion 不得为空"));
    }
    let channel_id = parse_channel_id(root, path)?;
    let channel_ttl_seconds = require_u64(root, "channelTtlSeconds", "channelTtlSeconds", path)?;
    if channel_ttl_seconds == 0 {
        return Err(config_error(path, "channelTtlSeconds 必须大于 0"));
    }
    let token_hash = parse_token_hash(root, path)?;
    let tokens = parse_tokens(root, path)?;
    let register_payload = parse_register_payload(root, path)?;
    let package_limit = parse_package_limit(root, path)?;
    let get_semantics = parse_get_semantics(root, path)?;
    let rate_limit = parse_rate_limit(root, path)?;
    let auth_header = parse_auth_header(root, path)?;
    let operations = parse_operations(root, path)?;
    let http_status = parse_http_status(root, path)?;
    let logging = parse_logging(root, path)?;

    Ok(LinkContract {
        contract_version: contract_version.to_string(),
        channel_id,
        channel_ttl_seconds,
        token_hash,
        tokens,
        register_payload,
        package_limit,
        get_semantics,
        rate_limit,
        auth_header,
        operations,
        http_status,
        logging,
    })
}

// ===== 分段校验 =====

/// 校验通道 id 策略。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 字段缺失或违反不透明约束时返回 `E_CONFIG`。
fn parse_channel_id(root: &Map<String, Value>, path: &Path) -> Result<ChannelIdPolicy, MeitoError> {
    let object = require_object_field(root, "channelId", "channelId", path)?;
    let opaque = require_bool(object, "opaque", "channelId.opaque", path)?;
    let parsed_by_server =
        require_bool(object, "parsedByServer", "channelId.parsedByServer", path)?;
    if !opaque || parsed_by_server {
        return Err(config_error(
            path,
            "channelId 必须 opaque=true 且 parsedByServer=false",
        ));
    }
    let max_length = require_u64(object, "maxLength", "channelId.maxLength", path)?;
    if max_length == 0 {
        return Err(config_error(path, "channelId.maxLength 必须大于 0"));
    }
    let max_length = usize::try_from(max_length).map_err(|error| {
        config_error(
            path,
            format!("channelId.maxLength 超出本机 usize 范围：{error}"),
        )
    })?;
    Ok(ChannelIdPolicy {
        opaque,
        max_length,
        parsed_by_server,
    })
}

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

/// 校验读写 token 的签发语义。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 签发方式或一次性语义不符合契约时返回 `E_CONFIG`。
fn parse_tokens(root: &Map<String, Value>, path: &Path) -> Result<TokenIssuancePolicy, MeitoError> {
    let object = require_object_field(root, "tokens", "tokens", path)?;
    let read_issuance = require_str(
        object,
        "readTokenIssuance",
        "tokens.readTokenIssuance",
        path,
    )?;
    let write_issuance = require_str(
        object,
        "writeTokenIssuance",
        "tokens.writeTokenIssuance",
        path,
    )?;
    if read_issuance != "once-per-channel" || write_issuance != "once-per-channel" {
        return Err(config_error(path, "读写 token 都必须 once-per-channel"));
    }
    let read_one_time = require_bool(object, "readTokenOneTime", "tokens.readTokenOneTime", path)?;
    if read_one_time {
        return Err(config_error(
            path,
            "tokens.readTokenOneTime 必须为 false：读 token 每通道签发一次，可重复读取",
        ));
    }
    Ok(TokenIssuancePolicy {
        read_issuance: read_issuance.to_string(),
        write_issuance: write_issuance.to_string(),
        read_one_time,
    })
}

/// 校验注册必需字段。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 缺少任一必需字段时返回 `E_CONFIG`。
fn parse_register_payload(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<RegisterPayloadPolicy, MeitoError> {
    let object = require_object_field(root, "registerPayload", "registerPayload", path)?;
    let required = require_string_array(object, "required", "registerPayload.required", path)?;
    for field in [
        FIELD_CHANNEL_ID,
        FIELD_READ_TOKEN_HASH,
        FIELD_WRITE_TOKEN_HASH,
        FIELD_PUBLIC_KEY,
    ] {
        if !required.iter().any(|item| item == field) {
            return Err(config_error(
                path,
                format!("registerPayload.required 必须包含 {field}"),
            ));
        }
    }
    Ok(RegisterPayloadPolicy { required })
}

/// 校验单包上限并解析引用。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 契约文件路径，用于定位同目录的被引用文件。
///
/// # 错误
/// 引用缺失、目标文件不可读或目标字段不是正整数时返回 `E_CONFIG`。
fn parse_package_limit(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<PackageLimitPolicy, MeitoError> {
    let object = require_object_field(root, "packageLimit", "packageLimit", path)?;
    if object.contains_key("maxCiphertextBytes") {
        return Err(config_error(
            path,
            "packageLimit 不得复写 maxCiphertextBytes，只能使用引用",
        ));
    }
    let reference = require_str(
        object,
        "maxCiphertextBytesRef",
        "packageLimit.maxCiphertextBytesRef",
        path,
    )?;
    let resolved = resolve_reference(path, reference, "packageLimit.maxCiphertextBytesRef")?;
    let value = resolved
        .as_u64()
        .ok_or_else(|| config_error(path, format!("引用 {reference} 的目标必须是非负整数")))?;
    if value == 0 {
        return Err(config_error(
            path,
            format!("引用 {reference} 的目标必须大于 0"),
        ));
    }
    let max_ciphertext_bytes = usize::try_from(value).map_err(|error| {
        config_error(
            path,
            format!("引用 {reference} 超出本机 usize 范围：{error}"),
        )
    })?;
    Ok(PackageLimitPolicy {
        max_ciphertext_bytes_ref: reference.to_string(),
        max_ciphertext_bytes,
    })
}

/// 解析 `文件名#点分路径` 形式的契约引用。
///
/// # 参数
/// - `contract_path`: 当前契约文件路径。
/// - `reference`: 引用表达式，例如 `connection-package.json#serverHandling.maxCiphertextBytes`。
/// - `field`: 引用所在字段名，仅用于错误信息。
///
/// # 错误
/// 引用格式错误、目标文件不可读、JSON 非法或路径不存在时返回 `E_CONFIG`。
fn resolve_reference(
    contract_path: &Path,
    reference: &str,
    field: &str,
) -> Result<Value, MeitoError> {
    let (file, pointer) = reference.split_once('#').ok_or_else(|| {
        config_error(
            contract_path,
            format!("{field} 必须是 文件名#点分路径 的引用，收到 {reference}"),
        )
    })?;
    if file.trim().is_empty() || pointer.trim().is_empty() {
        return Err(config_error(
            contract_path,
            format!("{field} 引用缺少文件名或路径：{reference}"),
        ));
    }
    let base: PathBuf = match contract_path.parent() {
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from("."),
    };
    let target_path = base.join(file);
    let text = fs::read_to_string(&target_path).map_err(|error| {
        config_error(
            contract_path,
            format!("引用目标 {} 不可读：{}", target_path.display(), error),
        )
    })?;
    let target: Value = serde_json::from_str(&text).map_err(|error| {
        config_error(
            contract_path,
            format!(
                "引用目标 {} 不是合法 JSON：{}",
                target_path.display(),
                error
            ),
        )
    })?;
    let mut current = &target;
    for segment in pointer.split('.') {
        current = current.get(segment).ok_or_else(|| {
            config_error(
                contract_path,
                format!("引用 {reference} 在 {} 中不存在", target_path.display()),
            )
        })?;
    }
    Ok(current.clone())
}

/// 校验读取语义。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 终止条件或最大读取次数非法时返回 `E_CONFIG`。
fn parse_get_semantics(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<GetSemanticsPolicy, MeitoError> {
    let object = require_object_field(root, "getSemantics", "getSemantics", path)?;
    let repeatable_until = require_string_array(
        object,
        "repeatableUntil",
        "getSemantics.repeatableUntil",
        path,
    )?;
    for until in ["delete", "ttl"] {
        if !repeatable_until.iter().any(|item| item == until) {
            return Err(config_error(
                path,
                format!("getSemantics.repeatableUntil 必须包含 {until}"),
            ));
        }
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
    let read_token_meaning = require_str(
        object,
        "readTokenMeaning",
        "getSemantics.readTokenMeaning",
        path,
    )?;
    Ok(GetSemanticsPolicy {
        repeatable_until,
        max_reads,
        read_token_meaning: read_token_meaning.to_string(),
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

/// 校验鉴权头策略。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 头名称或方案为空，或 `appliesTo` 缺少 PUT/GET/DELETE 时返回 `E_CONFIG`。
fn parse_auth_header(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<AuthHeaderPolicy, MeitoError> {
    let object = require_object_field(root, "authHeader", "authHeader", path)?;
    let name = require_str(object, "name", "authHeader.name", path)?;
    let scheme = require_str(object, "scheme", "authHeader.scheme", path)?;
    if name.trim().is_empty() || scheme.trim().is_empty() {
        return Err(config_error(path, "authHeader 的 name 与 scheme 不得为空"));
    }
    let applies_to = require_string_array(object, "appliesTo", "authHeader.appliesTo", path)?;
    for operation in ["put", "get", "delete"] {
        if !applies_to.iter().any(|item| item == operation) {
            return Err(config_error(
                path,
                format!("authHeader.appliesTo 缺少 {operation}"),
            ));
        }
    }
    Ok(AuthHeaderPolicy {
        name: name.to_string(),
        scheme: scheme.to_string(),
        applies_to,
    })
}

/// 校验操作清单与重复注册语义。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 方法、鉴权角色或重复注册键与冻结语义不一致时返回 `E_CONFIG`。
fn parse_operations(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<OperationsPolicy, MeitoError> {
    let object = require_object_field(root, "operations", "operations", path)?;
    let register_object = require_object_field(object, "register", "operations.register", path)?;
    let method = require_str(
        register_object,
        "method",
        "operations.register.method",
        path,
    )?;
    if method != "POST" {
        return Err(config_error(
            path,
            format!("operations.register.method 只接受 POST，收到 {method}"),
        ));
    }
    let auth_text = require_str(register_object, "auth", "operations.register.auth", path)?;
    let auth = parse_operation_auth(auth_text, "operations.register.auth", path)?;
    if auth != OperationAuth::None {
        return Err(config_error(path, "operations.register.auth 必须为 none"));
    }
    let duplicate_live_channel = require_str(
        register_object,
        "duplicateLiveChannel",
        "operations.register.duplicateLiveChannel",
        path,
    )?;
    if duplicate_live_channel != "duplicate_channel" {
        return Err(config_error(
            path,
            "operations.register.duplicateLiveChannel 必须为 duplicate_channel",
        ));
    }
    Ok(OperationsPolicy {
        register: OperationPolicy {
            method: method.to_string(),
            auth,
        },
        put: parse_operation(object, "put", "PUT", OperationAuth::WriteToken, path)?,
        get: parse_operation(object, "get", "GET", OperationAuth::ReadToken, path)?,
        delete: parse_operation(object, "delete", "DELETE", OperationAuth::WriteToken, path)?,
        duplicate_live_channel: duplicate_live_channel.to_string(),
    })
}

/// 校验单个操作的方法与鉴权角色。
///
/// # 参数
/// - `operations`: 契约 `operations` 对象。
/// - `key`: 操作名。
/// - `expected_method`: 该操作必须使用的方法。
/// - `expected_auth`: 该操作必须使用的鉴权角色。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 方法或鉴权角色与冻结语义不一致时返回 `E_CONFIG`。
fn parse_operation(
    operations: &Map<String, Value>,
    key: &str,
    expected_method: &str,
    expected_auth: OperationAuth,
    path: &Path,
) -> Result<OperationPolicy, MeitoError> {
    let name = format!("operations.{key}");
    let object = require_object_field(operations, key, &name, path)?;
    let method_name = format!("{name}.method");
    let method = require_str(object, "method", &method_name, path)?;
    if method != expected_method {
        return Err(config_error(
            path,
            format!("{method_name} 只接受 {expected_method}，收到 {method}"),
        ));
    }
    let auth_name = format!("{name}.auth");
    let auth_text = require_str(object, "auth", &auth_name, path)?;
    let auth = parse_operation_auth(auth_text, &auth_name, path)?;
    if auth != expected_auth {
        return Err(config_error(
            path,
            format!("{auth_name} 与冻结的鉴权角色不一致"),
        ));
    }
    Ok(OperationPolicy {
        method: method.to_string(),
        auth,
    })
}

/// 把契约字符串转换为鉴权角色。
///
/// # 参数
/// - `value`: `none`、`read_token` 或 `write_token`。
/// - `name`: 字段全名，用于错误信息。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 未知角色返回 `E_CONFIG`。
fn parse_operation_auth(value: &str, name: &str, path: &Path) -> Result<OperationAuth, MeitoError> {
    match value {
        "none" => Ok(OperationAuth::None),
        "read_token" => Ok(OperationAuth::ReadToken),
        "write_token" => Ok(OperationAuth::WriteToken),
        _ => Err(config_error(
            path,
            format!("{name} 只接受 none/read_token/write_token，收到 {value}"),
        )),
    }
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
    let object = require_object_field(root, "httpStatusMap", "httpStatusMap", path)?;
    Ok(HttpStatusPolicy {
        success: require_status(object, "success", "httpStatusMap.success", path)?,
        duplicate_channel: require_status(
            object,
            "duplicate_channel",
            "httpStatusMap.duplicate_channel",
            path,
        )?,
        bad_token: require_status(object, "bad_token", "httpStatusMap.bad_token", path)?,
        unknown_channel: require_status(
            object,
            "unknown_channel",
            "httpStatusMap.unknown_channel",
            path,
        )?,
        too_large: require_status(object, "too_large", "httpStatusMap.too_large", path)?,
        rate_limited: require_status(object, "rate_limited", "httpStatusMap.rate_limited", path)?,
        method_not_allowed: require_status(
            object,
            "method_not_allowed",
            "httpStatusMap.method_not_allowed",
            path,
        )?,
        malformed: require_status(object, "malformed", "httpStatusMap.malformed", path)?,
        internal: require_status(object, "internal", "httpStatusMap.internal", path)?,
    })
}

/// 校验日志禁项。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误信息。
///
/// # 错误
/// 白名单不完整、允许 blob/摘要写日志或禁项缺失时返回 `E_CONFIG`。
fn parse_logging(root: &Map<String, Value>, path: &Path) -> Result<LoggingPolicy, MeitoError> {
    let object = require_object_field(root, "logging", "logging", path)?;
    let allowed_fields =
        require_string_array(object, "allowedFields", "logging.allowedFields", path)?;
    for field in LOG_ALLOWED_FIELDS {
        if !allowed_fields.iter().any(|item| item == field) {
            return Err(config_error(
                path,
                format!("logging.allowedFields 必须包含 {field}"),
            ));
        }
    }
    let only_allowed_fields = require_bool(
        object,
        "onlyAllowedFields",
        "logging.onlyAllowedFields",
        path,
    )?;
    if !only_allowed_fields {
        return Err(config_error(path, "logging.onlyAllowedFields 必须为 true"));
    }
    let forbidden_content =
        require_string_array(object, "forbiddenContent", "logging.forbiddenContent", path)?;
    for word in LOG_FORBIDDEN_CONTENT {
        if !forbidden_content.iter().any(|item| item == word) {
            return Err(config_error(
                path,
                format!("logging.forbiddenContent 必须包含 {word}"),
            ));
        }
    }
    let blob_logged = require_bool(object, "blobLogged", "logging.blobLogged", path)?;
    let hash_logged = require_bool(object, "hashLogged", "logging.hashLogged", path)?;
    if blob_logged || hash_logged {
        return Err(config_error(
            path,
            "logging.blobLogged 与 logging.hashLogged 都必须为 false",
        ));
    }
    Ok(LoggingPolicy {
        allowed_fields,
        only_allowed_fields,
        forbidden_content,
        blob_logged,
        hash_logged,
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
