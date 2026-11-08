//! File: 连接包封装、解封与每作业密钥（contracts/connection-package.json）。
//!
//! 线格式、算法、magic、格式版本、布局偏移与单包上限全部从契约读取，代码不另写数值。
//! 私钥只存在于进程内，随 `StaticSecret` 离开作用域清零，不导出、不记录；明文只交给
//! 调用方，本模块不解析业务字段。

use std::fs;
use std::path::Path;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde_json::{Map, Value};
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

use crate::error::{ErrorCode, MeitoError};

// ===== 契约常量 =====

/// 连接包契约的仓库相对路径。
pub const CONNECTION_PACKAGE_CONTRACT_PATH: &str = "contracts/connection-package.json";

/// X25519 公钥的固定字节数；由算法本身决定，不属于契约数值。
const X25519_PUBLIC_KEY_BYTES: usize = 32;

/// XChaCha20-Poly1305 nonce 的固定字节数；由算法本身决定，不属于契约数值。
const XCHACHA_NONCE_BYTES: usize = 24;

/// 布局必须按此顺序声明；字段名属于线格式标识，不随契约版本变化。
const LAYOUT_NAMES: [&str; 6] = [
    "magic",
    "format_version",
    "algorithm_id",
    "ephemeral_public_key",
    "nonce",
    "ciphertext",
];

// ===== 数据模型 =====

/// 连接包布局计算出的字节偏移；全部由契约 `layout` 求和得到。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageLayout {
    /// magic 的字节数。
    pub magic_len: usize,
    /// 格式版本字段的偏移。
    pub format_version_offset: usize,
    /// 算法 id 字段的偏移。
    pub algorithm_id_offset: usize,
    /// 临时公钥字段的起始偏移。
    pub public_key_offset: usize,
    /// 临时公钥字段的字节数。
    pub public_key_len: usize,
    /// nonce 字段的起始偏移。
    pub nonce_offset: usize,
    /// nonce 字段的字节数。
    pub nonce_len: usize,
    /// 密文字段的起始偏移。
    pub ciphertext_offset: usize,
    /// 密文之前的固定头部总长度。
    pub header_len: usize,
}

/// `contracts/connection-package.json` 的强类型视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPackageContract {
    /// 容器起始 magic，按字节比较。
    pub magic: String,
    /// magic 的字节数，必须与 `magic` 的字节长度一致。
    pub magic_bytes: usize,
    /// 头部写入的格式版本。
    pub format_version: u8,
    /// 唯一支持的算法名。
    pub algorithm_id: String,
    /// 头部写入的算法 id 字节。
    pub algorithm_id_byte: u8,
    /// 密钥交换算法名；当前只支持 `x25519`。
    pub key_exchange: String,
    /// AEAD 算法名；当前只支持 `xchacha20-poly1305`。
    pub aead: String,
    /// 实现来源；当前只支持 `rustcrypto`。
    pub implementation: String,
    /// AAD 策略；当前只支持 `none`。
    pub aad: String,
    /// 明文顶层字段名，由上层负责填充与解释。
    pub plaintext_fields: Vec<String>,
    /// 密文字节上限，来自 `serverHandling.maxCiphertextBytes`。
    pub max_ciphertext_bytes: usize,
    /// 头部布局偏移。
    pub layout: PackageLayout,
}

/// 每作业临时密钥对；私钥不导出，离开作用域时由 `StaticSecret` 清零。
pub struct ConnectionKeyPair {
    secret: StaticSecret,
    public: PublicKey,
}

// ===== 契约加载 =====

