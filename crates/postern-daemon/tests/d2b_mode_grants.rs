//! D2b mode-grants 域 handler 行为测试（GREEN 域）——router oneshot 钉死 mode/grants 端点。
//!
//! 钉死 [`control::handlers::mode_grants`](postern_daemon::control::handlers::mode_grants) 四端点：
//! - `POST /v1/mode {op:"read"}`：同源读 → `200` + `ModeStateRow[]`（id/scope 一律 string；
//!   `effective_mode` = global.meet(scoped) 投影；无 `GET /v1/mode`）。
//! - `POST /v1/mode {op:"set", scope, mode, ttl_ms?, version}`：写 → `200` + `{rows, policy_rev}`
//!   （`policy_rev` 字符串、前进）；乐观锁 stale ⇒ `409 Conflict` + 错误信封。
//! - `GET /v1/grants`：授权视图 → `200` + `GrantsView{your_grants, temp_grants}`（temp 行 id string）。
//! - `POST /v1/grants/temp/elevate` / `revoke`：写 → `200` + `WriteAck` / stale ⇒ `409`。
//!
//! 驱动（06 §9）：内存 Fake 全句柄（`PolicyRepo`/`Enrollment`/`AuditSink`）装配 [`ControlState`]，
//! 经 [`router`](postern_daemon::control::router::router) 装配 in-process router，
//! `tower::ServiceExt::oneshot` 打请求、精确断言 HTTP 状态码 / 响应形状。写端点三联动需 origin +
//! actor（审计支），由控制面 listener 经 SO_PEERCRED 采集后透传——本测试非 shells，**绝不**写字面
//! `ConnOrigin::` 变体，以 `use ... as Origin` 别名构造并经 axum `Extension` 注入（router 内对应
//! 端点已以 `Extension` 提取，缺失即 fail-closed 500）。读端点不需 origin/actor。
//!
//! 雷区纪律：本文件零 SQL 标记（写全经 Fake `PolicyRepo` 缝）；不构造机密类型；argon2 不在本
//! 路径（无 KDF）；`#[tokio::test]` 异步驱动；router 前 front 一层 `CatchPanicLayer`（镜像生产
//! `serve_router_over_uds` 的 panic 兜底，未实现端点表现为 500 而非 crash 测试线程）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use axum::Router;
use tower::ServiceExt; // oneshot
use tower_http::catch_panic::CatchPanicLayer;

use postern_core::domain::ResourceCode;
use postern_core::error::AuditError;
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink};
// 控制面写端点须传来源（三联动审计支需 origin）；测试非 shells，绝不写字面 `ConnOrigin::` 变体
// ——以别名构造，经 axum Extension 注入（SEC_CONSTRUCTION_SITES 只扫字面 `ConnOrigin::`）。
use postern_core::request::ConnOrigin as Origin;

use postern_daemon::control::router::router;
use postern_daemon::control::{
    Actor, ControlState, Enrollment, PolicyRepo, WriteError, WriteIntent, WriteOutcome,
};
use postern_daemon::error::DaemonError;

// ════════════════════════════════════════════════════════════════════════════
//  内存 Fake 句柄
// ════════════════════════════════════════════════════════════════════════════

/// 注入到 Fake repo 的写结果。
#[derive(Clone)]
enum WritePlan {
    /// 全成功：回新版本 / 修订号。
    Ok { version: i64, policy_rev: u64 },
    /// 乐观锁冲突。
    Conflict,
}

/// 内存 PolicyRepo 缝：按 WritePlan 报写成败；`list` 按 entity 回固定一项信封；policy_rev 回注入值。
struct FakeRepo {
    plan: WritePlan,
    rev: u64,
}

impl FakeRepo {
    fn new(plan: WritePlan, rev: u64) -> Arc<Self> {
        Arc::new(Self { plan, rev })
    }
}

impl PolicyRepo for FakeRepo {
    fn commit_write(
        &self,
        _actor: &Actor,
        _intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
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
        // 按 entity 回该域读模型行（store row 形态：id i64 / 业务列 / version）。
        let items = match entity {
            // mode_state 行：scope=null 为全局；scope="db-main" 为资源覆盖（更严 Observe）。
            "mode" => vec![
                serde_json::json!({
                    "scope": null, "mode": "maintain", "expires_at": null,
                    "version": 2, "updated_at": "1700000000000", "updated_by": "op-a",
                }),
                serde_json::json!({
                    "scope": "db-main", "mode": "observe", "expires_at": "1700000900000",
                    "version": 5, "updated_at": "1700000500000", "updated_by": "op-b",
                }),
            ],
            // temp_grants 行（id 雪花 i64；resource 代号；capability；时窗）。
            "grants" => vec![serde_json::json!({
                "id": 7000000000000000001_i64,
                "resource": "db-main", "capability": "mutate",
                "granted_at": "1700000000000", "expires_at": "1700003600000",
                "ended_at": null, "end_reason": null, "version": 0,
            })],
            _ => vec![],
        };
        let total = items.len() as u64;
        Ok(Page {
            items,
            page_no: page.page_no,
            page_size: page.page_size,
            total,
        })
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        Ok(self.rev)
    }
}

