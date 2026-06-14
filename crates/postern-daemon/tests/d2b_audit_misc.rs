//! D2b 控制面 audit-misc 域 handler 行为测试（GREEN）——按域驱动 in-process router oneshot
//! （§6.5 / F-6 / L-12 / L-14 / L-15）。
//!
//! 钉死 audit-misc 域各 handler（[`control::handlers::misc`](postern_daemon::control::handlers::misc)）
//! 经 axum 提取器 → [`endpoints`](postern_daemon::control::endpoints) → 响应装配：
//! - **审计 / 拒绝摘要列读**（`GET /v1/audit`、`GET /v1/denials/summary`）：200 + `Page` 信封
//!   （字段恒 `items`，F-6）。
//! - **设置读**（`GET /v1/settings`）：200 + `SettingRow[]` 裸数组（doc-specified，非 Page 信封）。
//! - **设置写**（`POST /v1/settings`）：`on_timeout=deny` ⇒ 200 + [`WriteAck`]（`policy_rev`
//!   字符串、rev 前进）；`on_timeout=allow` ⇒ **写入时刻即拒**（fail-closed，L-12，绝不持久化
//!   在线放行）；乐观锁冲突 ⇒ 409。
//! - **审批**（`POST /v1/approvals`）：审批关闭 ⇒ `op:list` 回空 `Page`；`op:adjudicate` 恒拒
//!   （绝不伪造裁决通过，L-12）。
//! - **导出 / 导入**（`POST /v1/export`、`POST /v1/import`）：export 回稳定 `{ toml }` 形状；
//!   import `on_timeout=allow` ⇒ 校验阶段即拒（L-12），`deny`/缺省 ⇒ 应用结果计数。
//! - **健康**（`GET /v1/health`）：D1 已接真，确认 200 + `status:"ok"` + `policy_rev`。
//!
//! 驱动方式（06 §9）：以内存 Fake 全句柄注入（Fake `PolicyRepo`/`Enrollment`/`AuditSink`）装配
//! [`ControlState`]，经 [`router`](postern_daemon::control::router::router) 装配 in-process
//! router，`tower::ServiceExt::oneshot` 打请求、断言精确到 HTTP 状态码 / 响应形状。router 前
//! front 一层 `CatchPanicLayer`（镜像生产 `serve_router_over_uds` 的 panic 兜底：未实现 handler →
//! 500 而非 crash 测试线程）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（读写全经 Fake `PolicyRepo` 缝）；不构造 `ConnOrigin` / 机密
//! 类型（经 router oneshot，来源类型不在测试侧构造）；`#[tokio::test]` 异步驱动。argon2 不在本
//! 路径（控制面 router 无 KDF），故可直接 `cargo test -p postern-daemon --test d2b_audit_misc`。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

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
//  内存 Fake 句柄（与 d2b_handlers.rs 同形态：只钉本域 handler 行为所需）
// ════════════════════════════════════════════════════════════════════════════

/// 注入到 Fake repo 的写结果。
#[derive(Clone)]
enum WritePlan {
    /// 全成功：回新版本 / 修订号。
    Ok { version: i64, policy_rev: u64 },
    /// 乐观锁冲突。
    Conflict,
}

/// 内存 PolicyRepo 缝：按 WritePlan 报写成败；list 回固定一项信封（任何实体同形）；
/// policy_rev 回注入值。记录最后一次写的实体，供「写经哪个 entity」断言。
struct FakeRepo {
    plan: WritePlan,
    rev: u64,
    last_entity: std::sync::Mutex<Option<&'static str>>,
}

impl FakeRepo {
    fn new(plan: WritePlan, rev: u64) -> Arc<Self> {
        Arc::new(Self {
            plan,
            rev,
            last_entity: std::sync::Mutex::new(None),
        })
    }
}