/// 从仓库根目录加载并校验连接包契约。
///
/// # 参数
/// - `root`: 仓库根目录，契约必须位于 `root/contracts/connection-package.json`。
///
/// # 错误
/// 文件缺失、JSON 非法、字段缺失、算法或编码不受支持、布局与算法尺寸不符时返回
/// `E_CONFIG`（退出码 2）。
pub fn load_connection_package_contract(
    root: &Path,
) -> Result<ConnectionPackageContract, MeitoError> {
    let path = root.join(CONNECTION_PACKAGE_CONTRACT_PATH);
    let text = fs::read_to_string(&path).map_err(|error| {
        MeitoError::io(
            ErrorCode::Config,
            format!("读取连接包契约 {}", path.display()),
            &error,
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        MeitoError::new(
            ErrorCode::Config,
            format!("解析连接包契约 {} 失败：{error}", path.display()),
        )
    })?;
    parse_contract(&value, &path)
}

/// 校验根对象并把 JSON 转换为强类型契约。
///
/// # 参数
/// - `value`: 已解析的契约 JSON。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 任一必需字段缺失或取值不受支持时返回 `E_CONFIG`。
fn parse_contract(value: &Value, path: &Path) -> Result<ConnectionPackageContract, MeitoError> {
    let root = require_object(value, path)?;
    let magic = require_str(root, "magic", path)?.to_string();
    let magic_bytes = require_u64(root, "magicBytes", path)?;
    if magic.is_empty() {
        return Err(config_error(path, "magic 不得为空"));
    }
    let magic_len = usize::try_from(magic_bytes)
        .map_err(|error| config_error(path, format!("magicBytes 超出本机 usize 范围：{error}")))?;
    if magic.len() != magic_len {
        return Err(config_error(
            path,
            format!(
                "magicBytes={magic_bytes} 与 magic 的实际字节长度 {} 不一致",
                magic.len()
            ),
        ));
    }
    let format_version = to_u8(
        require_u64(root, "formatVersion", path)?,
        "formatVersion",
        path,
    )?;

    let algorithm = require_object_field(root, "algorithm", path)?;
    let algorithm_id = require_str(algorithm, "id", path)?.to_string();
    require_supported(
        &algorithm_id,
        "x25519-xchacha20poly1305",
        "algorithm.id",
        path,
    )?;
    let algorithm_id_byte = to_u8(
        require_u64(algorithm, "idByte", path)?,
        "algorithm.idByte",
        path,
    )?;
    let key_exchange = require_str(algorithm, "keyExchange", path)?.to_string();
    require_supported(&key_exchange, "x25519", "algorithm.keyExchange", path)?;
    let aead = require_str(algorithm, "aead", path)?.to_string();
    require_supported(&aead, "xchacha20-poly1305", "algorithm.aead", path)?;
    let implementation = require_str(algorithm, "implementation", path)?.to_string();
    require_supported(
        &implementation,
        "rustcrypto",
        "algorithm.implementation",
        path,
    )?;
    let aad = require_str(algorithm, "aad", path)?.to_string();
    require_supported(&aad, "none", "algorithm.aad", path)?;
    validate_algorithm_table(root, algorithm_id_byte, &algorithm_id, path)?;

    validate_encoding(root, path)?;
    let max_ciphertext_bytes = parse_server_handling(root, path)?;
    let plaintext_fields = parse_plaintext_fields(root, path)?;
    let layout = parse_layout(root, magic_bytes, path)?;

    Ok(ConnectionPackageContract {
        magic,
        magic_bytes: magic_len,
        format_version,
        algorithm_id,
        algorithm_id_byte,
        key_exchange,
        aead,
        implementation,
        aad,
        plaintext_fields,
        max_ciphertext_bytes,
        layout,
    })
}

// ===== 分段校验 =====

/// 校验算法 id 表必须包含与 `algorithm.id/idByte` 一致且唯一的一行。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `id_byte`: 头部写入的算法 id 字节。
/// - `id`: 算法名字。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 表缺失、条目非法、缺少匹配行或 id 重复时返回 `E_CONFIG`。
fn validate_algorithm_table(
    root: &Map<String, Value>,
    id_byte: u8,
    id: &str,
    path: &Path,
) -> Result<(), MeitoError> {
    let table = root
        .get("algorithmIds")
        .and_then(Value::as_array)
        .ok_or_else(|| config_error(path, "缺少 algorithmIds 数组"))?;
    if table.is_empty() {
        return Err(config_error(path, "algorithmIds 不得为空"));
    }
    let mut matched = false;
    let mut ids = Vec::with_capacity(table.len());
    for entry in table {
        let object = entry
            .as_object()
            .ok_or_else(|| config_error(path, "algorithmIds 条目必须是对象"))?;
        let entry_id = to_u8(require_u64(object, "id", path)?, "algorithmIds.id", path)?;
        let name = require_str(object, "name", path)?;
        if ids.contains(&entry_id) {
            return Err(config_error(
                path,
                format!("algorithmIds 中 id {entry_id} 重复"),
            ));
        }
        ids.push(entry_id);
        if entry_id == id_byte && name == id {
            matched = true;
        }
    }
    if !matched {
        return Err(config_error(
            path,
            format!("algorithmIds 缺少 id={id_byte} name={id} 的条目"),
        ));
    }
    Ok(())
}

/// 校验容器与编码字段；只接受本实现支持的一种线格式。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 字段缺失或取值与实现不匹配时返回 `E_CONFIG`。
fn validate_encoding(root: &Map<String, Value>, path: &Path) -> Result<(), MeitoError> {
    let encoding = require_object_field(root, "encoding", path)?;
    require_supported(
        require_str(encoding, "container", path)?,
        "binary",
        "encoding.container",
        path,
    )?;
    require_supported(
        require_str(encoding, "integerEncoding", path)?,
        "single-byte",
        "encoding.integerEncoding",
        path,
    )?;
    require_supported(
        require_str(encoding, "wireEncoding", path)?,
        "base64url-no-padding",
        "encoding.wireEncoding",
        path,
    )?;
    require_supported(
        require_str(encoding, "plaintextEncoding", path)?,
        "utf-8",
        "encoding.plaintextEncoding",
        path,
    )?;
    require_supported(
        require_str(encoding, "padding", path)?,
        "none",
        "encoding.padding",
        path,
    )
}

/// 校验服务端存储语义并返回密文上限。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// `serverHandling` 缺失、服务端试图解析或记录、或上限非正时返回 `E_CONFIG`。
fn parse_server_handling(root: &Map<String, Value>, path: &Path) -> Result<usize, MeitoError> {
    let handling = require_object_field(root, "serverHandling", path)?;
    require_supported(
        require_str(handling, "store", path)?,
        "ciphertext-only",
        "serverHandling.store",
        path,
    )?;
    if require_bool(handling, "parse", path)? {
        return Err(config_error(path, "serverHandling.parse 必须为 false"));
    }
    if require_bool(handling, "log", path)? {
        return Err(config_error(path, "serverHandling.log 必须为 false"));
    }
    let max = require_u64(handling, "maxCiphertextBytes", path)?;
    if max == 0 {
        return Err(config_error(
            path,
            "serverHandling.maxCiphertextBytes 必须大于 0",
        ));
    }
    usize::try_from(max).map_err(|error| {
        config_error(
            path,
            format!("serverHandling.maxCiphertextBytes 超出 usize 范围：{error}"),
        )
    })
}

/// 读取明文顶层字段名；只校验非空，不解释其业务结构。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 字段表缺失、为空或含非字符串/空串时返回 `E_CONFIG`。
fn parse_plaintext_fields(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<Vec<String>, MeitoError> {
    let table = root
        .get("plaintextFields")
        .and_then(Value::as_array)
        .ok_or_else(|| config_error(path, "缺少 plaintextFields 数组"))?;
    if table.is_empty() {
        return Err(config_error(path, "plaintextFields 不得为空"));
    }
    let mut fields = Vec::with_capacity(table.len());
    for entry in table {
        let name = entry
            .as_str()
            .ok_or_else(|| config_error(path, "plaintextFields 条目必须是字符串"))?;
        if name.trim().is_empty() {
            return Err(config_error(path, "plaintextFields 不得包含空字段名"));
        }
        fields.push(name.to_string());
    }
    Ok(fields)
}

/// 按契约 `layout` 顺序累加偏移，并校验与算法固定尺寸一致。
///
/// # 参数
/// - `root`: 契约根对象。
/// - `magic_bytes`: 契约声明的 magic 字节数。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 布局条目数量、顺序、名称、类型不符，或公钥/nonce 尺寸与算法不符时返回 `E_CONFIG`。
fn parse_layout(
    root: &Map<String, Value>,
    magic_bytes: u64,
    path: &Path,
) -> Result<PackageLayout, MeitoError> {
    let entries = root
        .get("layout")
        .and_then(Value::as_array)
        .ok_or_else(|| config_error(path, "缺少 layout 数组"))?;
    if entries.len() != LAYOUT_NAMES.len() {
        return Err(config_error(
            path,
            format!(
                "layout 必须包含 {} 个条目，实际 {}",
                LAYOUT_NAMES.len(),
                entries.len()
            ),
        ));
    }

    let mut layout = PackageLayout {
        magic_len: 0,
        format_version_offset: 0,
        algorithm_id_offset: 0,
        public_key_offset: 0,
        public_key_len: 0,
        nonce_offset: 0,
        nonce_len: 0,
        ciphertext_offset: 0,
        header_len: 0,
    };
    let mut cursor = 0usize;
    for (index, entry) in entries.iter().enumerate() {
        let object = entry
            .as_object()
            .ok_or_else(|| config_error(path, "layout 条目必须是对象"))?;
        let order = require_u64(object, "order", path)?;
        if order != index as u64 + 1 {
            return Err(config_error(
                path,
                format!("layout[{index}].order 必须为 {}", index + 1),
            ));
        }
        let name = require_str(object, "name", path)?;
        if name != LAYOUT_NAMES[index] {
            return Err(config_error(
                path,
                format!("layout[{index}].name 必须为 {}", LAYOUT_NAMES[index]),
            ));
        }
        match index {
            0 => {
                require_supported(
                    require_str(object, "type", path)?,
                    "bytes",
                    "layout.magic.type",
                    path,
                )?;
                let bytes = require_u64(object, "bytes", path)?;
                if bytes != magic_bytes {
                    return Err(config_error(
                        path,
                        format!("layout.magic.bytes={bytes} 与 magicBytes={magic_bytes} 不一致"),
                    ));
                }
                let len = length_to_usize(bytes, "layout.magic.bytes", path)?;
                layout.magic_len = len;
                cursor = advance(cursor, len, path)?;
            }
            1 | 2 => {
                require_supported(
                    require_str(object, "type", path)?,
                    "u8",
                    "layout 整数类型",
                    path,
                )?;
                let bytes = require_u64(object, "bytes", path)?;
                if bytes != 1 {
                    return Err(config_error(
                        path,
                        format!("layout 的 u8 字段 bytes 必须为 1，实际 {bytes}"),
                    ));
                }
                if index == 1 {
                    layout.format_version_offset = cursor;
                } else {
                    layout.algorithm_id_offset = cursor;
                }
                cursor = advance(cursor, 1, path)?;
            }
            3 => {
                require_supported(
                    require_str(object, "type", path)?,
                    "bytes",
                    "layout.ephemeral_public_key.type",
                    path,
                )?;
                let len = length_to_usize(
                    require_u64(object, "bytes", path)?,
                    "layout.ephemeral_public_key.bytes",
                    path,
                )?;
                layout.public_key_offset = cursor;
                layout.public_key_len = len;
                cursor = advance(cursor, len, path)?;
            }
            4 => {
                require_supported(
                    require_str(object, "type", path)?,
                    "bytes",
                    "layout.nonce.type",
                    path,
                )?;
                let len = length_to_usize(
                    require_u64(object, "bytes", path)?,
                    "layout.nonce.bytes",
                    path,
                )?;
                layout.nonce_offset = cursor;
                layout.nonce_len = len;
                cursor = advance(cursor, len, path)?;
            }
            _ => {
                require_supported(
                    require_str(object, "type", path)?,
                    "bytes",
                    "layout.ciphertext.type",
                    path,
                )?;
                let size = object
                    .get("bytes")
                    .and_then(Value::as_str)
                    .ok_or_else(|| config_error(path, "layout.ciphertext.bytes 必须是字符串"))?;
                require_supported(size, "variable", "layout.ciphertext.bytes", path)?;
                if !require_bool(object, "includesTag", path)? {
                    return Err(config_error(
                        path,
                        "layout.ciphertext.includesTag 必须为 true",
                    ));
                }
                layout.ciphertext_offset = cursor;
            }
        }
    }

    if layout.public_key_len != X25519_PUBLIC_KEY_BYTES {
        return Err(config_error(
            path,
            format!(
                "X25519 公钥必须为 {X25519_PUBLIC_KEY_BYTES} 字节，契约声明 {}",
                layout.public_key_len
            ),
        ));
    }
    if layout.nonce_len != XCHACHA_NONCE_BYTES {
        return Err(config_error(
            path,
            format!(
                "XChaCha20-Poly1305 nonce 必须为 {XCHACHA_NONCE_BYTES} 字节，契约声明 {}",
                layout.nonce_len
            ),
        ));
    }
    layout.header_len = cursor;
    Ok(layout)
}

// ===== 密钥与封装 =====

impl ConnectionKeyPair {
    /// 生成每作业临时密钥对；调用方不得跨作业复用。
    ///
    /// # 参数
    /// - 无。
    ///
    /// # 错误
    /// 不返回错误；随机源由操作系统提供。
    pub fn generate() -> Self {
        Self::from_static_secret(StaticSecret::random_from_rng(OsRng))
    }

    /// 从固定私钥构造密钥对，仅供固定向量与回归测试。
    ///
    /// # 参数
    /// - `secret`: 32 字节 X25519 私钥。
    ///
    /// # 错误
    /// 不返回错误；生产代码必须使用 [`ConnectionKeyPair::generate`]，私钥不导出。
    pub fn from_secret_bytes(secret: [u8; X25519_PUBLIC_KEY_BYTES]) -> Self {
        Self::from_static_secret(StaticSecret::from(secret))
    }

    /// 返回公钥字节，供工作流输入或固定向量比对。
    ///
    /// # 参数
    /// - 无（仅 `&self`）。
    ///
    /// # 错误
    /// 不返回错误。
    pub fn public_key_bytes(&self) -> [u8; X25519_PUBLIC_KEY_BYTES] {
        self.public.to_bytes()
    }

    /// 用已构造的 `StaticSecret` 初始化密钥对。
    ///
    /// # 参数
    /// - `secret`: 私钥容器，离开作用域时清零。
    ///
    /// # 错误
    /// 不返回错误。
    fn from_static_secret(secret: StaticSecret) -> Self {
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }
}

/// 封装 UTF-8 明文，返回契约定义的二进制容器。
///
/// # 参数
/// - `contract`: 已加载的连接包契约。
/// - `recipient_public`: 接收方 X25519 公钥。
/// - `plaintext`: 必须包含契约 `plaintextFields` 的 UTF-8 文本；本层不解释字段结构。
///
/// # 错误
/// 加密失败、密文超过 `maxCiphertextBytes` 时返回 `E_CRYPTO`（退出码 6）。
pub fn seal_connection_package(
    contract: &ConnectionPackageContract,
    recipient_public: &[u8; X25519_PUBLIC_KEY_BYTES],
    plaintext: &str,
) -> Result<Vec<u8>, MeitoError> {
    let ephemeral = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral);
    let shared = ephemeral.diffie_hellman(&PublicKey::from(*recipient_public));
    let cipher = XChaCha20Poly1305::new_from_slice(&shared.to_bytes())
        .map_err(|error| crypto_error(format!("初始化 XChaCha20-Poly1305 失败：{error}")))?;
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|error| crypto_error(format!("加密连接包失败：{error}")))?;
    if ciphertext.len() > contract.max_ciphertext_bytes {
        return Err(crypto_error(format!(
            "密文 {} 字节超过契约上限 {}",
            ciphertext.len(),
            contract.max_ciphertext_bytes
        )));
    }

    let mut container = Vec::new();
    container.extend_from_slice(contract.magic.as_bytes());
    container.push(contract.format_version);
    container.push(contract.algorithm_id_byte);
    container.extend_from_slice(ephemeral_public.as_bytes());
    container.extend_from_slice(nonce.as_slice());
    container.extend_from_slice(&ciphertext);
    if container.len() != contract.layout.header_len + ciphertext.len() {
        return Err(crypto_error("封装结果与契约头部布局不一致"));
    }
    Ok(container)
}

