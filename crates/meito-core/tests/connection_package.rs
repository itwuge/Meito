//! File: 连接包契约、封装/解封、线格式与跨语言固定向量测试。
//!
//! 期望值一律从 `contracts/connection-package.json` 读取；固定向量由入库脚本
//! `scripts/gen-connection-package-vector.py` 生成（PyNaCl 1.6.2：X25519 → HKDF-SHA256 →
//! XChaCha20-Poly1305 IETF，空 AAD），用于验证 RustCrypto 实现的互操作性。向量可复算：
//!
//! ```text
//! pip install pynacl==1.6.2
//! python scripts/gen-connection-package-vector.py
//! ```
//!
//! 脚本不含系统随机源，连续两次输出必须逐字节一致。X25519 公钥与共享密钥另用
//! RFC 7748 §6.1 公开向量核对。

use std::path::PathBuf;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use meito_core::{
    decode_package_wire, encode_package_wire, load_connection_package_contract,
    open_connection_package, seal_connection_package, ConnectionKeyPair, ConnectionPackageContract,
    ErrorCode, CONNECTION_PACKAGE_CONTRACT_PATH,
};
use serde_json::{json, Value};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

// ===== 测试数据 =====

/// 合法明文示例；字段名与契约 `plaintextFields` 一致。
const PAYLOAD: &str = r#"{"version":1,"session_id":"11111111-1111-4111-8111-111111111111","host_key_pin":"SHA256:test-pin","p2p":{"host":"127.0.0.1","mode":"frp-xtcp","port":17000},"entry":{"terminal":17001,"graphical":16080,"vnc":15900},"read_token":"read-token-test","write_token":"write-token-test","expires_at":"2026-11-08T12:00:00Z"}"#;

/// RFC 7748 §6.1 的 Alice 私钥。
const RFC7748_ALICE_SECRET: &str =
    "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";

/// RFC 7748 §6.1 的 Alice 公钥。
const RFC7748_ALICE_PUBLIC: &str =
    "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a";

/// 入库脚本生成的固定容器（HKDF-SHA256 派生后 XChaCha20-Poly1305，空 AAD）；
/// 接收方私钥为 RFC7748_ALICE_SECRET，临时私钥为 RFC 7748 §6.1 Bob 私钥，nonce 为 00..17。
/// 重新生成：`python scripts/gen-connection-package-vector.py`。
const LIBSODIUM_VECTOR_WIRE: &str = "TUVJVE8xAQDentt9e33BtNNbYcLs5DU3P4NDyFt4Z02t_H4Ub4grTwABAgMEBQYHCAkKCwwNDg8QERITFBUWF9mWNwdQIlvEW4V-cIAuM_nYfWlqBmrfArkWouVKvHIMGEw1bR96pQAYvwfq13u7B66JH85F8ZVgeEk2EnsTtu6QORhuWEkpaY4VU5LnYKtwG_Nf_BCZ1jxt0yWZyOexCIoj9B4B0A64shXehXPW7rZ155STzsq5cJkt8FOzM1U7HX1B-fOkD_xkfVR8d300DPDYvtcsLVzyJlWC4jceFzy8_Ow2lm9HD9ICi5gYT8fyZObildsjKU9LImSTbd8kPKzjDWFbBhlYJXmenzVZmH3bbCchxB6sJ7hiHaQxqzBrDfloGyYUPmhCP6V2U4aJ0Yo7f8ZAZbGWHI4KakzcT1-gqXg3v_sCFUjI8f-5AoXyJpbuEm2wVrYKKIln5qJiAnPSL0IzBLWgsADbX_HRFXiSZdG_RbhPqbJNu7CaQySGH40Y-ZEGfPUq";

