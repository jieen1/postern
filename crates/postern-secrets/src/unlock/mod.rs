//! 解锁面：`MasterKeySource` 四来源取主密钥（§5.2 / 详细设计 4.3）。
//!
//! trait 定义于 `source`，四实现各自一文件：仅 `passphrase` 经 argon2id KDF 派生；
//! `key_file`/`os_keychain`/`systemd_cred` 直接持有 32B 主密钥、无 KDF。强度差异如实
//! 落到各实现的类型与文档注释（契约 B-8：解锁强度诚实表述）。

pub mod key_file;
pub mod os_keychain;
pub mod passphrase;
pub mod source;
pub mod systemd_cred;

pub use source::MasterKeySource;
