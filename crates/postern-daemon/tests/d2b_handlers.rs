//! D2b 控制面 handler 行为测试（RED）——按域驱动 in-process router oneshot（§6.5 / F-6 / L-14）。
//!
//! 钉死 [`control::handlers`](postern_daemon::control::handlers) 各域 handler 经 axum 提取器 →
//! [`endpoints`](postern_daemon::control::endpoints) → 响应装配：
//! - **读**（`GET /v1/principals` 等）：回 `200` + `Page` 信封（字段 `items`，F-6）。
//! - **写**（`POST /v1/principals` 等）：回 `200` + [`WriteAck`](postern_daemon::control::dto::WriteAck)
//!   （`policy_rev` 字符串，rev 前进）。
//! - **乐观锁冲突**：stale 版本 ⇒ `409 Conflict` + 错误信封。
//! - **凭据写**：`POST /v1/credentials` 明确回「D2c 未启用」（`501` + `credentials_not_enabled`）——
//!   绝不伪造写凭据成功（**当前即绿**：该 handler 已落地）。
//!
//! 驱动方式（06 §9）：以**内存 Fake 全句柄注入**（Fake `PolicyRepo`/`Enrollment`/`AuditSink`）装配
//! [`ControlState`]，经 [`router`](postern_daemon::control::router::router) 装配 in-process router，
//! `tower::ServiceExt::oneshot` 打请求、断言精确到 HTTP 状态码 / 响应形状。读写 handler 体本波次为
//! `unimplemented!()` 骨架——为使 unimplemented 表现为 `500`（而非 crash 测试线程），router 前 front
//! 一层 `CatchPanicLayer`（镜像生产 `serve_router_over_uds` 的 panic 兜底）。故读 200 / 写 200 /
//! stale 409 三类断言**当前红**（实测 500），GreenAuth 域内填 handler 体后转绿。
//!
//! 雷区纪律：本文件**零 SQL 标记**（写全经 Fake `PolicyRepo` 缝）；认证比对若需仅用 `(uid)` 直比，
//! 本测试经 router oneshot 不构造来源类型；`#[tokio::test]` 异步驱动。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use axum::Router;
use tower::ServiceExt; // oneshot
use tower_http::catch_panic::CatchPanicLayer;

use postern_core::domain::ResourceCode;
use postern_core::error::AuditError;
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink};

use postern_daemon::control::router::router;
use postern_daemon::control::{
    Actor, ControlState, Enrollment, PolicyRepo, WriteError, WriteIntent, WriteOutcome,
};
use postern_daemon::error::DaemonError;

// ════════════════════════════════════════════════════════════════════════════
//  内存 Fake 句柄（与 control.rs 同形态：只钉本波次 handler 行为所需）
// ════════════════════════════════════════════════════════════════════════════

/// 注入到 Fake repo 的写结果。
#[derive(Clone)]
enum WritePlan {
    /// 全成功：回新版本 / 修订号。
    Ok { version: i64, policy_rev: u64 },
    /// 乐观锁冲突。
    Conflict,
}

/// 内存 PolicyRepo 缝：按 WritePlan 报写成败；list 回固定一项信封；policy_rev 回注入值。
struct FakeRepo {
    plan: WritePlan,
    rev: u64,
    last_actor_system: AtomicI64,
}

impl FakeRepo {
    fn new(plan: WritePlan, rev: u64) -> Arc<Self> {
        Arc::new(Self {
            plan,
            rev,
            last_actor_system: AtomicI64::new(-1),
        })
    }
}

impl PolicyRepo for FakeRepo {
    fn commit_write(
        &self,
        actor: &Actor,
        _intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
        self.last_actor_system
            .store(i64::from(matches!(actor, Actor::System)), Ordering::SeqCst);
        match self.plan {
            WritePlan::Ok {
                version,
                policy_rev,
            } => Ok(WriteOutcome {
                version,
                policy_rev,
            }),
            WritePlan::Conflict => Err(WriteError::VersionConflict),
        }
    }

