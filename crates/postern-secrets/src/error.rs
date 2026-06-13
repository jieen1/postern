//! 机密面错误词汇基底。
//!
//! 本域对外的两个 thiserror 错误枚举：`UnlockError`（保险箱解锁失败面）与
//! `ResolveError`（代号→真实地址解析失败面）。资源凭据物化失败复用 core 的
//! `CredentialError`（在本域不重定义，import 即可）。
//!
//! 设计承诺（docs/modules/03-postern-secrets.md §3.1、§6.2/§6.3、§8 L-11；
//! 详细设计 4.3 / 5.4 / 7.2-1）：
//! - 每变体只携带常量英文文案与错误码判别，绝不内嵌真实地址 / 凭据 / 账号明文，
//!   也绝不插值任何外部输入（红线 7.2-1：跨 crate 边界前已脱敏）。
//! - `Display` / `Debug` 文案恒为编译期常量字符串，无格式化占位符。

use thiserror::Error;

/// 保险箱解锁失败面（`MasterKeySource::obtain` + vault 解锁路径）。
///
/// 覆盖解锁的各 fail-closed 分支（§8 F-1 / L-1 / L-2、§6.2）：主密钥获取失败、
/// `format_version` 不识别、AAD 校验失败、payload 解密失败、KDF 派生失败。
/// 任一变体出现即不返回保险箱句柄、数据面不开放。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UnlockError {
    /// `MasterKeySource::obtain` 未能取得 32 字节主密钥。
    #[error("master key source unavailable")]
    ObtainFailed,
    /// 明文头 `format_version` 不被当前实现识别（L-1，fail-closed，绝不按未知格式尝试解密）。
    #[error("vault format version not recognized")]
    UnknownFormatVersion,
    /// 明文头全段作为 AEAD AAD 的校验失败（头部篡改 / 版本降级，L-2）。
    #[error("vault header integrity check failed")]
    AadMismatch,
    /// 以 data-key 解密 payload 失败（密文损坏 / data-key 错误）。
    #[error("vault payload decryption failed")]
    PayloadDecryptFailed,
    /// argon2id KDF 派生主密钥失败（仅 passphrase 来源涉及 KDF）。
    #[error("key derivation failed")]
    KdfFailed,
    /// argon2id KDF 参数越界（m_cost/t_cost/p_cost 超出本实现接受的安全上限，L-2/fail-closed）。
    /// 被篡改的保险箱文件可把 m_cost 写成 GB/TB 级病态值——本实现在调用 argon2 **之前**
    /// 拒绝，绝不据此申请大内存（防 unlock 期 OOM 拒绝服务）。
    #[error("key derivation parameters out of accepted range")]
    KdfParamsOutOfRange,
}

/// 代号→真实地址解析失败面（`resolve(code) -> Result<ResolvedTarget, ResolveError>`）。
///
/// 与 `CredentialError` 同构的 fail-closed 失败返回（§8 F-4 / L-5、§6.3）：
/// 未知代号、保险箱不可用一律返回 `Err`、无产物，签名层即无"缺省地址"返回路径。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolveError {
    /// 请求的资源代号在 `targets` 段无映射。
    #[error("no target for requested resource code")]
    UnknownCode,
    /// 保险箱已锁定或不可用，无法解析。
    #[error("vault unavailable")]
    VaultUnavailable,
}