/// 固定向量解密后的明文；键顺序为序列化时的字典序。
const LIBSODIUM_VECTOR_PLAINTEXT: &str = r#"{"entry":{"graphical":16080,"terminal":17001,"vnc":15900},"expires_at":"2026-11-08T12:00:00Z","host_key_pin":"SHA256:test-pin","p2p":{"host":"127.0.0.1","mode":"frp-xtcp","port":17000},"read_token":"read-token-test","session_id":"11111111-1111-4111-8111-111111111111","version":1,"write_token":"write-token-test"}"#;

// ===== 测试辅助 =====

/// 返回仓库根目录。
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("解析仓库根目录")
}

/// 读取仓库连接包契约原文。
fn contract_text() -> String {
    std::fs::read_to_string(repository_root().join(CONNECTION_PACKAGE_CONTRACT_PATH))
        .expect("读取仓库连接包契约")
}

/// 从仓库契约解析出的 JSON；用于独立取得期望值。
fn contract_value() -> Value {
    serde_json::from_str(&contract_text()).expect("解析仓库连接包契约")
}

/// 生成独立临时目录并写好 `contracts/` 子目录。
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("meito-package-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("contracts")).expect("创建临时契约目录");
    root
}

/// 复制仓库契约到临时目录并执行一次改写。
fn rewritten_contract(name: &str, mutate: impl FnOnce(&mut Value)) -> PathBuf {
    let root = temp_root(name);
    let mut value = contract_value();
    mutate(&mut value);
    std::fs::write(
        root.join(CONNECTION_PACKAGE_CONTRACT_PATH),
        serde_json::to_string(&value).expect("序列化临时契约"),
    )
    .expect("写入临时契约");
    root
}

/// 把十六进制文本解码为字节。
fn hex_bytes(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0, "十六进制长度必须为偶数");
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("十六进制字符"), 16)
                .expect("合法十六进制字节")
        })
        .collect()
}

/// 把 64 位十六进制文本解码为 32 字节数组。
fn hex32(text: &str) -> [u8; 32] {
    hex_bytes(text).try_into().expect("必须为 32 字节")
}

/// 加载仓库契约并生成一次性接收方密钥与封装结果。
fn sealed(contract: &ConnectionPackageContract) -> (ConnectionKeyPair, Vec<u8>) {
    let keypair = ConnectionKeyPair::generate();
    let container = seal_connection_package(contract, &keypair.public_key_bytes(), PAYLOAD)
        .expect("封装示例明文");
    (keypair, container)
}

/// 用 RFC 7748 Alice 私钥解封仓库固定向量；用于验证派生参数不一致时必须失败。
fn open_fixed_vector(
    contract: &ConnectionPackageContract,
) -> Result<String, meito_core::MeitoError> {
    let keypair = ConnectionKeyPair::from_secret_bytes(hex32(RFC7748_ALICE_SECRET));
    let container = decode_package_wire(LIBSODIUM_VECTOR_WIRE).expect("解码固定向量");
    open_connection_package(contract, &keypair, &container)
}

/// 断言错误码为 `E_CRYPTO` 且退出码为 6。
fn assert_crypto(error: meito_core::MeitoError) {
    assert_eq!(error.code(), ErrorCode::Crypto, "错误码");
    assert_eq!(error.exit_code(), 6, "退出码");
}

/// 断言错误码为 `E_CONFIG` 且退出码为 2。
fn assert_config(error: meito_core::MeitoError) {
    assert_eq!(error.code(), ErrorCode::Config, "错误码");
    assert_eq!(error.exit_code(), 2, "退出码");
}

// ===== 契约读取 =====