/// 解封二进制容器并返回 UTF-8 明文。
///
/// # 参数
/// - `contract`: 已加载的连接包契约。
/// - `keypair`: 接收方密钥对，每作业临时生成。
/// - `container`: 契约线格式的二进制容器。
///
/// # 错误
/// 头部长度不足、magic/版本/算法 id 不符、密文超限、AEAD 校验失败或明文不是
/// UTF-8 时返回 `E_CRYPTO`（退出码 6）。
pub fn open_connection_package(
    contract: &ConnectionPackageContract,
    keypair: &ConnectionKeyPair,
    container: &[u8],
) -> Result<String, MeitoError> {
    let layout = &contract.layout;
    let (prefix, ciphertext) = container
        .split_at_checked(layout.header_len)
        .ok_or_else(|| {
            crypto_error(format!(
                "容器 {} 字节不足固定头部 {} 字节",
                container.len(),
                layout.header_len
            ))
        })?;
    let magic = prefix
        .get(..layout.magic_len)
        .ok_or_else(|| crypto_error("容器缺少 magic"))?;
    if magic != contract.magic.as_bytes() {
        return Err(crypto_error("magic 与契约不符"));
    }
    let version = prefix
        .get(layout.format_version_offset)
        .copied()
        .ok_or_else(|| crypto_error("容器缺少格式版本"))?;
    if version != contract.format_version {
        return Err(crypto_error(format!(
            "格式版本 {version} 与契约 {} 不符",
            contract.format_version
        )));
    }
    let algorithm = prefix
        .get(layout.algorithm_id_offset)
        .copied()
        .ok_or_else(|| crypto_error("容器缺少算法 id"))?;
    if algorithm != contract.algorithm_id_byte {
        return Err(crypto_error(format!(
            "算法 id {algorithm} 与契约 {} 不符",
            contract.algorithm_id_byte
        )));
    }
    if ciphertext.len() > contract.max_ciphertext_bytes {
        return Err(crypto_error(format!(
            "密文 {} 字节超过契约上限 {}",
            ciphertext.len(),
            contract.max_ciphertext_bytes
        )));
    }

    let public_key = fixed_array_from_slice::<X25519_PUBLIC_KEY_BYTES>(
        prefix,
        layout.public_key_offset,
        layout.public_key_len,
    )?;
    let nonce = fixed_array_from_slice::<XCHACHA_NONCE_BYTES>(
        prefix,
        layout.nonce_offset,
        layout.nonce_len,
    )?;
    let shared = keypair.secret.diffie_hellman(&PublicKey::from(public_key));
    let cipher = XChaCha20Poly1305::new_from_slice(&shared.to_bytes())
        .map_err(|error| crypto_error(format!("初始化 XChaCha20-Poly1305 失败：{error}")))?;
    let plaintext = cipher
        .decrypt(XNonce::from_slice(&nonce), ciphertext)
        .map_err(|error| crypto_error(format!("AEAD 校验或解密失败：{error}")))?;
    String::from_utf8(plaintext).map_err(|error| crypto_error(format!("明文不是 UTF-8：{error}")))
}

