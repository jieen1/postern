//! D2b 控制面认证链行为测试（RED→GREEN）——钉死 control.sock 接入门两支皆必需（§8 L-1）。
//!
//! 控制面认证门 front 全部控制端点：**SO_PEERCRED uid 主门**（即便对端同 uid 也比对，裸同 uid
//! 绝不旁路）**再叠** control-token 第二因子；两支皆必需，缺任一即 fail-closed 拒。`GET /v1/health`
//! 豁免 token（运维探活）**但仍过 peer 门**（uid 必相符）。本测试钉四条：
//! 1. **peer 不符拒**：peer_uid≠self_uid ⇒ 403（即便携带正确 token，主门先拒）。
//! 2. **缺 token 拒**：peer 相符但缺 / 错 token ⇒ 401（uid 对仍须出示凭据）。
//! 3. **对 token 放行**：peer 相符 + 正确 token ⇒ 过认证门（非 401/403，达下游 handler）。
//! 4. **health 豁免 token 仍过 peer 门**：peer 相符 + 无 token ⇒ health 200；peer 不符 ⇒ 仍 403。
//!
//! 驱动方式（06 §9）：以内存 Fake 句柄装配 [`ControlState`] → [`router`] → [`with_control_auth`]
//! front 认证门 → `tower::ServiceExt::oneshot` 打请求。生产由
//! [`serve_control_router_over_uds`](postern_daemon::shells::serve::serve_control_router_over_uds)
//! 逐连接经 SO_PEERCRED 注入 `Extension(PeerUid)`；本测试以 `Extension(PeerUid(..))` 直接注入到
//! 请求模拟该透传（不构造来源类型——`PeerUid` 是裸 `u32` 包装，control/ 合规）。
//!
//! 雷区纪律：本文件**零 SQL 标记**；认证比对只读 `(uid)` 直比，不构造 `ConnOrigin` 字面变体；
//! handler unimplemented 经 `CatchPanicLayer` 收成 500（镜像生产 serve 的 panic 兜底），故「过门」
//! 断言只判「非 401/403」。异步用 `#[tokio::test]`。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use axum::http::StatusCode;
use axum::Router;
use tower::ServiceExt; // oneshot
use tower_http::catch_panic::CatchPanicLayer;

use postern_core::domain::ResourceCode;
use postern_core::error::AuditError;
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink};

use postern_daemon::control::auth::{ControlAuth, PeerUid, CONTROL_TOKEN_HEADER};
use postern_daemon::control::router::{router, with_control_auth};
use postern_daemon::control::{
    Actor, ControlState, Enrollment, PolicyRepo, WriteError, WriteIntent, WriteOutcome,
};
use postern_daemon::error::DaemonError;

// ════════════════════════════════════════════════════════════════════════════
//  固定材料 + 内存 Fake 句柄（只钉认证门行为所需）
// ════════════════════════════════════════════════════════════════════════════

/// daemon 自身 uid（认证门主门比对基准）。
const SELF_UID: u32 = 1000;
/// 一个不被采信的他者 uid（跨信任域，主门必拒）。
const OTHER_UID: u32 = 4242;
/// 正确的 control-token 字节（boot 从 0600 token 文件读入的期望值）。
const TOKEN: &[u8] = b"deadbeefcafef00d0011223344556677";

/// 内存 PolicyRepo 缝：list 回固定一项信封、policy_rev 回固定值、写恒成功（认证门测不触写路径）。
struct FakeRepo;

impl PolicyRepo for FakeRepo {
    fn commit_write(
        &self,
        _actor: &Actor,
        _intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
        Ok(WriteOutcome {
            version: 1,
            policy_rev: 1,
        })
    }

    fn list(
        &self,
        _entity: &'static str,
        page: PageQuery,
    ) -> Result<Page<serde_json::Value>, DaemonError> {
        Ok(Page {
            items: vec![serde_json::json!({ "id": "100", "name": "agent-a" })],
            page_no: page.page_no,
            page_size: page.page_size,
            total: 1,
        })
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        Ok(7)
    }
}

/// 内存 AuditSink 缝（认证门测不触审计支）。
struct FakeAudit;
impl AuditSink for FakeAudit {
    fn record(&self, _event: AuditEvent) -> Result<(), AuditError> {
        Ok(())
    }
}

/// 内存 Enrollment 缝（不构造任何机密类型）。
struct FakeEnrollment;
impl Enrollment for FakeEnrollment {
    fn enroll(&self, _resource: &ResourceCode, _tier: &str) -> Result<(), DaemonError> {
        Ok(())
    }
}

/// 装配 Fake 注入的 ControlState。
fn state() -> ControlState {
    ControlState::new(
        Arc::new(FakeRepo),
        Arc::new(FakeEnrollment),
        Arc::new(FakeAudit),
    )
}

/// 装配 in-process 控制面 router：router → front 认证门 → front CatchPanic（unimplemented
/// handler → 500，而非 crash 测试线程；镜像生产 `serve_control_router_over_uds` 的 panic 兜底）。
/// `token` 为认证门期望的 control-token（`None` 模拟无 token 文件 ⇒ 凭据恒 false）。
fn app(token: Option<Vec<u8>>) -> Router {
    let auth = ControlAuth::new(SELF_UID, token);
    with_control_auth(router(state()), auth).layer(CatchPanicLayer::new())
}

