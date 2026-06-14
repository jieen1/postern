//! D2b roles-bindings 域 handler 行为测试（GREEN）——in-process router oneshot（§6.5 / F-6 / L-14）。
//!
//! 钉死 [`control::handlers::roles`](postern_daemon::control::handlers) /
//! [`bindings`](postern_daemon::control::handlers) 两域 handler 经 axum 提取器 →
//! [`endpoints`](postern_daemon::control::endpoints) → 响应装配：
//! - **读**（`GET /v1/roles` / `GET /v1/bindings`）：回 `200` + `Page` 信封（字段恒为 `items`，
//!   F-6；缺省分页 page_no=1/page_size=20 经 [`PageParams::to_query`] 缺省填充 + 钳制）。
//! - **写**（`POST /v1/roles` / `POST /v1/bindings`）：写成功 ⇒ `200` +
//!   [`WriteAck`](postern_daemon::control::dto::WriteAck)（`policy_rev` 为**字符串**且为前进后
//!   的修订号——u64 同雪花纪律字符串化出线）。
//! - **乐观锁冲突**：stale 版本 ⇒ `409 Conflict` + 错误信封（机读码 `version_conflict`）。
//! - **写失败**：三联动任一支失败（此处以审计写失败注入）⇒ `500` + 错误信封 `write_failed`
//!   （fail-closed，绝不伪报成功）。
//! - **id 一律字符串**：bindings 写 DTO 的 `principal_id`/`role_id` 为雪花字符串，经 WriteIntent
//!   字段透传不丢精度（写端点缝 Fake 回放收到的 intent.fields，断言两 id 仍为 JSON 字符串）。
//!
//! 驱动方式（06 §9 / 镜像 d2b_handlers）：以内存 Fake 全句柄注入装配 [`ControlState`]，经
//! [`router`](postern_daemon::control::router::router) 装配 in-process router，
//! `tower::ServiceExt::oneshot` 打请求、断言精确到 HTTP 状态码 / 响应形状。router front 一层
//! `CatchPanicLayer`（镜像生产 `serve_router_over_uds` 的 panic 兜底）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（写全经 Fake `PolicyRepo` 缝）；不构造来源类型 / 机密类型；
//! argon2 不在本路径（无 KDF）⇒ 可直接 `cargo test -p postern-daemon --test d2b-roles-bindings`，
//! 无需 systemd-run 内存包裹；`#[tokio::test]` 异步驱动。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::{Arc, Mutex};

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
//  内存 Fake 句柄（与 d2b_handlers 同形态：只钉本域 handler 行为所需）
// ════════════════════════════════════════════════════════════════════════════

/// 注入到 Fake repo 的写结果。
#[derive(Clone)]
enum WritePlan {
    /// 全成功：回新版本 / 修订号。
    Ok { version: i64, policy_rev: u64 },
    /// 乐观锁冲突。
    Conflict,
}

/// 内存 PolicyRepo 缝：按 WritePlan 报写成败；记录最近一次写意图（供 id 字符串透传断言）；
/// list 回固定一项信封（id 一律字符串投影）；policy_rev 回注入值。
struct FakeRepo {
    plan: WritePlan,
    rev: u64,
    last_intent: Mutex<Option<WriteIntent>>,
    last_list_entity: Mutex<Option<&'static str>>,
}

impl FakeRepo {
    fn new(plan: WritePlan, rev: u64) -> Arc<Self> {
        Arc::new(Self {
            plan,
            rev,
            last_intent: Mutex::new(None),
            last_list_entity: Mutex::new(None),
        })
    }
}

impl PolicyRepo for FakeRepo {
    fn commit_write(
        &self,
        _actor: &Actor,
        intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
        *self.last_intent.lock().expect("intent lock") = Some(intent.clone());
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
        entity: &'static str,
        page: PageQuery,
    ) -> Result<Page<serde_json::Value>, DaemonError> {
        *self.last_list_entity.lock().expect("list entity lock") = Some(entity);
        // 投影一项：id 一律字符串（雪花不丢精度）；按实体回各自读模型形状。
        let item = match entity {
            "roles" => serde_json::json!({
                "id": "200", "name": "reader", "description": null, "version": 0
            }),
            "bindings" => serde_json::json!({
                "id": "300", "principal_id": "100", "role_id": "200", "version": 0
            }),
            other => serde_json::json!({ "id": "1", "entity": other }),
        };
        Ok(Page {
            items: vec![item],
            page_no: page.page_no,
            page_size: page.page_size,
            total: 1,
        })
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        Ok(self.rev)
    }
}