// ===== 线格式 =====

/// 把二进制容器编码为契约规定的 base64url（无填充）。
///
/// # 参数
/// - `container`: 二进制容器。
///
/// # 错误
/// 不返回错误。
pub fn encode_package_wire(container: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(container)
}

/// 把 base64url（无填充）文本解码为二进制容器。
///
/// # 参数
/// - `wire`: 契约线格式文本。
///
/// # 错误
/// 不是合法 base64url 时返回 `E_CRYPTO`（退出码 6）。
pub fn decode_package_wire(wire: &str) -> Result<Vec<u8>, MeitoError> {
    URL_SAFE_NO_PAD
        .decode(wire)
        .map_err(|error| crypto_error(format!("线格式不是合法 base64url：{error}")))
}

// ===== 辅助函数 =====

/// 把 `&Value` 转为对象。
///
/// # 参数
/// - `value`: 待转换 JSON。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 不是 JSON 对象时返回 `E_CONFIG`。
fn require_object<'a>(value: &'a Value, path: &Path) -> Result<&'a Map<String, Value>, MeitoError> {
    value
        .as_object()
        .ok_or_else(|| config_error(path, "连接包契约根必须是对象"))
}

/// 读取必需对象字段。
///
/// # 参数
/// - `root`: 父对象。
/// - `key`: 字段名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 字段缺失或不是对象时返回 `E_CONFIG`。
fn require_object_field<'a>(
    root: &'a Map<String, Value>,
    key: &str,
    path: &Path,
) -> Result<&'a Map<String, Value>, MeitoError> {
    root.get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| config_error(path, format!("缺少 {key} 对象")))
}

