//! vault 写入与 rekey / rotate-kdf（设计承诺级桩，函数体未实现）。
//!
//! 原子写入纪律（详细设计 5.4 / §7-6 / L-4）：写**临时文件** → `fsync` → 原子 `rename`
//! 覆盖；覆盖前把现有 vault 保留为上一代 **`.bak`**——**绝不就地覆盖原 vault**。半写
//! （临时文件写到一半 / `rename` 前）中断时，原 vault 仍可正常解锁、`.bak` 可回退（L-4）。
//!
//! 每次写入**整体重加密**：CSPRNG 重生成 payload nonce（绝不复用，L-3），用 data-key
//! 重新加密整段 payload。要表达"整体重加密写入"——本文件不写任何裸数据库写标记
//! （原子写用 `std::fs`，不碰数据库）。
//!
//! rekey / rotate-kdf（F-9）：**只重写包裹槽**（用新主密钥 / 新 KDF 参数重新包裹同一
//! data-key），**payload 密文一字不动**——同一 data-key 仍解原 payload。
//!
//! fail-closed（B-6）：任一 IO / 加密步骤失败一律返回 `VaultWriteError`，绝不 unwrap /
//! panic；写未完成则原 vault + `.bak` 完好，control 不提交相应策略变更（§6.5）。

use std::io::Write;

use thiserror::Error;
use zeroize::Zeroizing;

use crate::unlock::passphrase::{Argon2Params, Passphrase};
use crate::unlock::source::MasterKeySource;
use crate::vault::crypto;
use crate::vault::format::{VaultFile, NONCE_LEN};
use crate::vault::header::{Header, KdfParams, Slot};
use crate::vault::payload::Payload;

/// vault 写入失败面（原子写 / 重加密 / rekey 各 fail-closed 分支）。
///
/// 每变体只携常量英文文案，绝不内嵌路径 / 明文（跨 crate 边界前已脱敏，红线 7.2-1）。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VaultWriteError {
    /// 临时文件写入 / `fsync` / 原子 `rename` / `.bak` 保留中任一 IO 步骤失败。
    #[error("vault atomic write failed")]
    AtomicWriteFailed,
    /// 整体重加密（包裹 data-key / 加密 payload）失败。
    #[error("vault re-encryption failed")]
    ReencryptFailed,
}

impl Payload {
    /// 把 payload 整体重加密并**原子写入** `path`：CSPRNG 重生成 nonce → 用 data-key
    /// 加密 payload（明文头作 AAD）→ 写临时文件 → `fsync` → 原子 `rename` 覆盖；覆盖前
    /// 现有 vault 保留为 `.bak`。`master_key`/`header` 决定包裹槽（整体重加密一并重写）。
    ///
    /// fail-closed（L-4）：任一步失败返回 `Err`、原 vault + `.bak` 不被半写损坏。
    pub fn write_atomic(
        &self,
        path: &std::path::Path,
        master_key: &[u8; 32],
        data_key: &Zeroizing<[u8; 32]>,
        header: &Header,
    ) -> Result<(), VaultWriteError> {
        // 整体重加密：用 master_key 重包裹 data_key 取新包裹槽（CSPRNG 新 nonce_i），
        // 据 header 的来源 / KDF 元数据组装单槽，再用 data_key 加密整段 payload。
        let template = header
            .primary_slot()
            .ok_or(VaultWriteError::ReencryptFailed)?;
        let (slot_nonce, wrapped) = crypto::wrap_data_key(master_key, data_key)
            .map_err(|_| VaultWriteError::ReencryptFailed)?;
        let slot = Slot {
            source: template.source,
            kdf_params: template.kdf_params,
            salt: template.salt.clone(),
            nonce_i: slot_nonce,
            wrapped_data_key: wrapped,
        };
        let bytes = encrypt_vault(self, data_key, slot)?;
        atomic_write(path, &bytes)
    }
}

/// 用 data-key 整体加密 payload 并 `encode` 成整 vault 字节（明文头骨架作 AAD，
/// payload nonce 由 CSPRNG 重新生成，绝不复用，L-3）。
fn encrypt_vault(
    payload: &Payload,
    data_key: &Zeroizing<[u8; 32]>,
    slot: Slot,
) -> Result<Vec<u8>, VaultWriteError> {
    let plaintext = payload
        .to_plaintext()
        .map_err(|_| VaultWriteError::ReencryptFailed)?;
    let mut vf = VaultFile {
        header: Header {
            format_version: crate::vault::format::FORMAT_VERSION,
            slots: vec![slot],
        },
        payload_nonce: [0u8; NONCE_LEN],
        ciphertext: Vec::new(),
    };
    let aad = vf.aad_bytes();
    let (payload_nonce, ciphertext) = crypto::encrypt_payload(data_key, &plaintext, &aad)
        .map_err(|_| VaultWriteError::ReencryptFailed)?;
    vf.payload_nonce = payload_nonce;
    vf.ciphertext = ciphertext;
    Ok(vf.encode())
}

/// 原子写：覆盖前把现有 vault 复制为 `.bak`，写临时文件 → `fsync` → 原子 `rename`。
/// 任一步失败返回 `AtomicWriteFailed`，原 vault + `.bak` 不被半写损坏（L-4）。
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), VaultWriteError> {
    // 覆盖前保留上一代为 .bak（仅当目标已存在）。
    if path.exists() {
        let bak = with_suffix(path, "bak");
        std::fs::copy(path, &bak).map_err(|_| VaultWriteError::AtomicWriteFailed)?;
    }

    // 临时文件落在同目录（保证 rename 同文件系统、原子）。
    let tmp = with_suffix(path, "tmp");
    write_and_sync(&tmp, bytes)?;

    // 原子 rename 覆盖目标。
    std::fs::rename(&tmp, path).map_err(|_| VaultWriteError::AtomicWriteFailed)?;
    Ok(())
}

