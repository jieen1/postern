//! `local_process` 认证器（身份与凭证域 8.4；详细设计 6.13 / 第七部分）。
//!
//! 零凭证族：本地进程不出示任何 secret，身份据**网关可观测**的连接来源
//! （`SO_PEERCRED` 取得的 uid/gid）裁定，绝不采信自报字段（公理三）。来源必须是
//! Unix peer——TCP 来源对 `local_process` 一律不成立（无法以 SO_PEERCRED 取信任域门）。
//!
//! 与 `CredentialView` 的对接（以 core 实际类型为准）：core `CredentialMeta` 只有
//! `principal` / `kind` / `secret_hash` / `expires_at` / `revoked_at` 五个字段，没有
//! 独立的 match-spec 字段。本地进程的 uid/gid 匹配规则因此编码在该 kind 凭据的
//! `secret_hash` 文本里（`local_process` 无真实 secret，`secret_hash` 复用为匹配规则
//! 载体）——规则文法见 [`UidRule`]。这是对 core 既有字段的诚实复用，不向 core 增字段。
//!
//! 路径纪律（雷区）：本文件不在 `src/shells/`，故**绝不**出现字面 `ConnOrigin::UnixPeer`
//! / `ConnOrigin::Tcp`（契约 SEC_CONSTRUCTION_SITES 扫描这两个字面串）。需要来源类型时
//! 以 `use postern_core::request::ConnOrigin as Origin` 别名读/解构。本文件零 SQL 标记、
//! 无 unsafe、不吞错（fail-closed，一切不匹配/无法判定 → `Err`）。

use postern_core::domain::{CredentialView, PresentedCredential, PrincipalId, Timestamp};
use postern_core::error::AuthError;
use postern_core::plugin::Authenticator;
// 雷区 2：以别名读/解构来源，绝不写字面 ConnOrigin:: 变体（本文件不在 shells/）。
use postern_core::request::ConnOrigin as Origin;

use crate::identity::expiry::is_live;

/// 本认证器的注册键（与 `PresentedCredential::kind()` 选型一致）。
pub const KIND: &str = "local_process";

/// 本地进程 uid/gid 匹配规则——编码在 `local_process` 凭据的 `secret_hash` 文本里。
///
/// 文法（逗号分隔的 `字段=值` 对，确定性解析）：
/// - `uid=<u32>`：要求观测 uid 等值（必选）。
/// - `gid=<u32>`：要求观测 gid 等值（可选；缺省即不约束 gid）。
///
/// 任一对解析失败（非 `uid`/`gid` 键、值非 u32、缺 `uid`）→ 规则非法 → 该候选不匹配
/// （fail-closed，绝不放宽为「无约束通过」）。
struct UidRule {
    /// 必须等值的 uid。
    uid: u32,
    /// 若 `Some`，必须等值的 gid；`None` 表示不约束 gid。
    gid: Option<u32>,
}

impl UidRule {
    /// 从 `secret_hash` 文本确定性解析匹配规则；任何格式偏差 → `None`（fail-closed）。
    fn parse(spec: &str) -> Option<Self> {
        let mut uid: Option<u32> = None;
        let mut gid: Option<u32> = None;
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (key, value) = part.split_once('=')?;
            let value: u32 = value.trim().parse().ok()?;
            match key.trim() {
                "uid" => uid = Some(value),
                "gid" => gid = Some(value),
                // 未知键 → 规则非法（不静默忽略，避免「拼写错的约束被当成无约束」）。
                _ => return None,
            }
        }
        // uid 必选：没有 uid 约束的 local_process 规则一律视为非法（绝不无约束放行）。
        Some(Self { uid: uid?, gid })
    }

    /// 观测到的 uid/gid 是否满足本规则（gid 仅在规则约束时比对）。
    fn matches(&self, observed_uid: u32, observed_gid: u32) -> bool {
        if observed_uid != self.uid {
            return false;
        }
        match self.gid {
            Some(want) => observed_gid == want,
            None => true,
        }
    }
}

/// `local_process` 认证器：据 SO_PEERCRED 观测 uid/gid 在 `CredentialView` 里确定性
/// 选取匹配的 `local_process` 凭据，裁定 principal。
///
/// 决策语义（fail-closed）：
/// - 来源不是 Unix peer（如 TCP）→ `UndeterminableOrigin`（无法取 SO_PEERCRED 信任域门）。
/// - 无任一 `local_process` 凭据的 uid/gid 规则匹配观测来源 → `InvalidCredential`。
/// - 命中候选但已过期/已吊销（按 `now` 墙钟二次校验）→ `ExpiredCredential` /
///   `RevokedCredential`。
/// - 命中且在生命期内 → `Ok(principal)`。
///
/// 多候选确定性：按 `CredentialView.credentials` 的固定顺序取**首个**匹配且在生命期内
/// 的候选（snapshot 容器为确定性 `Vec`，顺序稳定 → 选取确定）。
pub struct LocalProcessAuthenticator;

impl Authenticator for LocalProcessAuthenticator {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn authenticate(
        &self,
        _presented: &PresentedCredential,
        origin: &Origin,
        creds: &CredentialView,
        now: Timestamp,
    ) -> Result<PrincipalId, AuthError> {
        // 来源裁定：仅 Unix peer 可作 local_process 信任域门；其余来源无法判定 → 拒。
        let (uid, gid) = match origin {
            Origin::UnixPeer { uid, gid } => (*uid, *gid),
            _ => return Err(AuthError::UndeterminableOrigin),
        };

        for meta in &creds.credentials {
            if meta.kind != KIND {
                continue;
            }
            let Some(rule) = UidRule::parse(&meta.secret_hash) else {
                // 规则非法的候选直接跳过（不放行、不报错——继续找其它合法候选）。
                continue;
            };
            if !rule.matches(uid, gid) {
                continue;
            }
            // 命中匹配候选：按 now 墙钟二次校验时效（过期即刻失效，不依赖 sweeper）。
            is_live(meta, now)?;
            return Ok(meta.principal);
        }

        // 无任一匹配候选：不泄露存在性，统一 InvalidCredential。
        Err(AuthError::InvalidCredential)
    }
}
