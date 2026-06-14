//! D2b-ext 控制面 handler 接真行为测试（RED→GREEN）——细则 / 条件 / 拒绝备注 / 凭据列读。
//!
//! 钉死此前硬编 501 的 misc 域 handler（[`control::handlers::misc`](postern_daemon::control::handlers::misc)）
//! 在适配器接通后经 axum 提取器 → [`endpoints`](postern_daemon::control::endpoints) → 响应装配：
//! - **集合读**（`GET /v1/constraints`、`GET /v1/conditions`、`GET /v1/deny-notes`）：200 +
//!   `Page` 信封（字段恒 `items`，F-6）——**不再** 501。
//! - **写**（`POST /v1/constraints`、`POST /v1/conditions`、`POST /v1/deny-notes`）：`expected_version`
//!   缺 ⇒ 新增、带 ⇒ 乐观锁逻辑删除（与适配器 `commit_*` 分流对称），200 + [`WriteAck`]
//!   （`policy_rev` 字符串、rev 前进）——**不再** 501；写经正确 `entity`。
//! - **乐观锁冲突**：stale 版本 ⇒ 409 Conflict（F-6 / L-15）。
//! - **凭据列读**（`GET /v1/credentials`）：读模型未接通（适配器无 `credentials` 臂）⇒ 仍 501 +
//!   稳定码（能力未接通，绝非经适配器折成不可区分的 500）。
//!
//! 驱动方式（06 §9）：以内存 Fake 全句柄注入（Fake `PolicyRepo`/`Enrollment`/`AuditSink`）装配
//! [`ControlState`]，经 [`router`](postern_daemon::control::router::router) 装配 in-process router，
//! `tower::ServiceExt::oneshot` 打请求、断言精确到 HTTP 状态码 / 响应形状 / 写经哪个 entity。router
//! 前 front 一层 `CatchPanicLayer`（镜像生产 panic 兜底：未实现 handler → 500 而非 crash 线程）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（读写全经 Fake `PolicyRepo` 缝）；不构造 `ConnOrigin` / 机密
//! 类型（经 router oneshot，来源类型不在测试侧构造）；`#[tokio::test]` 异步驱动。argon2 不在本
//! 路径（控制面 router 无 KDF），故可直接 `cargo test -p postern-daemon --test d2bx_handlers2`。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;
use std::sync::Mutex;

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
//  内存 Fake 句柄（与 d2b_handlers.rs / d2b_audit_misc.rs 同形态）
// ════════════════════════════════════════════════════════════════════════════

/// 注入到 Fake repo 的写结果。
#[derive(Clone)]
enum WritePlan {
    /// 全成功：回新版本 / 修订号。
    Ok { version: i64, policy_rev: u64 },
    /// 乐观锁冲突。
    Conflict,
}

/// 一次写被记录的关键路由信息（实体 + 期望版本 + 业务字段），供「写经哪个 entity / 哪个分支」断言。
#[derive(Clone)]
struct LastWrite {
    entity: &'static str,
    expected_version: Option<i64>,
    fields: serde_json::Value,
}

/// 内存 PolicyRepo 缝：按 WritePlan 报写成败；list 回固定一项信封（任何实体同形）；
/// policy_rev 回注入值。记录最后一次写的实体 / 期望版本 / 字段，供路由断言。
struct FakeRepo {
    plan: WritePlan,
    rev: u64,
    last: Mutex<Option<LastWrite>>,
}

impl FakeRepo {
    fn new(plan: WritePlan, rev: u64) -> Arc<Self> {
        Arc::new(Self {
            plan,
            rev,
            last: Mutex::new(None),
        })
    }
}

