//! systemd_cred 来源：**直接持有 32B 主密钥**，无 KDF（§5.2 / 详细设计 5.4）。
//!
//! 真实强度（诚实表述，§5.2 表 + §7-7、契约 B-8）：**等于 systemd 凭据保护**
//! （可走 TPM 封存），**无 KDF 加固**——不夸大为「argon2id 保护」。适用无人值守、
//! 服务化形态。
//!
//! 依赖纪律（雷区）：本实现**不得引入 argon2**（强度诚实：无 KDF 加固）。主密钥以
//! `Zeroizing` 持有，离作用域清零；不 Debug 出明文。测试不碰真实 systemd/TPM——以
//! 固定 32B 构造 Fake 驱动（§3.1 测试策略）。

use zeroize::Zeroizing;

use crate::error::UnlockError;
use crate::unlock::source::MasterKeySource;

/// systemd_cred 解锁来源：直接持有 systemd 凭据（可经 TPM 封存）交付的 32B 主密钥，无 KDF。
///
/// 行为承诺（§8 F-2）：**给定 32B 主密钥 → `obtain` 原样返回该 32B**（直接持有、
/// 无派生）。强度等于 systemd 凭据保护，不夸大。
///
/// 主密钥以 `Zeroizing` 持有，不 derive `Debug`（避免明文外泄）。
pub struct SystemdCred {
    master_key: Zeroizing<[u8; 32]>,
}

impl SystemdCred {
    /// 以 systemd 凭据交付的 32B 主密钥构造（直接持有，无 KDF）。
    pub fn new(master_key: Zeroizing<[u8; 32]>) -> Self {
        Self { master_key }
    }
}

impl MasterKeySource for SystemdCred {
    fn obtain(&self) -> Result<Zeroizing<[u8; 32]>, UnlockError> {
        // 直接持有：原样返回所持 32B 主密钥（无 KDF、无变换）。
        // 副本同以 Zeroizing 持有，离调用方作用域清零。
        Ok(Zeroizing::new(*self.master_key))
    }
}
