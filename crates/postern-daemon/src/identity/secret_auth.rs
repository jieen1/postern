//! 基于 secret 的认证器内核（`api_key` / `token` 共用；身份与凭证域 8.4）。
//!
//! `api_key` 与 `token` 的认证形态同构——presented 出示的 secret 字节经 argon2id 与
//! 匹配 kind 凭据的 `secret_hash`（PHC 串）verify，命中且在生命期内 + 可信域相符即裁定
//! principal。两者仅 kind 与可信域门不同，故抽出本内核 [`SecretAuthenticator`]，两族各
//! 以不同 `kind` + [`TrustDomain`] 配置实例化。
//!
//! 可信域校验（详细设计第七部分 / 6.13，「据可观测 origin 校验，不采信自报」）：core
//! `CredentialMeta` 无独立 trust_domain 字段，故可信域是**认证器实例**的配置门（网关装配
//! 时按凭据族信任策略设定），据**观测到的** [`Origin`] 分类判定，绝不读请求自报字段——
//! 网络凭据（api_key/token）来自网络面、本地凭据来自本地 peer，可信域门即表达这一边界。
//!
//! secret 比对纪律：argon2id verify 自身即常量时间口令比对（`password-hash` 的
//! `verify_password`），presented secret 与中间态不落日志、`PresentedCredential::Debug`
//! 恒 `REDACTED`；本文件**绝不** `tracing`/`format!` 出 secret 或 hash。
//!
//! 路径纪律（雷区）：本文件不在 `src/shells/`，**绝不**出现字面 `ConnOrigin::UnixPeer`
//! / `ConnOrigin::Tcp`（以 `Origin` 别名读/解构）。零 SQL 标记、无 unsafe、不吞错
//! （一切失败显式 → `Err`，fail-closed）。

use argon2::password_hash::{PasswordHash, PasswordVerifier};
use argon2::Argon2;

use postern_core::domain::{CredentialView, PresentedCredential, PrincipalId, Timestamp};
use postern_core::error::AuthError;
use postern_core::plugin::Authenticator;
// 雷区 2：以别名读/解构来源，绝不写字面 ConnOrigin:: 变体。
use postern_core::request::ConnOrigin as Origin;

use crate::identity::expiry::is_live;

/// 凭据族可信域门：据**观测**来源裁定该 secret 凭据是否来自允许的可信域。
///
/// 不采信自报字段——只读网关采集的 [`Origin`]（uid/gid 或 TCP 对端）。两个门覆盖
/// api_key/token 的典型部署：
/// - [`TrustDomain::Network`]：凭据为网络凭据，仅 TCP 来源相符（远程 Agent 凭密钥接入）。
/// - [`TrustDomain::LocalUnix`]：凭据仅供本地 peer 使用，仅 Unix peer 来源相符。
#[derive(Clone, Copy)]
pub enum TrustDomain {
    /// 仅 TCP 观测来源相符（网络凭据）。
    Network,
    /// 仅 Unix peer 观测来源相符（本地凭据）。
    LocalUnix,
}

impl TrustDomain {
    /// 观测来源是否落在本可信域内（据观测 origin，不采信自报）。
    fn admits(self, origin: &Origin) -> bool {
        matches!(
            (self, origin),
            (TrustDomain::Network, Origin::Tcp { .. })
                | (TrustDomain::LocalUnix, Origin::UnixPeer { .. })
        )
    }
}

/// 基于 secret 的认证器（argon2id verify + 可信域门 + now 时效二次校验）。
///
/// 决策语义（fail-closed，顺序固定→确定性归因）：
/// 1. 观测来源不落配置可信域 → `TrustDomainMismatch`（先于任何 secret 比对，避免在错误
///    可信域上做无谓 verify）。
/// 2. 遍历 `CredentialView` 中 kind 相符的候选，按固定顺序取**首个** argon2id verify 通过
///    且在生命期内者：
///    - verify 通过 → 按 `now` 校验 `expires_at`/`revoked_at`：过期 → `ExpiredCredential`，
///      吊销 → `RevokedCredential`，在生命期内 → `Ok(principal)`。
/// 3. 无任一候选 verify 通过 → `InvalidCredential`（不泄露存在性）。
pub struct SecretAuthenticator {
    /// 注册键 / 选型 kind（`api_key` 或 `token`）。
    kind: &'static str,
    /// 本族凭据的可信域门（据观测来源判定）。
    trust_domain: TrustDomain,
}

impl SecretAuthenticator {
    /// 以 kind 与可信域门构造（网关装配点按凭据族策略设定）。
    pub fn new(kind: &'static str, trust_domain: TrustDomain) -> Self {
        Self { kind, trust_domain }
    }
}

impl Authenticator for SecretAuthenticator {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn authenticate(
        &self,
        presented: &PresentedCredential,
        origin: &Origin,
        creds: &CredentialView,
        now: Timestamp,
    ) -> Result<PrincipalId, AuthError> {
        // [1] 可信域门先行：观测来源不落配置可信域即拒（据观测 origin，不采信自报）。
        if !self.trust_domain.admits(origin) {
            return Err(AuthError::TrustDomainMismatch);
        }

        let presented_secret = presented.secret();
        let verifier = Argon2::default();

        for meta in &creds.credentials {
            if meta.kind != self.kind {
                continue;
            }
            // secret_hash 是 argon2id PHC 串；解析失败的候选跳过（不放行、不报错）。
            let Ok(parsed) = PasswordHash::new(&meta.secret_hash) else {
                continue;
            };
            // argon2id verify（常量时间口令比对，参数取自 stored PHC 串自身）。
            if verifier
                .verify_password(presented_secret, &parsed)
                .is_err()
            {
                continue;
            }
            // verify 通过：按 now 墙钟二次校验时效（过期/吊销即刻失效）。
            is_live(meta, now)?;
            return Ok(meta.principal);
        }

        // 无任一候选 verify 通过：统一 InvalidCredential（不泄露存在性）。
        Err(AuthError::InvalidCredential)
    }
}