    fn list(
        &self,
        _entity: &'static str,
        page: PageQuery,
    ) -> Result<Page<serde_json::Value>, DaemonError> {
        Ok(Page {
            items: vec![
                serde_json::json!({ "id": "100", "name": "agent-a", "kind": "agent", "version": 0 }),
            ],
            page_no: page.page_no,
            page_size: page.page_size,
            total: 1,
        })
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        Ok(self.rev)
    }
}

/// 内存 AuditSink 缝：恒成功记录（本波次只需三联动审计支不阻断）。
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
fn state(plan: WritePlan, rev: u64) -> ControlState {
    ControlState::new(
        FakeRepo::new(plan, rev),
        Arc::new(FakeEnrollment),
        Arc::new(FakeAudit),
    )
}

/// 装配 in-process 控制面 router，front 一层 CatchPanic（unimplemented handler → 500，
/// 而非 crash 测试线程；镜像生产 `serve_router_over_uds` 的 panic 兜底）。
fn app(plan: WritePlan, rev: u64) -> Router {
    router(state(plan, rev)).layer(CatchPanicLayer::new())
}

/// 默认成功 plan 的 router（rev=7）。
fn ok_app() -> Router {
    app(
        WritePlan::Ok {
            version: 1,
            policy_rev: 8,
        },
        7,
    )
}

/// 构造一条请求。
fn req(method: &str, uri: &str, body: axum::body::Body) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body)
        .expect("request builds")
}

/// 取响应体为 JSON。
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body reads");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

// ════════════════════════════════════════════════════════════════════════════
//  凭据写：明确「D2c 未启用」——当前即绿（handler 已落地，不伪造成功）
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/credentials`：D2c 未启用 ⇒ 回 501 + 错误信封 `credentials_not_enabled`
/// （绝不伪造写凭据成功）。
#[tokio::test]
async fn post_credentials_reports_d2c_not_enabled() {
    let resp = ok_app()
        .oneshot(req("POST", "/v1/credentials", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "凭据写 vault = D2c：D2b 明确回 501（不伪造成功）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("credentials_not_enabled"),
        "错误信封 {{error:{{code,message}}}}，机读码标明 D2c 未启用"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  集合读 handler：200 + Page 信封（items）——当前红（handler unimplemented → 500）
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/principals`：回 200 + `Page` 信封（字段 `items`，F-6）。当前红（unimplemented→500）。
#[tokio::test]
async fn get_principals_returns_paged_items_envelope() {
    let resp = ok_app()
        .oneshot(req(
            "GET",
            "/v1/principals?page_no=1&page_size=20",
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "集合读回 200（Page 信封）"
    );
    let json = body_json(resp).await;
    assert!(
        json.get("items").map(|i| i.is_array()).unwrap_or(false),
        "分页信封字段恒为 items（非 list，F-6）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  写 handler：200 + WriteAck（policy_rev 字符串）——当前红（handler unimplemented → 500）
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/principals`：写成功 ⇒ 200 + WriteAck，`policy_rev` 为字符串且为前进后的修订号。
/// 当前红（unimplemented→500）。
#[tokio::test]
async fn post_principals_returns_write_ack_with_string_policy_rev() {
    let body = axum::body::Body::from(
        serde_json::to_vec(&serde_json::json!({ "name": "agent-a", "kind": "agent" })).unwrap(),
    );
    let resp = ok_app()
        .oneshot(req("POST", "/v1/principals", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "写成功回 200（三联动 COMMIT + 重建 + 审计）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 为字符串、且为前进后的修订号（rev 前进）"
    );
}

/// `POST /v1/principals` 乐观锁冲突 ⇒ 409 Conflict + 错误信封。当前红（unimplemented→500）。
#[tokio::test]
async fn post_principals_stale_version_returns_409() {
    let body = axum::body::Body::from(
        serde_json::to_vec(&serde_json::json!({ "name": "agent-a", "kind": "agent" })).unwrap(),
    );
    let resp = app(WritePlan::Conflict, 7)
        .oneshot(req("POST", "/v1/principals", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::CONFLICT,
        "乐观锁版本冲突 ⇒ 409 Conflict（F-6 / L-15）"
    );
}
