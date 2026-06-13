//! 凭据时效二次校验：`expires_at` / `revoked_at` 在求值时刻（`now`）按墙钟判定
//! （身份与凭证域 8.4；详细设计 6.2）。
//!
//! 三族认证器共用：过期/吊销即刻失效，不依赖后台 sweeper 的清扫时序——校验只看
//! 传入的 `now`（确定性），不读系统时钟。fail-closed：过期 → `ExpiredCredential`，
//! 已吊销 → `RevokedCredential`。
//!
//! 路径纪律：本文件零 SQL 标记、无 unsafe、不构造任何机密/来源类型、不吞错。

use postern_core::domain::{CredentialMeta, Timestamp};
use postern_core::error::AuthError;

/// 按 `now` 墙钟二次校验候选凭据的时效；在生命期内返回 `Ok(())`，否则 fail-closed `Err`。
///
/// 语义（详细设计 6.2）：
/// - `expires_at` 存在且 `now >= expires_at` → 已过期（边界即刻失效）→ `ExpiredCredential`。
/// - `revoked_at` 存在且 `now >= revoked_at` → 已吊销 → `RevokedCredential`。
///
/// 两者都看 `now`：过期/吊销在求值时刻立即生效，绝不等 sweeper。先判过期再判吊销，
/// 顺序固定（确定性归因）。
pub fn is_live(meta: &CredentialMeta, now: Timestamp) -> Result<(), AuthError> {
    if let Some(expires_at) = meta.expires_at {
        if now >= expires_at {
            return Err(AuthError::ExpiredCredential);
        }
    }
    if let Some(revoked_at) = meta.revoked_at {
        if now >= revoked_at {
            return Err(AuthError::RevokedCredential);
        }
    }
    Ok(())
}