/// 写字节到临时文件并 `fsync`（数据落盘后才允许 rename，半写不丢）。
fn write_and_sync(tmp: &std::path::Path, bytes: &[u8]) -> Result<(), VaultWriteError> {
    let mut f = std::fs::File::create(tmp).map_err(|_| VaultWriteError::AtomicWriteFailed)?;
    f.write_all(bytes)
        .map_err(|_| VaultWriteError::AtomicWriteFailed)?;
    f.sync_all()
        .map_err(|_| VaultWriteError::AtomicWriteFailed)?;
    Ok(())
}

/// 在路径的文件名后追加 `.<suffix>`（如 `vault.postern` → `vault.postern.bak`）。
fn with_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".");
    s.push(suffix);
    std::path::PathBuf::from(s)
}

/// rekey：用**新主密钥**重新包裹同一 data-key，**只重写包裹槽**——原 vault 字节读入、
/// 解出 data-key、用新主密钥重包裹、原子写回；**payload 密文一字不动**（F-9）。
/// 返回新的 vault 文件字节（包裹槽更新、payload nonce + ciphertext 与原文逐字节相等）。
pub fn rekey(
    vault_bytes: &[u8],
    old_master_key: &[u8; 32],
    new_master_key: &[u8; 32],
) -> Result<Vec<u8>, VaultWriteError> {
    let mut vault = VaultFile::decode(vault_bytes).map_err(|_| VaultWriteError::ReencryptFailed)?;
    let slot = vault
        .header
        .primary_slot()
        .ok_or(VaultWriteError::ReencryptFailed)?;
    // 旧主密钥解出 data-key，新主密钥重包裹（payload 密文不动）。
    let data_key = crypto::unwrap_data_key(old_master_key, &slot.nonce_i, &slot.wrapped_data_key)
        .map_err(|_| VaultWriteError::ReencryptFailed)?;
    let (slot_nonce, wrapped) = crypto::wrap_data_key(new_master_key, &data_key)
        .map_err(|_| VaultWriteError::ReencryptFailed)?;
    let new_slot = Slot {
        source: slot.source,
        kdf_params: slot.kdf_params,
        salt: slot.salt.clone(),
        nonce_i: slot_nonce,
        wrapped_data_key: wrapped,
    };
    // 只重写包裹槽；payload nonce + ciphertext 逐字节保留。
    vault.header.slots = vec![new_slot];
    Ok(vault.encode())
}

/// rotate-kdf：用**新 argon2id 参数 + salt**重新派生主密钥并重包裹同一 data-key，
/// **只重写包裹槽**（仅 passphrase 来源有意义），**payload 密文一字不动**（F-9）。
/// 返回新的 vault 文件字节（包裹槽 KDF 参数 / salt 更新、payload 段与原文逐字节相等）。
pub fn rotate_kdf(
    vault_bytes: &[u8],
    passphrase: &Zeroizing<Vec<u8>>,
    new_params: KdfParams,
    new_salt: &[u8],
) -> Result<Vec<u8>, VaultWriteError> {
    let mut vault = VaultFile::decode(vault_bytes).map_err(|_| VaultWriteError::ReencryptFailed)?;
    let slot = vault
        .header
        .primary_slot()
        .ok_or(VaultWriteError::ReencryptFailed)?;

    // 仅 passphrase 来源有 KDF 参数 / salt 可换；其余来源无意义 → fail-closed。
    let old_params = slot.kdf_params.ok_or(VaultWriteError::ReencryptFailed)?;
    let old_salt = slot.salt.clone().ok_or(VaultWriteError::ReencryptFailed)?;

    // 旧参数派生旧主密钥 → 解出 data-key。
    let old_master = derive_master(passphrase, &old_salt, old_params)?;
    let data_key = crypto::unwrap_data_key(&old_master, &slot.nonce_i, &slot.wrapped_data_key)
        .map_err(|_| VaultWriteError::ReencryptFailed)?;

    // 新参数 + 新 salt 派生新主密钥 → 重包裹同一 data-key。
    let new_master = derive_master(passphrase, new_salt, new_params)?;
    let (slot_nonce, wrapped) = crypto::wrap_data_key(&new_master, &data_key)
        .map_err(|_| VaultWriteError::ReencryptFailed)?;

    let new_slot = Slot {
        source: slot.source,
        kdf_params: Some(new_params),
        salt: Some(new_salt.to_vec()),
        nonce_i: slot_nonce,
        wrapped_data_key: wrapped,
    };
    // 只重写包裹槽；payload nonce + ciphertext 逐字节保留。
    vault.header.slots = vec![new_slot];
    Ok(vault.encode())
}

/// 以 argon2id(passphrase, salt, params) 派生 32B 主密钥（rotate-kdf 内部用）。
fn derive_master(
    passphrase: &Zeroizing<Vec<u8>>,
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; 32]>, VaultWriteError> {
    let src = Passphrase::new(
        passphrase.clone(),
        salt.to_vec(),
        Argon2Params {
            m_cost: params.m_cost,
            t_cost: params.t_cost,
            p_cost: params.p_cost,
        },
    );
    src.obtain().map_err(|_| VaultWriteError::ReencryptFailed)
}
