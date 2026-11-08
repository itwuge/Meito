#!/usr/bin/env python3
"""生成连接包固定测试向量（可复算，无系统随机源）。

契约来源：contracts/connection-package.json 的 kdf / aad / testVectors 段。
输入全部为固定常量：接收方私钥与临时私钥分别取 RFC 7748 §6.1 的 Alice / Bob
私钥，nonce 固定为 0x00..0x17，明文为固定 JSON（键按字典序序列化）。
密钥派生严格按契约：HKDF-SHA256，salt 为空串（RFC 5869 等价于 32 个零字节），
info 为 "meito-v1/connection-package"，输出 32 字节；AEAD 不传 AAD。

依赖：PyNaCl==1.6.2（pip install pynacl==1.6.2）。
用法：python scripts/gen-connection-package-vector.py
输出：stdout 打印一份 JSON；连续两次运行必须逐字节一致。
"""

import base64
import hashlib
import hmac
import json

from nacl.bindings import (
    crypto_aead_xchacha20poly1305_ietf_encrypt,
    crypto_scalarmult,
    crypto_scalarmult_base,
)

# ===== 契约固定常量 =====

MAGIC = b"MEITO1"
FORMAT_VERSION = 1
ALGORITHM_ID = 0
KDF_INFO = b"meito-v1/connection-package"
KDF_SALT = b""
KDF_OUTPUT_BYTES = 32

# ===== 固定输入（RFC 7748 §6.1） =====

RECIPIENT_SECRET_HEX = (
    "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"
)
EPHEMERAL_SECRET_HEX = (
    "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb"
)
NONCE_HEX = "000102030405060708090a0b0c0d0e0f1011121314151617"

PLAINTEXT_OBJECT = {
    "version": 1,
    "session_id": "11111111-1111-4111-8111-111111111111",
    "host_key_pin": "SHA256:test-pin",
    "p2p": {"host": "127.0.0.1", "mode": "frp-xtcp", "port": 17000},
    "entry": {"terminal": 17001, "graphical": 16080, "vnc": 15900},
    "read_token": "read-token-test",
    "write_token": "write-token-test",
    "expires_at": "2026-11-08T12:00:00Z",
}


def hkdf_sha256(ikm: bytes, salt: bytes, info: bytes, length: int) -> bytes:
    """RFC 5869 HKDF-SHA256；空 salt 按规范等价于 HashLen 个零字节。"""
    if not salt:
        salt = b"\x00" * hashlib.sha256().digest_size
    prk = hmac.new(salt, ikm, hashlib.sha256).digest()
    okm = b""
    block = b""
    counter = 1
    while len(okm) < length:
        block = hmac.new(
            prk, block + info + bytes([counter]), hashlib.sha256
        ).digest()
        okm += block
        counter += 1
    return okm[:length]


def hex_bytes(text: str) -> bytes:
    """把十六进制文本解码为字节。"""
    return bytes.fromhex(text)


def wire_encode(container: bytes) -> str:
    """按契约 encoding.wireEncoding=base64url-no-padding 编码。"""
    return base64.urlsafe_b64encode(container).decode("ascii").rstrip("=")


def main() -> None:
    recipient_secret = hex_bytes(RECIPIENT_SECRET_HEX)
    recipient_public = crypto_scalarmult_base(recipient_secret)
    ephemeral_secret = hex_bytes(EPHEMERAL_SECRET_HEX)
    ephemeral_public = crypto_scalarmult_base(ephemeral_secret)
    nonce = hex_bytes(NONCE_HEX)

    shared_secret = crypto_scalarmult(ephemeral_secret, recipient_public)
    derived_key = hkdf_sha256(
        shared_secret, KDF_SALT, KDF_INFO, KDF_OUTPUT_BYTES
    )

    plaintext = json.dumps(
        PLAINTEXT_OBJECT, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    ciphertext = crypto_aead_xchacha20poly1305_ietf_encrypt(
        plaintext, b"", nonce, derived_key
    )

    container = (
        MAGIC
        + bytes([FORMAT_VERSION, ALGORITHM_ID])
        + ephemeral_public
        + nonce
        + ciphertext
    )

    vector = {
        "contractVersion": "0.3.5",
        "recipientSecretHex": RECIPIENT_SECRET_HEX,
        "recipientPublicHex": recipient_public.hex(),
        "ephemeralSecretHex": EPHEMERAL_SECRET_HEX,
        "ephemeralPublicHex": ephemeral_public.hex(),
        "nonceHex": NONCE_HEX,
        "kdf": {
            "algorithm": "HKDF-SHA256",
            "saltHex": "",
            "info": KDF_INFO.decode("utf-8"),
            "outputBytes": KDF_OUTPUT_BYTES,
        },
        "aadHex": "",
        "sharedSecretHex": shared_secret.hex(),
        "derivedKeyHex": derived_key.hex(),
        "plaintext": plaintext.decode("utf-8"),
        "ciphertextHex": ciphertext.hex(),
        "containerHex": container.hex(),
        "wire": wire_encode(container),
    }
    print(json.dumps(vector, ensure_ascii=False, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
