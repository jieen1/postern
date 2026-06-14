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
//!
//! 中间件接线（[`control_auth`]）：axum `from_fn_with_state` 把本门 front 全部控制面端点。
//! 对端 uid 由控制面 listener（shells/）经 SO_PEERCRED 采集后经请求扩展 [`PeerUid`] 注入
//! （本文件只读 `u32`、绝不构造来源类型）；控制面本地凭据（control-token）由请求头
//! [`CONTROL_TOKEN_HEADER`] 携带，与 boot 装配期从 token 文件读入的期望值常量时间比对。
//! `GET /v1/health` 豁免 token 第二因子（运维探活），但**仍过 peer 门**（uid 必相符）。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::control::dto::ApiErrorBody;

/// 控制面本地凭据（control-token）的请求头名（第二因子，L-1）。
///
/// 中间件从此头取出客户端出示的 token，与 boot 装配期从 0600 token 文件读入的期望值常量
/// 时间比对；缺头 / 不符 ⇒ `credential_ok=false` ⇒ [`AuthReject::MissingControlCredential`]。
pub const CONTROL_TOKEN_HEADER: &str = "x-postern-control-token";

/// 豁免 token 第二因子的端点路径（运维探活）——**仍过 peer 门**（uid 必相符，L-1）。
///
/// 仅 `GET /v1/health` 豁免 control-token：它不读写策略、不触机密，运维据其对账控制面健康
/// 但 uid 主门绝不豁免（裸跨信任域连接即便打 health 也拒）。
const TOKEN_EXEMPT_PATH: &str = "/v1/health";

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

/// 由控制面 listener（shells/）经 SO_PEERCRED 采集、经请求扩展注入 handler 链的对端 uid。
///
/// 控制面 serve 路径在每条连接 accept 后取 `peer_cred().uid()`（唯一可信来源事实），把它经
/// `Extension(PeerUid)` 注入该连接每条请求的扩展集；中间件只读此 `u32` 与 `self_uid` 直比，
/// **绝不**构造来源类型（control/ 非 shells）。缺该扩展（非控制面 serve 路径 / 来源采集缺失）
/// ⇒ 无可信对端事实 ⇒ fail-closed 拒（绝不退化为采信自报或默认放行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerUid(pub u32);

/// 由已认证对端 uid 派生写操作者标识（[`Actor::Operator`]）——control.sock 写的 `created_by` /
/// `updated_by` 取值来源。
///
/// control.sock 经 0600 + SO_PEERCRED uid 主门 + control-token 第二因子认证（[`authenticate`]），
/// 故对端 OS uid 即「是谁在写」的**可信**标识。形态 `uid:<n>`（稳定、可对账、无机密）——
/// 控制面 serve 路径据此注入 `Extension(Actor)`，写 handler 读它填审计五字段，生产路径绝不再
/// 退化为空串 / 常量操作者（审计可追溯「谁写了」，而非空白）。
pub fn operator_of_peer(peer_uid: u32) -> crate::control::Actor {
    crate::control::Actor::Operator(format!("uid:{peer_uid}"))
}

/// 控制面认证中间件的注入状态（boot 装配期一次性物化）。
///
/// `self_uid` 为 daemon 自身 uid（经 SO_PEERCRED 自连对取得，[`crate::boot`]）；`token` 为
/// 从 0600 control-token 文件读入的期望凭据字节，`None` 表示**无 token 文件**——此时
/// `credential_ok` 恒 `false`，所有需 token 的端点一律 fail-closed 拒（缺凭据绝不放行）。
#[derive(Clone)]
pub struct ControlAuth {
    /// daemon 自身 uid（peer 主门比对基准）。
    pub self_uid: u32,
    /// control-token 期望字节；`None` ⇒ 无 token 文件 ⇒ credential_ok 恒 false（fail-closed）。
    pub token: Option<Arc<Vec<u8>>>,
}

impl ControlAuth {
    /// 由自身 uid + 期望 token（`None` = 无 token 文件）装配认证状态。
    pub fn new(self_uid: u32, token: Option<Vec<u8>>) -> Self {
        Self {
            self_uid,
            token: token.map(Arc::new),
        }
    }

    /// 常量时间比对出示 token 与期望 token：长度不等或无期望 token ⇒ `false`（fail-closed）。
    ///
    /// 无期望 token（无 token 文件）⇒ 恒 `false`（缺凭据绝不放行）；出示缺失同理由调用方传空
    /// 切片落到长度不等 ⇒ `false`。比对全长按位异或累加，不因首个不等字节短路（杜绝计时侧信道）。
    fn credential_ok(&self, presented: &[u8]) -> bool {
        let expected = match &self.token {
            Some(t) => t.as_slice(),
            None => return false,
        };
        if expected.len() != presented.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in expected.iter().zip(presented.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

/// 控制面认证中间件（axum `from_fn_with_state`）：front 全部控制端点，逐支 fail-closed（L-1）。
///
/// 主门（peer uid）：从请求扩展取 [`PeerUid`]（控制面 serve 经 SO_PEERCRED 注入）；缺该扩展即
/// 无可信对端事实 ⇒ fail-closed 拒。第二因子（control-token）：从 [`CONTROL_TOKEN_HEADER`] 取
/// 出示 token，常量时间比对期望值；`GET /v1/health` 豁免此因子（但**仍过 peer 门**）。两支经
/// [`authenticate`] 合判，`Err(AuthReject)` ⇒ 对应 HTTP 拒绝码 + 错误信封（脱敏、无机密）。
pub async fn control_auth(
    State(auth): State<ControlAuth>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // 主门：取经 SO_PEERCRED 注入的对端 uid。缺该扩展 ⇒ 无可信来源事实 ⇒ fail-closed 拒
    // （绝不退化为采信自报；非控制面 serve 路径不应触达本中间件）。
    let peer_uid = match request.extensions().get::<PeerUid>() {
        Some(PeerUid(uid)) => *uid,
        None => return reject(AuthReject::PeerUidMismatch),
    };

    // 第二因子：health 豁免 token（仍过 peer 门）；其余端点须出示并匹配 control-token。
    let credential_ok = if request.uri().path() == TOKEN_EXEMPT_PATH {
        true
    } else {
        let presented = request
            .headers()
            .get(CONTROL_TOKEN_HEADER)
            .map(|v| v.as_bytes().to_vec())
            .unwrap_or_default();
        auth.credential_ok(&presented)
    };

    // 两支合判（L-1）：任一不满足即 fail-closed 拒，二者皆满足才放行下游 handler。
    match authenticate(peer_uid, auth.self_uid, credential_ok) {
        Ok(()) => next.run(request).await,
        Err(reason) => reject(reason),
    }
}

/// 把一支认证失败映射为 HTTP 拒绝响应（脱敏错误信封，无机密细节）。
///
/// `PeerUidMismatch`（跨信任域）⇒ **403 Forbidden**；`MissingControlCredential`（缺/错凭据）
/// ⇒ **401 Unauthorized**。两者皆 fail-closed，错误信封恒为常量码 / 文案，不回显请求与机密。
fn reject(reason: AuthReject) -> Response {
    let (status, code, message) = match reason {
        AuthReject::PeerUidMismatch => (
            StatusCode::FORBIDDEN,
            "peer_uid_mismatch",
            "peer uid does not match the daemon uid",
        ),
        AuthReject::MissingControlCredential => (
            StatusCode::UNAUTHORIZED,
            "missing_control_credential",
            "missing or invalid control credential",
        ),
    };
    (status, Json(ApiErrorBody::new(code, message))).into_response()
}
