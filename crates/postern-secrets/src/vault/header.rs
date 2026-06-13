//! vault 明文头（作为 AEAD 的 AAD 参与校验）（设计承诺级桩，函数体未实现）。
//!
//! 明文头 = 魔数 + `format_version` + 包裹槽（详细设计 5.4 / §7-6）。它**无敏感信息**
//! （data-key 在槽内是被包裹的密文），但**全段作为 payload AEAD 的 AAD**——任一字节
//! 篡改 / `format_version` 降级都使 payload 解密失败（L-2，`AadMismatch`）。
//!
//! 「随机 data-key + 包裹槽」两层结构（详细设计 5.4 / §3 rekey）：payload 始终由随机
//! data-key 加密，主密钥只加解密包裹槽里的 `wrapped_data_key`。rekey/rotate-kdf 只
//! 重写包裹槽、payload 密文一字不动（F-9）。单槽起步，结构预留多槽。
//!
//! 内存纪律：本结构只持有明文头**非敏感**字段（已包裹的 data-key 密文、salt、参数）；
//! 解出的明文 data-key 不在此持有（在 `crypto` 单元的 `Zeroizing` 容器内）。

use crate::vault::format::NONCE_LEN;

/// 解锁来源判别（写入包裹槽，标识该槽如何取主密钥）。仅 `passphrase` 槽带 KDF 参数
/// 与 salt（argon2id 仅作用于 passphrase 来源）；其余来源直接持有 32B 主密钥、无 KDF。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotSource {
    /// 经 argon2id KDF 派生主密钥（唯一带 `kdf_params`/`salt` 的来源）。
    Passphrase,
    /// 直接持有 32B 主密钥（文件系统权限保护）。
    KeyFile,
    /// 直接持有 32B 主密钥（OS 钥匙串保护）。
    OsKeychain,
    /// 直接持有 32B 主密钥（systemd 凭据保护）。
    SystemdCred,
}

/// argon2id 包裹参数（仅 `passphrase` 槽携带；rotate-kdf 即更换这些参数 + salt）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// 内存成本（KiB）。
    pub m_cost: u32,
    /// 迭代次数。
    pub t_cost: u32,
    /// 并行度。
    pub p_cost: u32,
}

/// 单个包裹槽：把随机 data-key 用主密钥（或其 KDF 派生）包裹后的密文及其元数据。
///
/// `wrapped_data_key` 是 data-key 经 XChaCha20-Poly1305 包裹的密文（含 tag）；`nonce_i`
/// 是包裹这一步的 24B nonce（与 payload nonce 独立，同样 CSPRNG、绝不复用，L-3）。
/// `kdf_params`/`salt` 仅 `passphrase` 槽为 `Some`，其余来源为 `None`（强度诚实）。
pub struct Slot {
    /// 该槽的解锁来源判别。
    pub source: SlotSource,
    /// argon2id 参数（仅 passphrase 槽 `Some`）。
    pub kdf_params: Option<KdfParams>,
    /// argon2id salt（仅 passphrase 槽 `Some`）。
    pub salt: Option<Vec<u8>>,
    /// 包裹 data-key 这一步的 24B nonce（CSPRNG，绝不复用）。
    pub nonce_i: [u8; NONCE_LEN],
    /// 被包裹的 data-key 密文（XChaCha20-Poly1305 输出，含 Poly1305 tag）。
    pub wrapped_data_key: Vec<u8>,
}

/// vault 明文头：魔数 + `format_version` + 包裹槽集合（单槽起步）。
///
/// 全段作为 payload AEAD 的 AAD（`format::VaultFile::aad_bytes`），其中 `format_version`
/// 的取值由 `format::FORMAT_VERSION` 校验——不识别即拒锁（L-1）。
pub struct Header {
    /// 文件格式版本（不识别即 `UnknownFormatVersion`，L-1）。
    pub format_version: u8,
    /// 包裹槽（单槽起步，结构预留多槽）。
    pub slots: Vec<Slot>,
}

impl Header {
    /// 取第一个（当前唯一）包裹槽。空槽集合是损坏头，返回 `None`（调用方据此 fail-closed）。
    pub fn primary_slot(&self) -> Option<&Slot> {
        self.slots.first()
    }
}