#[test]
fn loader_reads_every_value_from_contract() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let value = contract_value();

    assert_eq!(contract.magic, value["magic"].as_str().expect("magic"));
    assert_eq!(
        contract.magic_bytes,
        value["magicBytes"].as_u64().expect("magicBytes") as usize
    );
    assert_eq!(
        contract.format_version,
        value["formatVersion"].as_u64().expect("formatVersion") as u8
    );
    assert_eq!(
        contract.algorithm_id,
        value["algorithm"]["id"].as_str().expect("algorithm.id")
    );
    assert_eq!(
        contract.algorithm_id_byte,
        value["algorithm"]["idByte"].as_u64().expect("idByte") as u8
    );
    assert_eq!(
        contract.max_ciphertext_bytes,
        value["serverHandling"]["maxCiphertextBytes"]
            .as_u64()
            .expect("maxCiphertextBytes") as usize
    );
    let expected_fields: Vec<String> = value["plaintextFields"]
        .as_array()
        .expect("plaintextFields")
        .iter()
        .map(|item| item.as_str().expect("字段名").to_string())
        .collect();
    assert_eq!(contract.plaintext_fields, expected_fields);

    assert_eq!(
        contract.kdf.algorithm,
        value["kdf"]["algorithm"].as_str().expect("kdf.algorithm")
    );
    assert_eq!(
        contract.kdf.hash,
        value["kdf"]["hash"].as_str().expect("kdf.hash")
    );
    assert_eq!(
        contract.kdf.ikm,
        value["kdf"]["ikm"].as_str().expect("kdf.ikm")
    );
    assert_eq!(
        contract.kdf.salt,
        value["kdf"]["salt"].as_str().expect("kdf.salt")
    );
    assert_eq!(
        contract.kdf.salt_policy,
        value["kdf"]["saltPolicy"].as_str().expect("kdf.saltPolicy")
    );
    assert_eq!(
        contract.kdf.info,
        value["kdf"]["info"].as_str().expect("kdf.info")
    );
    assert_eq!(
        contract.kdf.output_bytes,
        value["kdf"]["outputBytes"]
            .as_u64()
            .expect("kdf.outputBytes") as usize
    );
    assert_eq!(
        contract.kdf.used_as,
        value["kdf"]["usedAs"].as_str().expect("kdf.usedAs")
    );
    assert!(
        !contract.kdf.raw_shared_secret_as_key,
        "共享密钥不得直接当 AEAD 密钥"
    );
    assert_eq!(
        contract.aad.policy,
        value["aad"]["policy"].as_str().expect("aad.policy"),
        "AAD 策略必须来自契约"
    );
    assert_eq!(
        contract.aad.aad_bytes,
        value["aad"]["aadBytes"].as_u64().expect("aad.aadBytes") as usize,
        "AAD 字节数必须来自契约"
    );

    let expected_header: usize = value["layout"]
        .as_array()
        .expect("layout")
        .iter()
        .take(5)
        .map(|entry| entry["bytes"].as_u64().expect("固定字段字节数") as usize)
        .sum();
    assert_eq!(
        contract.layout.header_len, expected_header,
        "头部长度必须由契约 layout 求和得到"
    );
    assert_eq!(
        contract.layout.ciphertext_offset, expected_header,
        "密文偏移必须等于头部长度"
    );
}