impl PolicyRepo for FakeRepo {
    fn commit_write(
        &self,
        _actor: &Actor,
        intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
        *self.last.lock().unwrap() = Some(LastWrite {
            entity: intent.entity,
            expected_version: intent.expected_version,
            fields: intent.fields.clone(),
        });
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
        // 固定一项读模型（id 一律 string，雪花不丢精度）——细则 / 条件 / 拒绝备注同形态驱动。
        Ok(Page {
            items: vec![serde_json::json!({ "id": "100", "resource": "200", "version": 0 })],
            page_no: page.page_no,
            page_size: page.page_size,
            total: 1,
        })
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        Ok(self.rev)
    }
}

/// 内存 AuditSink 缝：恒成功记录（写端点三联动审计支不阻断）。
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

/// 装配 Fake 注入的 ControlState（返回 repo 句柄供路由断言）。
fn state(plan: WritePlan, rev: u64) -> (ControlState, Arc<FakeRepo>) {
    let repo = FakeRepo::new(plan, rev);
    let st = ControlState::new(
        Arc::clone(&repo) as Arc<dyn PolicyRepo>,
        Arc::new(FakeEnrollment),
        Arc::new(FakeAudit),
    );
    (st, repo)
}

/// 装配 in-process 控制面 router，front 一层 CatchPanic（未实现 handler → 500，非 crash 线程）。
fn app(plan: WritePlan, rev: u64) -> (Router, Arc<FakeRepo>) {
    let (st, repo) = state(plan, rev);
    (router(st).layer(CatchPanicLayer::new()), repo)
}

