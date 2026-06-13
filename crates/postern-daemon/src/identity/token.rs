//! `token` 认证器（身份与凭证域 8.4）。
//!
//! 网络凭据族（同 api_key 形态）：Agent 出示 token 字节，经 argon2id 与匹配凭据的
//! `secret_hash`（PHC 串）verify 裁定 principal；可信域门为 [`TrustDomain::Network`]
//! （仅 TCP 观测来源相符）。认证内核见 [`SecretAuthenticator`]，本文件只固定 kind 与
//! 可信域门、给装配点一个零参构造助手。

use crate::identity::secret_auth::{SecretAuthenticator, TrustDomain};

/// 本认证器的注册键（与 `PresentedCredential::kind()` 选型一致）。
pub const KIND: &str = "token";

/// 装配 `token` 认证器：argon2id verify + `Network` 可信域门。
pub fn authenticator() -> SecretAuthenticator {
    SecretAuthenticator::new(KIND, TrustDomain::Network)
}
