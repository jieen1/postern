//! key_file 来源：**直接持有 32B 主密钥**，无 KDF（§5.2 / 详细设计 5.4）。
//!
//! 真实强度（诚实表述，§5.2 表 + §7-7、契约 B-8）：**等于文件系统权限**
//! （与无口令 SSH 私钥同级），**无 KDF 加固**；任何能读到 key_file 的主体即可解锁
//! 全部凭据。弱模式威胁后果须显式声明——`key_file` **不作简单默认推荐**。
//!
//! 依赖纪律（雷区）：本实现**不得引入 argon2**（强度诚实：无 KDF 加固，argon2 仅
//! 在 passphrase 实现使用）。主密钥以 `Zeroizing` 持有，离作用域清零；不 Debug 出明文。

use zeroize::Zeroizing;

use crate::error::UnlockError;
use crate::unlock::source::MasterKeySource;

/// key_file 解锁来源：直接持有从 key_file 读入的 32B 主密钥，无 KDF。
///
/// 行为承诺（§8 F-2）：**给定 32B 主密钥 → `obtain` 原样返回该 32B**（直接持有、
/// 无派生、无变换）。强度等于 key_file 的文件系统权限保护，不夸大为「argon2id 保护」。
///
/// 主密钥以 `Zeroizing` 持有，不 derive `Debug`（避免明文外泄）。
pub struct KeyFile {
    master_key: Zeroizing<[u8; 32]>,
}

impl KeyFile {
    /// 以从 key_file 读入的 32B 主密钥构造（直接持有，无 KDF）。
    pub fn new(master_key: Zeroizing<[u8; 32]>) -> Self {
        Self { master_key }
    }
}

impl MasterKeySource for KeyFile {
    fn obtain(&self) -> Result<Zeroizing<[u8; 32]>, UnlockError> {
        // 直接持有：原样返回所持 32B 主密钥（无 KDF、无变换）。
        // 副本同以 Zeroizing 持有，离调用方作用域清零。
        Ok(Zeroizing::new(*self.master_key))
    }
}