/// 内存 AuditSink 缝：默认成功；可注入写失败（驱动写端点三联动审计支失败 → fail-closed 500）。
struct FakeAudit {
    fail: bool,
}
impl FakeAudit {
    fn ok() -> Arc<Self> {
        Arc::new(Self { fail: false })
    }
    fn failing() -> Arc<Self> {
        Arc::new(Self { fail: true })
    }
}
impl AuditSink for FakeAudit {
    fn record(&self, _event: AuditEvent) -> Result<(), AuditError> {
        if self.fail {
            Err(AuditError::WriteFailed)
        } else {
            Ok(())
        }
    }
}

/// 内存 Enrollment 缝（不构造任何机密类型）。
struct FakeEnrollment;
impl Enrollment for FakeEnrollment {
    fn enroll(&self, _resource: &ResourceCode, _tier: &str) -> Result<(), DaemonError> {
        Ok(())
    }
}

/// 装配 Fake 注入的 ControlState（repo + audit 可定制；enrollment 恒 Fake）。
fn state(repo: Arc<FakeRepo>, audit: Arc<FakeAudit>) -> ControlState {
    ControlState::new(repo, Arc::new(FakeEnrollment), audit)
}

/// 装配 in-process 控制面 router，front 一层 CatchPanic（镜像生产 panic 兜底）。
fn app(repo: Arc<FakeRepo>, audit: Arc<FakeAudit>) -> Router {
    router(state(repo, audit)).layer(CatchPanicLayer::new())
}

/// 默认成功 plan 的 router（写成功回 policy_rev=8，list rev=7）。
fn ok_app() -> (Arc<FakeRepo>, Router) {
    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 1,
            policy_rev: 8,
        },
        7,
    );
    let app = app(repo.clone(), FakeAudit::ok());
    (repo, app)
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

/// JSON body。
fn json_body(v: serde_json::Value) -> axum::body::Body {
    axum::body::Body::from(serde_json::to_vec(&v).expect("body serializes"))
}

/// 取响应体为 JSON。
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body reads");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