/// 读取必需字符串字段。
///
/// # 参数
/// - `root`: 父对象。
/// - `key`: 字段名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 字段缺失或不是字符串时返回 `E_CONFIG`。
fn require_str<'a>(
    root: &'a Map<String, Value>,
    key: &str,
    path: &Path,
) -> Result<&'a str, MeitoError> {
    root.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| config_error(path, format!("{key} 必须是字符串")))
}

/// 读取必需布尔字段。
///
/// # 参数
/// - `root`: 父对象。
/// - `key`: 字段名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 字段缺失或不是布尔值时返回 `E_CONFIG`。
fn require_bool(root: &Map<String, Value>, key: &str, path: &Path) -> Result<bool, MeitoError> {
    root.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| config_error(path, format!("{key} 必须是布尔值")))
}

/// 读取必需非负整数字段。
///
/// # 参数
/// - `root`: 父对象。
/// - `key`: 字段名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 字段缺失、为负或不是整数时返回 `E_CONFIG`。
fn require_u64(root: &Map<String, Value>, key: &str, path: &Path) -> Result<u64, MeitoError> {
    root.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| config_error(path, format!("{key} 必须是非负整数")))
}

/// 把契约整数收窄为 u8。
///
/// # 参数
/// - `value`: 契约整数。
/// - `field`: 字段名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 超出 u8 范围时返回 `E_CONFIG`。
fn to_u8(value: u64, field: &str, path: &Path) -> Result<u8, MeitoError> {
    u8::try_from(value)
        .map_err(|error| config_error(path, format!("{field} 超出 u8 范围：{error}")))
}