/// 构造一条控制面请求，注入模拟的对端 uid（生产由 serve 路径经 SO_PEERCRED 注入 Extension）。
/// `token` 为 `Some` 时附带 control-token 头。`peer_uid` 为 `Some` 时注入 PeerUid 扩展。
fn req(
    method: &str,
    uri: &str,
    peer_uid: Option<u32>,
    token: Option<&[u8]>,
) -> axum::http::Request<axum::body::Body> {
    let mut b = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        b = b.header(CONTROL_TOKEN_HEADER, t);
    }
    let mut request = b.body(axum::body::Body::empty()).expect("request builds");
    if let Some(uid) = peer_uid {
        // 模拟控制面 serve 经 SO_PEERCRED 注入的对端 uid 扩展（PeerUid 是裸 u32 包装，非来源类型）。
        request.extensions_mut().insert(PeerUid(uid));
    }
    request
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 1：peer 不符拒——peer_uid≠self_uid ⇒ 403（即便携带正确 token，主门先拒）
// ════════════════════════════════════════════════════════════════════════════

/// 对端 uid 与本进程 uid 不符 ⇒ 403 Forbidden（主门跨信任域拒），即便 token 正确。
#[tokio::test]
async fn peer_uid_mismatch_is_rejected_even_with_valid_token() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req("GET", "/v1/principals", Some(OTHER_UID), Some(TOKEN)))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "peer uid 不符 ⇒ 403（SO_PEERCRED 主门跨信任域拒，token 正确也不旁路主门，L-1）"
    );
}

/// 缺对端 uid 扩展（未经控制面 serve 路径采集来源）⇒ 403（无可信对端事实 ⇒ fail-closed）。
#[tokio::test]
async fn missing_peer_uid_extension_is_rejected() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req("GET", "/v1/principals", None, Some(TOKEN)))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "缺 PeerUid 扩展 ⇒ 无可信来源事实 ⇒ 403 fail-closed（绝不退化为采信自报 / 默认放行）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 2：缺 / 错 token 拒——peer 相符但凭据缺失 ⇒ 401（uid 对仍须出示凭据）
// ════════════════════════════════════════════════════════════════════════════

/// peer 相符但**缺** control-token ⇒ 401 Unauthorized（裸同 uid 无凭据绝不放行，L-1）。
#[tokio::test]
async fn matching_peer_without_token_is_rejected() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req("GET", "/v1/principals", Some(SELF_UID), None))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "peer 相符但缺 control-token ⇒ 401（第二因子缺失，fail-closed）"
    );
}

/// peer 相符但 token **错误** ⇒ 401（凭据不符等同缺失，fail-closed）。
#[tokio::test]
async fn matching_peer_with_wrong_token_is_rejected() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req(
            "GET",
            "/v1/principals",
            Some(SELF_UID),
            Some(b"wrong-token-zzzzzzzzzzzzzzzzzzzz"),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "peer 相符但 token 错 ⇒ 401（凭据不符等同缺失，fail-closed）"
    );
}

/// 装配端**无 token 文件**（ControlAuth.token=None）⇒ 即便出示任意 token 亦 401（缺期望凭据，
/// 凭据恒 false，fail-closed）。
#[tokio::test]
async fn no_token_file_rejects_even_with_presented_token() {
    let resp = app(None)
        .oneshot(req("GET", "/v1/principals", Some(SELF_UID), Some(TOKEN)))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "无 token 文件 ⇒ 期望凭据缺位 ⇒ 凭据恒 false ⇒ 401（缺凭据绝不放行，fail-closed）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 3：对 token 放行——peer 相符 + 正确 token ⇒ 过认证门（非 401/403，达下游 handler）
// ════════════════════════════════════════════════════════════════════════════

/// peer 相符 + 正确 token ⇒ 过认证门，请求达下游 handler（认证门不拒：非 401、非 403）。
#[tokio::test]
async fn matching_peer_with_valid_token_passes_auth_gate() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req("GET", "/v1/principals", Some(SELF_UID), Some(TOKEN)))
        .await
        .expect("router serves");
    // 过门即下游 handler 处理：handler 可能 unimplemented(→500) 或已实现(→2xx)；关键是**非认证拒绝**。
    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "正确 peer + token ⇒ 过 peer 主门（非 403）"
    );
    assert_ne!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "正确 peer + token ⇒ 过 token 第二因子（非 401）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 4：health 豁免 token 仍过 peer 门
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/health`：peer 相符 + **无 token** ⇒ 200（token 豁免，运维探活）。
#[tokio::test]
async fn health_is_token_exempt_when_peer_matches() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req("GET", "/v1/health", Some(SELF_UID), None))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "health 豁免 token：peer 相符 + 无 token ⇒ 200（D1 健康投影 handler 可达）"
    );
}

/// `GET /v1/health`：peer **不符** ⇒ 仍 403（health 豁免 token，但**绝不**豁免 peer 主门，L-1）。
#[tokio::test]
async fn health_still_enforces_peer_gate() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req("GET", "/v1/health", Some(OTHER_UID), None))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "health 豁免 token 但仍过 peer 门：peer 不符 ⇒ 403（peer 主门绝不豁免，L-1）"
    );
}

/// `GET /v1/health`：缺 peer uid 扩展 ⇒ 仍 403（health 不豁免 peer 主门，无来源事实 fail-closed）。
#[tokio::test]
async fn health_without_peer_extension_is_rejected() {
    let resp = app(Some(TOKEN.to_vec()))
        .oneshot(req("GET", "/v1/health", None, None))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "health 缺 PeerUid 扩展 ⇒ 无可信来源事实 ⇒ 403 fail-closed（peer 门绝不豁免）"
    );
}