// ════════════════════════════════════════════════════════════════════════════
//  roles 域
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/roles`：回 200 + `Page` 信封（字段恒为 `items`，F-6），项 id 为字符串。
#[tokio::test]
async fn get_roles_returns_paged_items_envelope() {
    let (repo, app) = ok_app();
    let resp = app
        .oneshot(req(
            "GET",
            "/v1/roles?page_no=1&page_size=20",
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "roles 集合读回 200（Page 信封）"
    );
    assert_eq!(
        *repo.last_list_entity.lock().expect("entity lock"),
        Some("roles"),
        "list handler 须以固定实体 \"roles\" 调端点（非请求自报表名）"
    );
    let json = body_json(resp).await;
    assert!(
        json.get("items").map(|i| i.is_array()).unwrap_or(false),
        "分页信封字段恒为 items（非 list，F-6）"
    );
    let id = json["items"][0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("role 项 id 为字符串");
    assert_eq!(id, "200", "role id 投影为字符串（雪花不丢精度）");
}

/// `GET /v1/roles`：缺省分页（无 query）⇒ 经 [`PageParams::to_query`] 缺省填充 page_no=1 /
/// page_size=20（缺省 20，F-6），回 200。
#[tokio::test]
async fn get_roles_default_pagination_fills_defaults() {
    let (_repo, app) = ok_app();
    let resp = app
        .oneshot(req("GET", "/v1/roles", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(
        json.get("page_size").and_then(|v| v.as_u64()),
        Some(20),
        "缺省分页 page_size 填充为 20（F-6 缺省 20）"
    );
    assert_eq!(
        json.get("page_no").and_then(|v| v.as_u64()),
        Some(1),
        "缺省分页 page_no 填充为 1"
    );
}

/// `POST /v1/roles`：写成功 ⇒ 200 + WriteAck，`policy_rev` 为字符串且为前进后的修订号。
#[tokio::test]
async fn post_roles_returns_write_ack_with_string_policy_rev() {
    let (repo, app) = ok_app();
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/roles",
            json_body(serde_json::json!({ "name": "reader", "description": "read only" })),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "roles 写成功回 200（三联动 COMMIT + 重建 + 审计）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 为字符串、且为前进后的修订号"
    );
    // DTO → WriteIntent 映射：entity 固定 roles，业务字段 name/description 落 fields，新增无版本。
    let intent = repo
        .last_intent
        .lock()
        .expect("intent lock")
        .clone()
        .expect("intent recorded");
    assert_eq!(intent.entity, "roles", "写意图实体固定为 roles");
    assert_eq!(
        intent.fields.get("name").and_then(|v| v.as_str()),
        Some("reader"),
        "DTO name 映射进 WriteIntent.fields"
    );
    assert_eq!(
        intent.fields.get("description").and_then(|v| v.as_str()),
        Some("read only"),
        "DTO description 映射进 WriteIntent.fields"
    );
    assert_eq!(
        intent.expected_version, None,
        "新增角色无乐观锁前驱版本（expected_version=None）"
    );
}

/// `POST /v1/roles` 乐观锁冲突 ⇒ 409 Conflict + 错误信封 `version_conflict`。
#[tokio::test]
async fn post_roles_stale_version_returns_409() {
    let repo = FakeRepo::new(WritePlan::Conflict, 7);
    let app = app(repo, FakeAudit::ok());
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/roles",
            json_body(serde_json::json!({ "name": "reader", "description": null })),
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
        "409 错误信封机读码为 version_conflict"
    );
}

/// `POST /v1/roles` 审计写失败（三联动一支）⇒ 500 + 错误信封 `write_failed`（fail-closed，
/// 绝不伪报成功）。
#[tokio::test]
async fn post_roles_audit_failure_returns_500_write_failed() {
    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 1,
            policy_rev: 8,
        },
        7,
    );
    let app = app(repo, FakeAudit::failing());
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/roles",
            json_body(serde_json::json!({ "name": "reader", "description": null })),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "三联动审计支失败 ⇒ 500（fail-closed，无半态）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("write_failed"),
        "500 错误信封机读码为 write_failed"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  bindings 域
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/bindings`：回 200 + `Page` 信封（`items`），principal_id/role_id 投影为字符串。
#[tokio::test]
async fn get_bindings_returns_paged_items_envelope() {
    let (repo, app) = ok_app();
    let resp = app
        .oneshot(req("GET", "/v1/bindings", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        *repo.last_list_entity.lock().expect("entity lock"),
        Some("bindings"),
        "list handler 须以固定实体 \"bindings\" 调端点"
    );
    let json = body_json(resp).await;
    let item = &json["items"][0];
    assert_eq!(
        item.get("principal_id").and_then(|v| v.as_str()),
        Some("100"),
        "binding principal_id 投影为字符串（雪花不丢精度）"
    );
    assert_eq!(
        item.get("role_id").and_then(|v| v.as_str()),
        Some("200"),
        "binding role_id 投影为字符串（雪花不丢精度）"
    );
}

/// `POST /v1/bindings`：写成功 ⇒ 200 + WriteAck（policy_rev 字符串），且 DTO 的雪花字符串
/// principal_id/role_id 经 WriteIntent.fields **仍为 JSON 字符串**透传（id 一律 string，不丢精度）。
#[tokio::test]
async fn post_bindings_returns_write_ack_and_keeps_ids_as_strings() {
    let (repo, app) = ok_app();
    // 用 > 2^53 的雪花值，验证不被当成 JS number 丢精度。
    let big_principal = "9007199254740993"; // 2^53 + 1
    let big_role = "9007199254740995";
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/bindings",
            json_body(serde_json::json!({
                "principal_id": big_principal,
                "role_id": big_role,
            })),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "bindings 写成功回 200"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 为字符串"
    );
    let intent = repo
        .last_intent
        .lock()
        .expect("intent lock")
        .clone()
        .expect("intent recorded");
    assert_eq!(intent.entity, "bindings", "写意图实体固定为 bindings");
    assert_eq!(
        intent.fields.get("principal_id").and_then(|v| v.as_str()),
        Some(big_principal),
        "principal_id 经 WriteIntent.fields 仍为 JSON 字符串（不丢精度、未被解析为 number）"
    );
    assert_eq!(
        intent.fields.get("role_id").and_then(|v| v.as_str()),
        Some(big_role),
        "role_id 经 WriteIntent.fields 仍为 JSON 字符串（不丢精度）"
    );
    assert_eq!(intent.expected_version, None, "新增绑定无乐观锁前驱版本");
}

/// `POST /v1/bindings` 乐观锁冲突 ⇒ 409 Conflict + 错误信封 `version_conflict`。
#[tokio::test]
async fn post_bindings_stale_version_returns_409() {
    let repo = FakeRepo::new(WritePlan::Conflict, 7);
    let app = app(repo, FakeAudit::ok());
    let resp = app
        .oneshot(req(
            "POST",
            "/v1/bindings",
            json_body(serde_json::json!({ "principal_id": "100", "role_id": "200" })),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::CONFLICT,
        "乐观锁版本冲突 ⇒ 409 Conflict"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("version_conflict"),
        "409 错误信封机读码为 version_conflict"
    );
}