impl PolicyRepo for FakeRepo {
    fn commit_write(
        &self,
        _actor: &Actor,
        intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
        *self.last_entity.lock().unwrap() = Some(intent.entity);
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
        // 固定一项读模型（id 一律 string，雪花不丢精度）——审计 / 拒绝摘要 / 设置同形态驱动。
        Ok(Page {
            items: vec![serde_json::json!({ "id": "100", "key": "approvals", "value": "off" })],
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

/// 装配 Fake 注入的 ControlState。
fn state(plan: WritePlan, rev: u64) -> ControlState {
    ControlState::new(
        FakeRepo::new(plan, rev),
        Arc::new(FakeEnrollment),
        Arc::new(FakeAudit),
    )
}

/// 装配 in-process 控制面 router，front 一层 CatchPanic（未实现 handler → 500，非 crash 线程）。
fn app(plan: WritePlan, rev: u64) -> Router {
    router(state(plan, rev)).layer(CatchPanicLayer::new())
}

/// 默认成功 plan 的 router（list rev=7；写成功 ⇒ version=1, policy_rev=8）。
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

// ════════════════════════════════════════════════════════════════════════════
//  审计 / 拒绝摘要列读：200 + Page 信封（items，F-6）
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/audit`：回 200 + `Page` 信封（字段恒 `items`，F-6）。
#[tokio::test]
async fn get_audit_returns_paged_items_envelope() {
    let resp = ok_app()
        .oneshot(req(
            "GET",
            "/v1/audit?page_no=1&page_size=20",
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "审计列读回 200（Page 信封）"
    );
    let json = body_json(resp).await;
    assert!(
        json.get("items").map(|i| i.is_array()).unwrap_or(false),
        "分页信封字段恒为 items（非 list，F-6）"
    );
}

/// `GET /v1/denials/summary`：回 200 + `Page` 信封（聚合行不泄真实地址 / 存在性，公理四）。
#[tokio::test]
async fn get_denials_summary_returns_paged_items_envelope() {
    let resp = ok_app()
        .oneshot(req(
            "GET",
            "/v1/denials/summary?window=7d&page_no=1&page_size=20",
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK, "拒绝摘要回 200");
    let json = body_json(resp).await;
    assert!(
        json.get("items").map(|i| i.is_array()).unwrap_or(false),
        "拒绝摘要恒 Page 信封（items，F-6）"
    );
}

/// 集合读分页钳制：`page_size=300` ⇒ 钳到 200（F-6，钳制委托 core 唯一钳制点）。
#[tokio::test]
async fn get_audit_clamps_page_size_to_global_ceiling() {
    let resp = ok_app()
        .oneshot(req(
            "GET",
            "/v1/audit?page_no=1&page_size=300",
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(
        json.get("page_size").and_then(|v| v.as_u64()),
        Some(200),
        "page_size=300 ⇒ 钳到全局上限 200（绝不无界，DB_PAGINATION_MANDATORY / F-6）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  设置读：200 + SettingRow[] 裸数组（doc-specified，非 Page 信封）
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/settings`：回 200 + `SettingRow[]` 裸数组（types.ts `getSettings` 形状）。
#[tokio::test]
async fn get_settings_returns_bare_row_array() {
    let resp = ok_app()
        .oneshot(req("GET", "/v1/settings", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK, "设置读回 200");
    let json = body_json(resp).await;
    assert!(
        json.is_array(),
        "设置读回裸 SettingRow[]（doc-specified 固定小集，非 Page 信封）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  设置写：on_timeout 守门（L-12）+ 标准写三联动（WriteAck）+ 乐观锁 409
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/settings` `on_timeout=deny` ⇒ 200 + WriteAck（`policy_rev` 字符串、rev 前进）。
#[tokio::test]
async fn post_settings_on_timeout_deny_returns_write_ack() {
    let resp = ok_app()
        .oneshot(json_req(
            "/v1/settings",
            serde_json::json!({
                "key": "escalation.on_timeout",
                "value": "deny",
                "on_timeout": "deny",
                "expected_version": 0
            }),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "on_timeout=deny ⇒ 标准写三联动 200"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 为字符串、且为前进后的修订号（rev 前进）"
    );
}

/// `POST /v1/settings` `on_timeout=allow` ⇒ 写入时刻即拒（fail-closed，L-12，绝不持久化在线放行）。
#[tokio::test]
async fn post_settings_on_timeout_allow_is_rejected_at_write_time() {
    let resp = ok_app()
        .oneshot(json_req(
            "/v1/settings",
            serde_json::json!({
                "key": "escalation.on_timeout",
                "value": "allow",
                "on_timeout": "allow",
                "expected_version": 0
            }),
        ))
        .await
        .expect("router serves");
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::OK,
        "on_timeout=allow 绝不回 200（绝不静默持久化成在线放行，L-12）"
    );
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "on_timeout=allow ⇒ 写入时刻即拒（fail-closed 400）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("on_timeout_allow_rejected"),
        "错误信封机读码标明 on_timeout=allow 被拒"
    );
}

/// `POST /v1/settings` 乐观锁冲突 ⇒ 409 + 错误信封（F-6 / L-15）。
#[tokio::test]
async fn post_settings_stale_version_returns_409() {
    let resp = app(WritePlan::Conflict, 7)
        .oneshot(json_req(
            "/v1/settings",
            serde_json::json!({
                "key": "escalation.on_timeout",
                "value": "deny",
                "on_timeout": "deny",
                "expected_version": 99
            }),
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
        "冲突错误信封机读码 version_conflict"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  审批：审批关闭 ⇒ list 空 Page / adjudicate 恒拒（绝不伪造裁决，L-12）
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/approvals` `op:list`：审批关闭 ⇒ 200 + **空** `Page` 信封（待审队列恒空）。
#[tokio::test]
async fn post_approvals_list_returns_empty_page_when_disabled() {
    let resp = ok_app()
        .oneshot(json_req(
            "/v1/approvals",
            serde_json::json!({ "op": "list", "page_no": 1, "page_size": 20 }),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "审批 list 回 200"
    );
    let json = body_json(resp).await;
    let items = json
        .get("items")
        .and_then(|i| i.as_array())
        .expect("审批 list 恒 Page 信封（items）");
    assert!(
        items.is_empty(),
        "审批关闭 ⇒ 待审队列恒空（escalate 不入队，L-12）"
    );
}

/// `POST /v1/approvals` `op:adjudicate`：审批关闭 ⇒ 恒拒（绝不伪造裁决通过，L-12）。
#[tokio::test]
async fn post_approvals_adjudicate_is_denied_when_disabled() {
    let resp = ok_app()
        .oneshot(json_req(
            "/v1/approvals",
            serde_json::json!({ "op": "adjudicate", "id": "100", "decision": "allow" }),
        ))
        .await
        .expect("router serves");
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::OK,
        "审批关闭 ⇒ 裁决绝不回 200（绝不伪造通过，L-12）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("approvals_disabled"),
        "裁决错误信封机读码标明审批已禁用"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  导出 / 导入：export 稳定形状 / import on_timeout=allow 校验拒（L-12）
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/export`：回 200 + 稳定 `{ toml }` 形状（导出包不含凭据材料，公理四）。
#[tokio::test]
async fn post_export_returns_toml_envelope() {
    let resp = ok_app()
        .oneshot(json_req("/v1/export", serde_json::json!({})))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK, "导出回 200");
    let json = body_json(resp).await;
    assert!(
        json.get("toml").map(|t| t.is_string()).unwrap_or(false),
        "导出回 {{ toml: string }} 稳定形状"
    );
}

/// `POST /v1/import` `on_timeout=allow`：校验阶段即拒（fail-closed，L-12，绝不应用）。
#[tokio::test]
async fn post_import_on_timeout_allow_is_rejected_at_validation() {
    let resp = ok_app()
        .oneshot(json_req(
            "/v1/import",
            serde_json::json!({ "on_timeout": "allow" }),
        ))
        .await
        .expect("router serves");
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::OK,
        "import on_timeout=allow 绝不回 200（绝不应用成在线放行，L-12）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("on_timeout_allow_rejected"),
        "import on_timeout=allow 错误信封机读码"
    );
}

/// `POST /v1/import` `on_timeout=deny`：校验通过 ⇒ 200 + 应用结果计数（`applied`）。
#[tokio::test]
async fn post_import_on_timeout_deny_applies() {
    let resp = ok_app()
        .oneshot(json_req(
            "/v1/import",
            serde_json::json!({ "on_timeout": "deny" }),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "import on_timeout=deny ⇒ 校验通过 200"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("applied").and_then(|v| v.as_bool()),
        Some(true),
        "应用结果信封含 applied"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  健康：D1 已接真，确认 200 + status:"ok" + policy_rev
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/health`：回 200 + `status:"ok"` + `policy_rev`（取自注入 policy_rev=7）。
#[tokio::test]
async fn get_health_returns_status_and_policy_rev() {
    let resp = ok_app()
        .oneshot(req("GET", "/v1/health", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK, "健康端点回 200");
    let json = body_json(resp).await;
    assert_eq!(
        json.get("status").and_then(|s| s.as_str()),
        Some("ok"),
        "健康投影 status 恒 ok（进程已 serving 才命中本 handler）"
    );
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_u64()),
        Some(7),
        "policy_rev 取自当前权威快照修订号（运维对账锚点）"
    );
}