/// 把契约整数换算为字节长度。
///
/// # 参数
/// - `value`: 契约整数。
/// - `field`: 字段名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 超出 `usize` 或为零时返回 `E_CONFIG`。
fn length_to_usize(value: u64, field: &str, path: &Path) -> Result<usize, MeitoError> {
    let length = usize::try_from(value)
        .map_err(|error| config_error(path, format!("{field} 超出 usize 范围：{error}")))?;
    if length == 0 {
        return Err(config_error(path, format!("{field} 必须大于 0")));
    }
    Ok(length)
}

/// 累加布局偏移并防止溢出。
///
/// # 参数
/// - `cursor`: 当前偏移。
/// - `len`: 本次字段长度。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 偏移溢出时返回 `E_CONFIG`。
fn advance(cursor: usize, len: usize, path: &Path) -> Result<usize, MeitoError> {
    cursor
        .checked_add(len)
        .ok_or_else(|| config_error(path, "layout 偏移溢出"))
}

/// 校验字段取值是否为实现支持的唯一值。
///
/// # 参数
/// - `actual`: 契约值。
/// - `expected`: 实现支持的值。
/// - `field`: 字段名。
/// - `path`: 仅用于错误定位。
///
/// # 错误
/// 取值不受支持时返回 `E_CONFIG`。
fn require_supported(
    actual: &str,
    expected: &str,
    field: &str,
    path: &Path,
) -> Result<(), MeitoError> {
    if actual != expected {
        return Err(config_error(
            path,
            format!("{field} 只接受 {expected}，收到 {actual}"),
        ));
    }
    Ok(())
}