#[test]
fn loader_follows_rewritten_magic() {
    let root = rewritten_contract("magic", |value| {
        let magic = value["magic"].as_str().expect("magic").to_string();
        let replacement = format!("X{}", &magic[1..]);
        value["magic"] = json!(replacement);
    });
    let contract = load_connection_package_contract(&root).expect("加载改写后的契约");
    let original = load_connection_package_contract(&repository_root()).expect("加载仓库契约");
    assert_ne!(contract.magic, original.magic, "magic 必须随契约变化");
    assert_eq!(contract.magic.len(), contract.magic_bytes);

    let (keypair, container) = sealed(&contract);
    assert_eq!(
        &container[..contract.layout.magic_len],
        contract.magic.as_bytes(),
        "封装必须写入契约 magic"
    );
    let plaintext =
        open_connection_package(&contract, &keypair, &container).expect("用同一改写契约解封");
    assert_eq!(plaintext, PAYLOAD);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn loader_follows_rewritten_max_limit() {
    let root = rewritten_contract("limit", |value| {
        value["serverHandling"]["maxCiphertextBytes"] = json!(64);
    });
    let contract = load_connection_package_contract(&root).expect("加载改写后的契约");
    assert_eq!(contract.max_ciphertext_bytes, 64);
    let keypair = ConnectionKeyPair::generate();
    let error = seal_connection_package(&contract, &keypair.public_key_bytes(), PAYLOAD)
        .expect_err("超过改写后的上限必须拒绝");
    assert_crypto(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn loader_rejects_unsupported_algorithm() {
    let root = rewritten_contract("algorithm", |value| {
        value["algorithm"]["id"] = json!("rsa2048");
    });
    let error = load_connection_package_contract(&root).expect_err("不支持的算法必须拒绝");
    assert_config(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn loader_rejects_layout_size_mismatch() {
    let root = rewritten_contract("layout", |value| {
        value["layout"][3]["bytes"] = json!(16);
    });
    let error = load_connection_package_contract(&root).expect_err("公钥尺寸不符必须拒绝");
    assert_config(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn loader_rejects_missing_limit() {
    let root = rewritten_contract("missing-limit", |value| {
        value["serverHandling"]
            .as_object_mut()
            .expect("serverHandling")
            .remove("maxCiphertextBytes");
    });
    let error = load_connection_package_contract(&root).expect_err("缺少上限必须拒绝");
    assert_config(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn missing_contract_is_rejected() {
    let root = temp_root("missing");
    let error = load_connection_package_contract(&root).expect_err("缺少契约必须拒绝");
    assert_config(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

// ===== KDF 与 AAD =====

#[test]
fn kdf_info_mismatch_fails_to_open() {
    let root = rewritten_contract("kdf-info", |value| {
        value["kdf"]["info"] = json!("meito-v1/other-purpose");
    });
    let contract = load_connection_package_contract(&root).expect("加载改写后的契约");
    let error = open_fixed_vector(&contract).expect_err("info 不一致必须解封失败");
    assert_crypto(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn kdf_salt_mismatch_fails_to_open() {
    let root = rewritten_contract("kdf-salt", |value| {
        value["kdf"]["salt"] = json!("meito-v1");
    });
    let contract = load_connection_package_contract(&root).expect("加载改写后的契约");
    let error = open_fixed_vector(&contract).expect_err("salt 不一致必须解封失败");
    assert_crypto(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn kdf_output_length_mismatch_fails_to_open() {
    let root = rewritten_contract("kdf-output", |value| {
        value["kdf"]["outputBytes"] = json!(16);
    });
    let contract = load_connection_package_contract(&root).expect("加载改写后的契约");
    assert_eq!(contract.kdf.output_bytes, 16, "输出长度必须来自契约");
    let error = open_fixed_vector(&contract).expect_err("输出长度不一致必须解封失败");
    assert_crypto(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn aad_policy_other_than_none_is_rejected() {
    let root = rewritten_contract("aad-policy", |value| {
        value["aad"]["policy"] = json!("optional");
    });
    let error = load_connection_package_contract(&root).expect_err("非 none 的 AAD 策略必须拒绝");
    assert_config(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

#[test]
fn aad_bytes_nonzero_is_rejected() {
    let root = rewritten_contract("aad-bytes", |value| {
        value["aad"]["aadBytes"] = json!(8);
    });
    let error = load_connection_package_contract(&root).expect_err("none 策略下非零 AAD 必须拒绝");
    assert_config(error);
    std::fs::remove_dir_all(&root).expect("清理临时目录");
}

/// 契约 `aad.policy=none`：固定向量只以空 AAD 通过认证，非空 AAD 必须认证失败。
#[test]
fn fixed_vector_authenticates_only_with_empty_aad() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    assert_eq!(contract.aad.policy, "none", "契约 AAD 策略");
    assert_eq!(contract.aad.aad_bytes, 0, "契约 AAD 字节数");
    let container = decode_package_wire(LIBSODIUM_VECTOR_WIRE).expect("解码固定向量");

    let public_key_start = contract.layout.public_key_offset;
    let public_key_end = public_key_start + contract.layout.public_key_len;
    let ephemeral_public: [u8; 32] = container
        .get(public_key_start..public_key_end)
        .expect("固定向量临时公钥")
        .try_into()
        .expect("临时公钥必须为 32 字节");
    let shared = StaticSecret::from(hex32(RFC7748_ALICE_SECRET))
        .diffie_hellman(&PublicKey::from(ephemeral_public));
    let hkdf = Hkdf::<Sha256>::new(Some(contract.kdf.salt.as_bytes()), shared.as_bytes());
    let mut key = vec![0u8; contract.kdf.output_bytes];
    hkdf.expand(contract.kdf.info.as_bytes(), &mut key)
        .expect("按契约派生固定向量密钥");
    let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("初始化 XChaCha20-Poly1305");

    let nonce_start = contract.layout.nonce_offset;
    let nonce_end = nonce_start + contract.layout.nonce_len;
    let nonce = XNonce::from_slice(
        container
            .get(nonce_start..nonce_end)
            .expect("固定向量 nonce"),
    );
    let ciphertext = container
        .get(contract.layout.ciphertext_offset..)
        .expect("固定向量密文");

    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad: &[],
            },
        )
        .expect("空 AAD 必须能解出固定向量");
    assert_eq!(
        String::from_utf8(plaintext).expect("明文 UTF-8"),
        LIBSODIUM_VECTOR_PLAINTEXT
    );
    let rejected = cipher.decrypt(
        nonce,
        Payload {
            msg: ciphertext,
            aad: b"not-none",
        },
    );
    assert!(rejected.is_err(), "非空 AAD 必须认证失败");
}

// ===== 封装与解封 =====

#[test]
fn round_trip_seals_and_opens() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (keypair, container) = sealed(&contract);
    assert!(
        container.len() > contract.layout.header_len,
        "容器必须包含密文"
    );
    assert_eq!(
        container.get(..contract.layout.magic_len),
        Some(contract.magic.as_bytes())
    );
    assert_eq!(
        container.get(contract.layout.format_version_offset),
        Some(&contract.format_version)
    );
    assert_eq!(
        container.get(contract.layout.algorithm_id_offset),
        Some(&contract.algorithm_id_byte)
    );

    let plaintext = open_connection_package(&contract, &keypair, &container).expect("解封");
    assert_eq!(plaintext, PAYLOAD);

    let wire = encode_package_wire(&container);
    let decoded = decode_package_wire(&wire).expect("线格式解码");
    assert_eq!(decoded, container, "base64url 往返必须无损");
}

#[test]
fn cross_language_libsodium_vector_opens() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let keypair = ConnectionKeyPair::from_secret_bytes(hex32(RFC7748_ALICE_SECRET));
    let container = decode_package_wire(LIBSODIUM_VECTOR_WIRE).expect("解码固定向量");
    let plaintext = open_connection_package(&contract, &keypair, &container)
        .expect("libsodium 向量必须能被本实现解封");
    assert_eq!(plaintext, LIBSODIUM_VECTOR_PLAINTEXT);
}

#[test]
fn rfc7748_public_key_vector_matches() {
    let keypair = ConnectionKeyPair::from_secret_bytes(hex32(RFC7748_ALICE_SECRET));
    assert_eq!(
        keypair.public_key_bytes(),
        hex32(RFC7748_ALICE_PUBLIC),
        "X25519 公钥必须匹配 RFC 7748 §6.1"
    );
}

// ===== 拒绝路径 =====

#[test]
fn tampered_ciphertext_is_rejected() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (keypair, mut container) = sealed(&contract);
    let offset = contract.layout.ciphertext_offset;
    let target = container.get_mut(offset).expect("密文首字节");
    *target ^= 0x01;
    let error = open_connection_package(&contract, &keypair, &container).expect_err("篡改必须拒绝");
    assert_crypto(error);
}

#[test]
fn wrong_magic_is_rejected() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (keypair, mut container) = sealed(&contract);
    let target = container.get_mut(0).expect("magic 首字节");
    *target ^= 0xFF;
    let error = open_connection_package(&contract, &keypair, &container).expect_err("错误 magic");
    assert_crypto(error);
}

#[test]
fn wrong_format_version_is_rejected() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (keypair, mut container) = sealed(&contract);
    let offset = contract.layout.format_version_offset;
    let target = container.get_mut(offset).expect("版本字段");
    *target = contract.format_version.wrapping_add(1);
    let error = open_connection_package(&contract, &keypair, &container).expect_err("错误版本");
    assert_crypto(error);
}

#[test]
fn unknown_algorithm_id_is_rejected() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (keypair, mut container) = sealed(&contract);
    let offset = contract.layout.algorithm_id_offset;
    let target = container.get_mut(offset).expect("算法 id 字段");
    *target = contract.algorithm_id_byte.wrapping_add(1);
    let error = open_connection_package(&contract, &keypair, &container).expect_err("未知算法 id");
    assert_crypto(error);
}

#[test]
fn truncated_container_is_rejected() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (keypair, container) = sealed(&contract);
    let truncated = container
        .get(..contract.layout.header_len - 1)
        .expect("截断头部");
    let error = open_connection_package(&contract, &keypair, truncated).expect_err("截断必须拒绝");
    assert_crypto(error);
}

#[test]
fn wrong_recipient_key_is_rejected() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (_, container) = sealed(&contract);
    let other = ConnectionKeyPair::generate();
    let error = open_connection_package(&contract, &other, &container).expect_err("错误私钥");
    assert_crypto(error);
}

#[test]
fn ciphertext_over_limit_is_rejected_on_seal() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let keypair = ConnectionKeyPair::generate();
    let oversized = "a".repeat(contract.max_ciphertext_bytes + 1);
    let error = seal_connection_package(&contract, &keypair.public_key_bytes(), &oversized)
        .expect_err("超限明文必须拒绝");
    assert_crypto(error);
}

#[test]
fn ciphertext_over_limit_is_rejected_on_open() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let (keypair, container) = sealed(&contract);
    let header = container
        .get(..contract.layout.header_len)
        .expect("固定头部");
    let mut oversized = header.to_vec();
    let zeros = vec![0u8; contract.max_ciphertext_bytes + 1];
    oversized.extend_from_slice(&zeros);
    let error =
        open_connection_package(&contract, &keypair, &oversized).expect_err("超限密文必须拒绝");
    assert_crypto(error);
}

#[test]
fn invalid_wire_encoding_is_rejected() {
    let error = decode_package_wire("!!!!not-base64!!!!").expect_err("非法 base64url");
    assert_crypto(error);
}

/// 确认测试辅助能正确解码固定向量头部，避免向量文本损坏时误判为解密失败。
#[test]
fn fixed_vector_header_matches_contract() {
    let contract = load_connection_package_contract(&repository_root()).expect("加载连接包契约");
    let container = decode_package_wire(LIBSODIUM_VECTOR_WIRE).expect("解码固定向量");
    assert_eq!(
        container.get(..contract.layout.magic_len),
        Some(contract.magic.as_bytes())
    );
    assert_eq!(
        container.get(contract.layout.format_version_offset),
        Some(&contract.format_version)
    );
    assert_eq!(
        container.get(contract.layout.algorithm_id_offset),
        Some(&contract.algorithm_id_byte)
    );
    let public_key_start = contract.layout.public_key_offset;
    let public_key_end = public_key_start + contract.layout.public_key_len;
    let public_key = container
        .get(public_key_start..public_key_end)
        .expect("固定向量公钥");
    assert_eq!(
        public_key,
        hex_bytes("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
    );
    let nonce_start = contract.layout.nonce_offset;
    let nonce_end = nonce_start + contract.layout.nonce_len;
    let nonce = container
        .get(nonce_start..nonce_end)
        .expect("固定向量 nonce");
    let expected_nonce: Vec<u8> = (0u8..24).collect();
    assert_eq!(nonce, expected_nonce);
}