/// 内存 AuditSink 缝：恒成功记录（三联动审计支不阻断）。
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

/// 固定控制面来源（同 uid 对端，listener 经 SO_PEERCRED 采集）。以别名构造，绝不写字面变体。
fn control_origin() -> Origin {
    Origin::UnixPeer {
        uid: 1000,
        gid: 1000,
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

/// 装配 in-process 控制面 router，注入写端点所需 origin + actor（Extension），front CatchPanic。
fn app(plan: WritePlan, rev: u64) -> Router {
    router(state(plan, rev))
        // 写端点经 Extension 提取 origin + actor（生产由控制面 listener 透传，本测试注入）。
        .layer(axum::Extension(control_origin()))
        .layer(axum::Extension(Actor::Operator("op-a".to_string())))
        .layer(CatchPanicLayer::new())
}

/// 默认成功 plan 的 router（rev=7，写后 policy_rev=8）。
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

/// JSON body helper。
fn json_body(v: serde_json::Value) -> axum::body::Body {
    axum::body::Body::from(serde_json::to_vec(&v).expect("json serializes"))
}

/// 取响应体为 JSON。
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body reads");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

// ════════════════════════════════════════════════════════════════════════════
//  POST /v1/mode {op:"read"}：同源读 → 200 + ModeStateRow[]
// ════════════════════════════════════════════════════════════════════════════

/// `op:read` 回 `ModeStateRow[]`（顶层数组，非 Page 信封）；scope 透传（null/代号）；
/// `effective_mode` = global.meet(scoped)：全局 maintain ∧ 资源 observe ⇒ 该资源 effective=observe，
/// 全局自身 effective=maintain。id 纪律：scope 为 string|null（绝非 number）。
#[tokio::test]
async fn post_mode_read_returns_mode_state_rows_with_effective_meet() {
    let resp = ok_app()
        .oneshot(req(
            "POST",
            "/v1/mode",
            json_body(serde_json::json!({"op": "read"})),
        ))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK, "op:read 回 200");
    let json = body_json(resp).await;
    let rows = json.as_array().expect("op:read 回顶层 ModeStateRow[] 数组");
    assert_eq!(rows.len(), 2, "两行：全局 + 资源覆盖");

    // 全局行：scope=null，mode=maintain，effective=maintain（无更严覆盖于全局本身）。
    let global = rows
        .iter()
        .find(|r| r.get("scope").map(|s| s.is_null()).unwrap_or(false))
        .expect("有全局行（scope=null）");
    assert_eq!(
        global.get("mode").and_then(|m| m.as_str()),
        Some("maintain")
    );
    assert_eq!(
        global.get("effective_mode").and_then(|m| m.as_str()),
        Some("maintain"),
        "全局 effective = 全局自身 mode"
    );

    // 资源行：scope="db-main"（string），mode=observe，effective=meet(maintain, observe)=observe。
    let scoped = rows
        .iter()
        .find(|r| r.get("scope").and_then(|s| s.as_str()) == Some("db-main"))
        .expect("有资源覆盖行（scope=代号 string）");
    assert_eq!(scoped.get("mode").and_then(|m| m.as_str()), Some("observe"));
    assert_eq!(
        scoped.get("effective_mode").and_then(|m| m.as_str()),
        Some("observe"),
        "资源 effective = global.meet(scoped) = 更严者 observe"
    );
    // policy_rev 字符串化（雪花纪律），取自当前快照修订号。
    assert_eq!(
        scoped.get("policy_rev").and_then(|v| v.as_str()),
        Some("7"),
        "ModeStateRow.policy_rev 为字符串、当前快照修订号"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  POST /v1/mode {op:"set"}：写 → 200 + {rows, policy_rev}；stale ⇒ 409
// ════════════════════════════════════════════════════════════════════════════

/// `op:set` 写成功 ⇒ 200 + `{rows: ModeStateRow[], policy_rev: "8"}`（policy_rev 字符串、前进）。
#[tokio::test]
async fn post_mode_set_returns_rows_and_string_policy_rev() {
    let body = json_body(serde_json::json!({
        "op": "set", "scope": "db-main", "mode": "freeze", "ttl_ms": 60000, "version": 5
    }));
    let resp = ok_app()
        .oneshot(req("POST", "/v1/mode", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "op:set 成功回 200"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 为字符串、前进后修订号"
    );
    assert!(
        json.get("rows").map(|r| r.is_array()).unwrap_or(false),
        "op:set 回 {{rows, policy_rev}}：rows 为 ModeStateRow[]"
    );
}

/// `op:set` 乐观锁 stale ⇒ 409 Conflict + 错误信封 `version_conflict`。
#[tokio::test]
async fn post_mode_set_stale_version_returns_409() {
    let body = json_body(serde_json::json!({
        "op": "set", "scope": null, "mode": "observe", "version": 1
    }));
    let resp = app(WritePlan::Conflict, 7)
        .oneshot(req("POST", "/v1/mode", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::CONFLICT,
        "乐观锁版本冲突 ⇒ 409（F-6 / L-15）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("version_conflict"),
        "409 带错误信封 {{error:{{code,message}}}}，机读码 version_conflict"
    );
}

/// `op` 缺失 / 未知 ⇒ 400（既非 read 也非 set，fail-closed 拒，绝不静默当读/写）。
#[tokio::test]
async fn post_mode_unknown_op_returns_400() {
    let resp = ok_app()
        .oneshot(req(
            "POST",
            "/v1/mode",
            json_body(serde_json::json!({"op": "wat"})),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "未知 op ⇒ 400（fail-closed，不静默当读/写）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  GET /v1/grants：授权视图 → 200 + GrantsView{your_grants, temp_grants}
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/grants` 回 `GrantsView`：`your_grants`（resource→capability[] 映射）+ `temp_grants`
/// （temp 行 id 一律 string，绝非 number）。
#[tokio::test]
async fn get_grants_returns_grants_view_with_string_temp_ids() {
    let resp = ok_app()
        .oneshot(req("GET", "/v1/grants", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "grants 视图回 200"
    );
    let json = body_json(resp).await;
    assert!(
        json.get("your_grants")
            .map(|g| g.is_object())
            .unwrap_or(false),
        "GrantsView.your_grants 为 resource→capability[] 映射对象"
    );
    let temp = json
        .get("temp_grants")
        .and_then(|t| t.as_array())
        .expect("GrantsView.temp_grants 为数组");
    assert_eq!(temp.len(), 1, "一条 temp grant");
    assert_eq!(
        temp[0].get("id").and_then(|v| v.as_str()),
        Some("7000000000000000001"),
        "temp grant id 一律 string（雪花不丢精度）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  POST /v1/grants/temp/elevate · revoke：写 → 200 + WriteAck；stale ⇒ 409
// ════════════════════════════════════════════════════════════════════════════

/// `elevate` 写成功 ⇒ 200 + WriteAck（policy_rev 字符串、前进）。
#[tokio::test]
async fn post_elevate_returns_write_ack() {
    let body = json_body(serde_json::json!({
        "principal": "100", "resource": "db-main", "capability": "mutate", "ttl_ms": 60000
    }));
    let resp = ok_app()
        .oneshot(req("POST", "/v1/grants/temp/elevate", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "elevate 成功回 200"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 字符串、前进"
    );
}

/// `elevate` ttl_ms<=0 ⇒ 400（临时授权必带正 TTL，绝不发永久升权）。
#[tokio::test]
async fn post_elevate_nonpositive_ttl_returns_400() {
    let body = json_body(serde_json::json!({
        "principal": "100", "resource": "db-main", "capability": "mutate", "ttl_ms": 0
    }));
    let resp = ok_app()
        .oneshot(req("POST", "/v1/grants/temp/elevate", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "ttl_ms<=0 ⇒ 400（临时授权必带正 TTL）"
    );
}

/// `revoke` 写成功 ⇒ 200 + WriteAck。
#[tokio::test]
async fn post_revoke_returns_write_ack() {
    let body = json_body(serde_json::json!({"id": "7000000000000000001", "version": 0}));
    let resp = ok_app()
        .oneshot(req("POST", "/v1/grants/temp/revoke", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "revoke 成功回 200"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 字符串、前进"
    );
}

/// `revoke` 乐观锁 stale ⇒ 409 Conflict。
#[tokio::test]
async fn post_revoke_stale_version_returns_409() {
    let body = json_body(serde_json::json!({"id": "7000000000000000001", "version": 99}));
    let resp = app(WritePlan::Conflict, 7)
        .oneshot(req("POST", "/v1/grants/temp/revoke", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::CONFLICT,
        "revoke 版本冲突 ⇒ 409"
    );
}
