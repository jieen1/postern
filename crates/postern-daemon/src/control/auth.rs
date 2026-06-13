//! 控制面认证：control.sock 的接入门（模块文档 06 §6.5 / §8 L-1）。
//!
//! control.sock 以 **0600** 权限创建（仅同 uid 可连，stat mode 恒 0600）；认证中间件 front
//! 所有控制面端点：先 SO_PEERCRED uid 比对（即便对端与本进程同 uid 也要比对——裸的同 uid
//! connect **不**自动放行），**再叠**一个控制面本地凭据校验。两者**皆必需**：缺任一即
//! fail-closed 拒绝（L-1）。控制面不采信请求自报身份。
//!
//! 雷区纪律：本文件经 SO_PEERCRED 取对端 uid，**比对只用 `(uid)` 直接比**，不构造任何字面
//! 来源类型；若确需持来源类型，以 `use postern_core::request::ConnOrigin as Origin` 别名读/
//! 解构（control/ 非 shells，写字面 `ConnOrigin::` 变体即违规）。
//!
//! 认证判定：两支（uid 比对 + 本地凭据）皆必需，逐支 fail-closed，二者皆满足才放行。

/// 控制面认证判定的两支必需条件（L-1）：缺任一即拒。
///
/// 用于把"uid 比对"与"本地凭据校验"两件事钉成**各自独立、皆必需**：测试逐支注入失败，
/// 断言任一支失败即整体拒绝（裸同 uid 无凭据 ⇒ 拒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthReject {
    /// SO_PEERCRED uid 与本进程 uid 不符（跨信任域）。
    PeerUidMismatch,
    /// 控制面本地凭据缺失 / 不符（即便同 uid 也必须出示）。
    MissingControlCredential,
}

/// 校验一个控制面请求的认证：SO_PEERCRED 对端 uid **与**控制面本地凭据二者皆必需（L-1）。
///
/// `peer_uid` 由 listener 经 `tokio::net::UnixStream::peer_cred`（安全 API，无 unsafe）取得后
/// 传入——本函数只对 `(peer_uid)` 与 `self_uid` 直接比对，绝不构造来源类型。`credential_ok`
/// 为控制面本地凭据校验结果。任一不满足 ⇒ `Err(AuthReject)`（fail-closed）；二者皆满足才
/// 放行。**裸的同 uid 且无凭据**（`credential_ok == false`）必返
/// `Err(AuthReject::MissingControlCredential)`。
pub fn authenticate(peer_uid: u32, self_uid: u32, credential_ok: bool) -> Result<(), AuthReject> {
    // 两支皆必需（L-1），逐支 fail-closed：
    // ① SO_PEERCRED uid 比对——即便同 uid 也先比对（裸同 uid 绝不旁路认证）。
    if peer_uid != self_uid {
        return Err(AuthReject::PeerUidMismatch);
    }
    // ② 控制面本地凭据校验——uid 相符仍须出示凭据。
    if !credential_ok {
        return Err(AuthReject::MissingControlCredential);
    }
    // 二者皆满足才放行。
    Ok(())
}