/// 从头部按偏移截取固定长度数组。
///
/// # 参数
/// - `prefix`: 固定头部。
/// - `offset`: 字段起始偏移。
/// - `len`: 字段字节数，必须等于 `N`。
///
/// # 错误
/// 偏移越界或长度不等于 `N` 时返回 `E_CRYPTO`。
fn fixed_array_from_slice<const N: usize>(
    prefix: &[u8],
    offset: usize,
    len: usize,
) -> Result<[u8; N], MeitoError> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| crypto_error("头部偏移溢出"))?;
    let slice = prefix
        .get(offset..end)
        .ok_or_else(|| crypto_error("头部字段越界"))?;
    slice
        .try_into()
        .map_err(|error| crypto_error(format!("头部字段长度不符：{error}")))
}

/// 构造连接包加密错误。
///
/// # 参数
/// - `message`: 具体原因，不得包含明文、密文或密钥。
///
/// # 错误
/// 不返回错误；返回已构造的 `E_CRYPTO`。
fn crypto_error(message: impl std::fmt::Display) -> MeitoError {
    MeitoError::new(ErrorCode::Crypto, format!("连接包 {message}"))
}

/// 构造带文件定位的配置错误。
///
/// # 参数
/// - `path`: 契约路径。
/// - `message`: 具体原因。
///
/// # 错误
/// 不返回错误；返回已构造的 `E_CONFIG`。
fn config_error(path: &Path, message: impl std::fmt::Display) -> MeitoError {
    MeitoError::new(
        ErrorCode::Config,
        format!("连接包契约 {} {message}", path.display()),
    )
}