/// 默认成功 plan 的 router（list rev=7；写成功 ⇒ version=1, policy_rev=8）。
fn ok_app() -> (Router, Arc<FakeRepo>) {
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

/// 构造一条带 JSON 体的 POST 请求。
fn json_req(uri: &str, value: serde_json::Value) -> axum::http::Request<axum::body::Body> {
    req(
        "POST",
        uri,
        axum::body::Body::from(serde_json::to_vec(&value).expect("json serializes")),
    )
}

/// 取响应体为 JSON。
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body reads");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

/// 断言一个集合读 handler 回 200 + `Page` 信封（`items` 数组，F-6），并钳分页（300 ⇒ 200）。
async fn assert_list_200_paged(uri_base: &str) {
    let (app, _repo) = ok_app();
    let resp = app
        .oneshot(req(
            "GET",
            &format!("{uri_base}?page_no=1&page_size=300"),
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "{uri_base} 列读接真 ⇒ 200（Page 信封，不再 501）"
    );
    let json = body_json(resp).await;
    assert!(
        json.get("items").map(|i| i.is_array()).unwrap_or(false),
        "{uri_base} 分页信封字段恒为 items（非 list，F-6）"
    );
    assert_eq!(
        json.get("page_size").and_then(|v| v.as_u64()),
        Some(200),
        "{uri_base} page_size=300 ⇒ 钳到全局上限 200（F-6 强制分页钳制）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  集合读接真：constraints / conditions / deny-notes ⇒ 200 + Page 信封（不再 501）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn get_constraints_returns_paged_items_envelope() {
    assert_list_200_paged("/v1/constraints").await;
}

#[tokio::test]
async fn get_conditions_returns_paged_items_envelope() {
    assert_list_200_paged("/v1/conditions").await;
}

#[tokio::test]
async fn get_deny_notes_returns_paged_items_envelope() {
    assert_list_200_paged("/v1/deny-notes").await;
}

// ════════════════════════════════════════════════════════════════════════════
//  写接真（create，expected_version 缺）：200 + WriteAck（rev 前进）+ 经正确 entity
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn post_constraint_create_returns_write_ack_and_routes_entity() {
    let (app, repo) = ok_app();
    let resp = app
        .oneshot(json_req(
            "/v1/constraints",
            serde_json::json!({
                "resource_id": "200",
                "capability": "mutate",
                "kind": "row_limit",
                "spec": "{\"max\":10}"
            }),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "细则新增接真 ⇒ 200（三联动，不再 501）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 为字符串、且为前进后的修订号"
    );
    let last = repo.last.lock().unwrap().clone().expect("写已发生");
    assert_eq!(last.entity, "constraints", "写经 entity=constraints");
    assert_eq!(
        last.expected_version, None,
        "create（无 expected_version）⇒ 适配器走新增分支"
    );
    assert_eq!(
        last.fields.get("resource_id").and_then(|v| v.as_str()),
        Some("200"),
        "resource_id 透传进 fields（雪花字符串）"
    );
}

#[tokio::test]
async fn post_condition_create_routes_nullable_fields() {
    let (app, repo) = ok_app();
    // 全局通用条件：resource_id / capability 皆缺。
    let resp = app
        .oneshot(json_req(
            "/v1/conditions",
            serde_json::json!({ "predicate": "business_hours" }),
        ))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK, "条件新增 ⇒ 200");
    let last = repo.last.lock().unwrap().clone().expect("写已发生");
    assert_eq!(last.entity, "conditions", "写经 entity=conditions");
    assert_eq!(last.expected_version, None, "create ⇒ 新增分支");
    assert!(
        last.fields
            .get("resource_id")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "缺 resource_id ⇒ fields 投影为 null（全局通用条件，对齐 types.ts string|null）"
    );
    assert_eq!(
        last.fields.get("predicate").and_then(|v| v.as_str()),
        Some("business_hours"),
        "predicate 透传进 fields"
    );
}

#[tokio::test]
async fn post_deny_note_create_routes_entity() {
    let (app, repo) = ok_app();
    let resp = app
        .oneshot(json_req(
            "/v1/deny-notes",
            serde_json::json!({
                "resource_id": "200",
                "capability": "drop",
                "note": "destructive op — denied by operator"
            }),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "拒绝备注新增 ⇒ 200"
    );
    let last = repo.last.lock().unwrap().clone().expect("写已发生");
    assert_eq!(last.entity, "deny_notes", "写经 entity=deny_notes");
    assert_eq!(last.expected_version, None, "create ⇒ 新增分支");
    assert_eq!(
        last.fields.get("note").and_then(|v| v.as_str()),
        Some("destructive op — denied by operator"),
        "note 透传进 fields（人亲笔说明）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  写接真（delete，带 expected_version）：透传 id + 期望版本走逻辑删除分支
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn post_constraint_delete_carries_expected_version() {
    let (app, repo) = ok_app();
    let resp = app
        .oneshot(json_req(
            "/v1/constraints",
            serde_json::json!({ "id": "100", "expected_version": 0 }),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "细则逻辑删除（带期望版本）⇒ 200 WriteAck"
    );
    let last = repo.last.lock().unwrap().clone().expect("写已发生");
    assert_eq!(last.entity, "constraints", "写经 entity=constraints");
    assert_eq!(
        last.expected_version,
        Some(0),
        "带 expected_version ⇒ 适配器走乐观锁逻辑删除分支"
    );
    assert_eq!(
        last.fields.get("id").and_then(|v| v.as_str()),
        Some("100"),
        "id 透传进 fields（雪花字符串）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  乐观锁冲突：stale 版本 ⇒ 409 Conflict
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn post_constraint_stale_version_returns_409() {
    let (app, _repo) = app(WritePlan::Conflict, 7);
    let resp = app
        .oneshot(json_req(
            "/v1/constraints",
            serde_json::json!({ "id": "100", "expected_version": 999 }),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::CONFLICT,
        "乐观锁版本冲突 ⇒ 409 Conflict（F-6 / L-15）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("version_conflict"),
        "409 带 version_conflict 错误信封"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  凭据列读：读模型未接通 ⇒ 仍 501（绝非经适配器折成不可区分的 500）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn get_credentials_reports_not_enabled_not_500() {
    let (app, _repo) = ok_app();
    let resp = app
        .oneshot(req("GET", "/v1/credentials", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "凭据读模型未接通（适配器无 credentials 臂）⇒ 501（绝非 500 内部失败）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some(postern_daemon::error::NOT_IMPLEMENTED_CODE),
        "501 带稳定机读码 not_implemented（能力未接通，运维据此区分于 500）"
    );
}
