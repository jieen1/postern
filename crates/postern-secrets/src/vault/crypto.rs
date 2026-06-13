//! 加密原语：XChaCha20-Poly1305 AEAD + 随机 nonce（设计承诺级桩，函数体未实现）。
//!
//! 两层加密（详细设计 5.4 / §3）：
//! 1. **包裹槽**：随机 data-key 用主密钥经 XChaCha20-Poly1305 包裹（`wrap_data_key` /
//!    `unwrap_data_key`）；rekey/rotate-kdf 只重写这一步。
//! 2. **payload**：payload 明文用 data-key 经 XChaCha20-Poly1305 加密，**明文头全段作
//!    AAD**（`encrypt_payload` / `decrypt_payload`）。头部任一字节篡改 / 版本降级使解密
//!    失败（L-2）。
//!
//! nonce 纪律（L-3，结构检查红线）：所有 24B nonce **恒从 CSPRNG 取**——`new_nonce`
//! 是唯一 nonce 取值路径，源为 `chacha20poly1305::aead::OsRng`（getrandom→操作系统
//! CSPRNG）；**源码无固定常量 nonce、无计数器递增 nonce 路径**。
//!
//! 内存纪律（B-6 / §7-1）：data-key、payload 解密产物一律 `Zeroizing` 持有，离作用域
//! 清零；AEAD/IO 失败一律 map 成 `UnlockError`（`AadMismatch`/`PayloadDecryptFailed`），
//! 绝不 unwrap / panic（fail-closed）。

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload as AeadPayload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::error::UnlockError;
use crate::vault::format::NONCE_LEN;

/// 生成一个全新的 24B nonce——**唯一**的 nonce 取值路径，源恒为 CSPRNG
/// （`chacha20poly1305::aead::OsRng`）。绝无常量 / 计数器 nonce 取值（L-3）。
pub fn new_nonce() -> [u8; NONCE_LEN] {
    // generate_nonce 内部以 OsRng（getrandom→操作系统 CSPRNG）填充 24B nonce。
    let nonce: XNonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    nonce.into()
}

/// 以 32B 密钥构造一个 XChaCha20-Poly1305 cipher。密钥本身的所有权不在此持有，
/// cipher 离作用域即释放。
fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(key.into())
}

/// 用主密钥把随机 data-key 包裹（加密）进包裹槽。返回 `(nonce_i, wrapped_data_key)`。
/// nonce 经 `new_nonce` CSPRNG 取，绝不复用（L-3）。
pub fn wrap_data_key(
    master_key: &[u8; 32],
    data_key: &Zeroizing<[u8; 32]>,
) -> Result<([u8; NONCE_LEN], Vec<u8>), UnlockError> {
    let nonce_i = new_nonce();
    let wrapped = cipher(master_key)
        .encrypt(
            XNonce::from_slice(&nonce_i),
            AeadPayload {
                msg: data_key.as_slice(),
                aad: b"",
            },
        )
        .map_err(|_| UnlockError::PayloadDecryptFailed)?;
    Ok((nonce_i, wrapped))
}

/// 用主密钥从包裹槽解出明文 data-key（`Zeroizing` 持有）。
///
/// fail-closed：主密钥错误 / 槽密文损坏使 AEAD 校验失败，map 成
/// `UnlockError::PayloadDecryptFailed`，绝不 unwrap / panic、绝不返回缺省 data-key。
pub fn unwrap_data_key(
    master_key: &[u8; 32],
    nonce_i: &[u8; NONCE_LEN],
    wrapped_data_key: &[u8],
) -> Result<Zeroizing<[u8; 32]>, UnlockError> {
    let plain = Zeroizing::new(
        cipher(master_key)
            .decrypt(
                XNonce::from_slice(nonce_i),
                AeadPayload {
                    msg: wrapped_data_key,
                    aad: b"",
                },
            )
            .map_err(|_| UnlockError::PayloadDecryptFailed)?,
    );
    // 解出的 data-key 必须恰 32 字节，否则视为损坏，fail-closed。
    let mut key: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    let arr: &[u8; 32] = plain
        .as_slice()
        .try_into()
        .map_err(|_| UnlockError::PayloadDecryptFailed)?;
    key.copy_from_slice(arr);
    Ok(key)
}

/// 用 data-key 加密 payload 明文，**`aad` 为明文头全段**。返回 `(payload_nonce, ciphertext)`。
/// nonce 经 `new_nonce` CSPRNG 取，绝不复用（L-3）。
pub fn encrypt_payload(
    data_key: &Zeroizing<[u8; 32]>,
    plaintext: &[u8],
    aad: &[u8],
) -> Result<([u8; NONCE_LEN], Vec<u8>), UnlockError> {
    let payload_nonce = new_nonce();
    let ciphertext = cipher(data_key)
        .encrypt(
            XNonce::from_slice(&payload_nonce),
            AeadPayload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| UnlockError::PayloadDecryptFailed)?;
    Ok((payload_nonce, ciphertext))
}

/// 用 data-key 解密 payload 密文，**`aad` 为明文头全段**——头部任一字节篡改 / 版本降级
/// 使 AEAD 校验失败（L-2）。解出的明文以 `Zeroizing` 持有。
///
/// fail-closed：AAD 不匹配 / data-key 错误 / 密文损坏一律使 AEAD 校验失败——本层将其
/// map 成 `UnlockError::AadMismatch`（payload 解密阶段的失败语义；data-key 错误已在
/// 上游 `unwrap_data_key` 拦为 `PayloadDecryptFailed`）。绝不 unwrap / panic、绝不返回部分明文。
pub fn decrypt_payload(
    data_key: &Zeroizing<[u8; 32]>,
    payload_nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, UnlockError> {
    let plain = cipher(data_key)
        .decrypt(
            XNonce::from_slice(payload_nonce),
            AeadPayload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| UnlockError::AadMismatch)?;
    Ok(Zeroizing::new(plain))
}
