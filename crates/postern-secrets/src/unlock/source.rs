//! `MasterKeySource` trait 定义（本域，D6 / 详细设计 4.3 / §5.2）。
//!
//! 解锁材料来源抽象：载体唯一（vault 文件），解锁方式按部署形态选择。统一接口
//! 暴露单一同步操作 `obtain`，取得 32 字节主密钥（`Zeroizing` 持有，离作用域清零）。
//!
//! 各来源真实强度不同（§5.2 诚实表）：仅 `passphrase` 经 argon2id KDF 派生；
//! `key_file`/`os_keychain`/`systemd_cred` 直接持有 32B 主密钥、无 KDF 加固——
//! 强度等于文件系统权限 / OS 钥匙串 / systemd 凭据保护。trait 本身不区分来源强度，
//! 强度差异落在各实现的类型与文档注释上（契约 B-8：解锁强度诚实表述）。

use zeroize::Zeroizing;

use crate::error::UnlockError;

/// 解锁材料来源（D6）：载体唯一，解锁方式按部署形态选择（详细设计 4.3 / §5.2）。
///
/// `obtain` 同步、无 async（解锁是启动期单次成本，不在请求热路径）；成功返回
/// `Zeroizing<[u8; 32]>` 主密钥，失败 fail-closed 返回 `UnlockError`（绝不返回
/// 缺省 / 全零密钥，签名层即无"解锁失败仍放行"路径）。
///
/// 真实强度差异由各实现承载，不在 trait 签名表达（§5.2 强度表、§7-7 解锁强度诚实）。
pub trait MasterKeySource {
    /// 取得 32 字节主密钥。同步执行；任何失败（KDF 派生失败 / 材料不可得）一律
    /// map 成 `UnlockError` 返回，绝不 unwrap / panic（B-6 lint 红线）。
    fn obtain(&self) -> Result<Zeroizing<[u8; 32]>, UnlockError>;
}
