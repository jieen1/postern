//! 保险箱面：vault 文件格式、解锁、写入（设计承诺级桩，函数体未实现）。
//!
//! 解锁流程（§3 / §6.2 / F-1）：`MasterKeySource::obtain` 取 32B 主密钥 → 用主密钥从
//! **包裹槽**解出随机 **data-key** → 以 data-key 经 XChaCha20-Poly1305 **解密 payload**，
//! **明文头全段作 AEAD AAD**。`format_version` 不识别即拒锁（L-1）；明文头任一字节篡改 /
//! 版本降级使 AAD 校验失败拒锁（L-2）；data-key 错误 / 密文损坏使 payload 解密失败拒锁。
//!
//! fail-closed（§6.2 / L-6）：解锁未成功 → **不返回保险箱句柄**——数据面在保险箱可用前
//! 不接受任何请求。一切失败 map 成 `UnlockError`，绝不 unwrap / panic（B-6）。
//!
//! 边界纪律：本单元只解出 payload 明文映射并以 `UnlockedVault` 句柄持有；据 `(code,tier)`
//! 物化成 `ResourceCredential`/`ResolvedTarget` 归 mapping/provider 单元——**本单元不构造
//! 这两个类型**。

pub mod crypto;
pub mod format;
pub mod header;
pub mod payload;
pub mod write;

use crate::error::UnlockError;
use crate::vault::payload::Payload;

/// 解锁成功后的保险箱句柄——持有解出的 payload 明文映射（`Zeroizing`）。
///
/// 这是 §6.2「交回 boot 装配」的可用保险箱句柄来源；mapping/provider 单元据它物化机密
/// 类型。回读对外只经掩码 / `vault://` 引用（`Payload` 的回读方法），绝不回吐明文。
pub struct UnlockedVault {
    /// 解锁后的 payload 明文（两段映射，`Zeroizing` 持有）。
    payload: Payload,
}

impl UnlockedVault {
    /// 借用解出的 payload（供同 crate 的 mapping/provider 单元据此物化机密类型）。
    pub fn payload(&self) -> &Payload {
        &self.payload
    }
}

/// 解锁 vault：给定 32B 主密钥与整 vault 文件字节，解出 payload 并返回可用保险箱句柄。
///
/// 步骤：`decode` 文件（校验魔数 + `format_version`，L-1）→ `unwrap_data_key`（主密钥
/// 开包裹槽）→ `decrypt_payload`（data-key + 明文头 AAD，L-2）→ `Payload::from_plaintext`。
///
/// fail-closed（F-1 / L-1 / L-2 / L-6）：任一步失败返回 `Err(UnlockError)`、**不返回句柄**。
pub fn unlock(master_key: &[u8; 32], vault_bytes: &[u8]) -> Result<UnlockedVault, UnlockError> {
    // 1) decode：校验魔数 + format_version（不识别即 UnknownFormatVersion，L-1）。
    let vault = format::VaultFile::decode(vault_bytes)?;

    // 2) 开包裹槽：主密钥解出随机 data-key（错误主密钥 / 槽损坏 → PayloadDecryptFailed）。
    let slot = vault
        .header
        .primary_slot()
        .ok_or(UnlockError::PayloadDecryptFailed)?;
    let data_key = crypto::unwrap_data_key(master_key, &slot.nonce_i, &slot.wrapped_data_key)?;

    // 3) 解密 payload：明文头前缀（原始字节）作 AAD（头部篡改 / 版本降级 → AadMismatch，L-2）。
    let aad = format::aad_slice(vault_bytes)?;
    let plaintext =
        crypto::decrypt_payload(&data_key, &vault.payload_nonce, &vault.ciphertext, aad)?;

    // 4) 解析两段 JSON payload（结构不符 / 截断 → PayloadDecryptFailed）。
    let payload = Payload::from_plaintext(&plaintext)?;

    Ok(UnlockedVault { payload })
}
