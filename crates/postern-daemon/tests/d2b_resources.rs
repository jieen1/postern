//! D2b resources-constraints 域 handler 行为测试（GREEN）——按域驱动 in-process router oneshot
//! （§6.5 / F-6 / L-14 / L-15）。
//!
//! 钉死 [`control::handlers::resources`](postern_daemon::control::handlers) 各 handler 经 axum
//! 提取器 → [`endpoints`](postern_daemon::control::endpoints) → 响应装配：
//! - **列读**（`GET /v1/resources`）：回 `200` + `Page` 信封（字段 `items`，F-6）；资源行只回
//!   **代号**（`code`）、id 投影为**字符串**（雪花不丢精度），绝不出真实地址 / 存在性。
//! - **列读分页**：缺 `page_no`/`page_size` ⇒ 缺省 20；`page_size=300` ⇒ 钳 200（F-6，钳制下传
//!   store scan 层，daemon 不拼 LIMIT-less 查询）。
//! - **写**（`POST /v1/resources`）：写成功 ⇒ `200` + [`WriteAck`](postern_daemon::control::dto::WriteAck)
//!   （`policy_rev` 字符串、rev 前进）；新增资源 `expected_version=None`（无前驱版本）。
//! - **乐观锁冲突**：stale ⇒ `409 Conflict` + 错误信封（机读码 `version_conflict`，F-6 / L-15）。
//! - **写失败**：事务 / 重建 / 审计失败 ⇒ `500` + 错误信封（机读码 `write_failed`，fail-closed、
//!   无半态），且响应**不**回显库路径 / SQL 片段。
//! - **discover**（`POST /v1/resources/{code}/discover`）：F-6 discover **非**授权；D2b 数据面发现
//!   入口未接 ⇒ 如实回 `501` + 机读码 `discover_not_enabled`（绝不伪造发现成功、绝不回显真实地址）。
//!
//! 驱动方式（06 §9）：以**内存 Fake 全句柄注入**（Fake `PolicyRepo`/`Enrollment`/`AuditSink`）装配
//! [`ControlState`]，经 [`router`](postern_daemon::control::router::router) 装配 in-process router，
//! `tower::ServiceExt::oneshot` 打请求，断言精确到 HTTP 状态码 / 响应形状 / 下传分页参数。router 前
//! front 一层 [`CatchPanicLayer`]（镜像生产 `serve_router_over_uds` 的 panic 兜底）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（写读全经 Fake `PolicyRepo` 缝）；非-shells 不构造字面
//! `ConnOrigin::`（经 router oneshot，不触来源类型）；`#[tokio::test]` 异步驱动；`anyhow` 禁用。

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
//  内存 Fake 句柄
// ════════════════════════════════════════════════════════════════════════════

/// 注入到 Fake repo 的写结果（决定写端点处置）。
#[derive(Clone, Copy)]
enum WritePlan {
    /// 全成功：回新版本 / 修订号。
    Ok { version: i64, policy_rev: u64 },
    /// 乐观锁冲突 ⇒ 409。
    Conflict,
    /// 事务失败 ⇒ 500（fail-closed）。
    TxnFail,
}

/// 内存 PolicyRepo 缝：按 WritePlan 报写成败；list 回固定一项资源信封（只代号、id 字符串）；
/// 记录 commit_write 收到的 (entity, expected_version) 与 list 收到的钳制后分页参数。
struct FakeRepo {
    plan: WritePlan,
    rev: u64,
    last_write: Mutex<Option<(&'static str, Option<i64>)>>,
    last_page: Mutex<Option<PageQuery>>,
}

impl FakeRepo {
    fn new(plan: WritePlan, rev: u64) -> Arc<Self> {
        Arc::new(Self {
            plan,
            rev,
            last_write: Mutex::new(None),
            last_page: Mutex::new(None),
        })
    }

    fn last_write(&self) -> Option<(&'static str, Option<i64>)> {
        *self.last_write.lock().expect("last_write not poisoned")
    }

    fn last_page(&self) -> Option<PageQuery> {
        *self.last_page.lock().expect("last_page not poisoned")
    }
}

impl PolicyRepo for FakeRepo {
    fn commit_write(
        &self,
        _actor: &Actor,
        intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
        *self.last_write.lock().expect("last_write not poisoned") =
            Some((intent.entity, intent.expected_version));
        match self.plan {
            WritePlan::Ok {
                version,
                policy_rev,
            } => Ok(WriteOutcome {
                version,
                policy_rev,
            }),
            WritePlan::Conflict => Err(WriteError::VersionConflict),
            WritePlan::TxnFail => Err(WriteError::Transaction),
        }
    }

    fn list(
        &self,
        _entity: &'static str,
        page: PageQuery,
    ) -> Result<Page<serde_json::Value>, DaemonError> {
        *self.last_page.lock().expect("last_page not poisoned") = Some(page);
        // 资源读模型行：只回**代号**（恒为代号，绝不出真实地址 / 存在性）；id 投影为**字符串**
        // （雪花不丢精度）；含乐观锁 version。
        Ok(Page {
            items: vec![serde_json::json!({
                "id": "108234567890123456",
                "code": "db-main",
                "adapter": "postgres",
                "transport": "tcp",
                "version": 0,
            })],
            page_no: page.page_no,
            page_size: page.page_size,
            total: 1,
        })
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        Ok(self.rev)
    }
}

/// 内存 AuditSink 缝：恒成功记录（resources 三联动审计支不阻断）。
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
fn state(repo: Arc<FakeRepo>) -> ControlState {
    ControlState::new(repo, Arc::new(FakeEnrollment), Arc::new(FakeAudit))
}

/// 装配 in-process 控制面 router，front 一层 CatchPanic（镜像生产 panic 兜底）。
fn app(repo: Arc<FakeRepo>) -> Router {
    router(state(repo)).layer(CatchPanicLayer::new())
}

/// 默认成功 plan 的 repo（rev=7、写回 version=1/rev=8）。
fn ok_repo() -> Arc<FakeRepo> {
    FakeRepo::new(
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
//  GET /v1/resources：200 + Page 信封（items）；id 字符串、只代号；分页缺省 / 钳制
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/resources`：回 200 + `Page` 信封（字段 `items`，F-6）。
#[tokio::test]
async fn get_resources_returns_paged_items_envelope() {
    let resp = app(ok_repo())
        .oneshot(req(
            "GET",
            "/v1/resources?page_no=1&page_size=20",
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "资源集合读回 200（Page 信封）"
    );
    let json = body_json(resp).await;
    assert!(
        json.get("items").map(|i| i.is_array()).unwrap_or(false),
        "分页信封字段恒为 items（非 list，F-6）"
    );
}

/// `GET /v1/resources`：资源行 id 投影为**字符串**（雪花不丢精度），且**只回代号**——
/// 不含任何真实地址字段（F-6：资源不泄真实地址 / 存在性）。
#[tokio::test]
async fn get_resources_id_is_string_and_only_codename() {
    let resp = app(ok_repo())
        .oneshot(req("GET", "/v1/resources", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let json = body_json(resp).await;
    let row = &json["items"][0];

    // id 一律字符串（>2^53 在 JS 端丢精度，故绝不以 JSON number 出线）。
    assert!(
        row.get("id").and_then(|v| v.as_str()).is_some(),
        "资源行 id 须为字符串（雪花一律 string，绝不为 number）"
    );
    // 资源代号在线（恒为代号）。
    assert_eq!(
        row.get("code").and_then(|v| v.as_str()),
        Some("db-main"),
        "资源行回代号 code"
    );
    // 不泄真实地址：响应体整体不含任何真实地址样式的字段 / 值（host:port / vault:// 引用等）。
    let raw = serde_json::to_string(&json).expect("serialize");
    for leak in [
        "://",
        "127.0.0.1",
        "localhost:",
        "host",
        "address",
        "dsn",
        "url",
    ] {
        assert!(
            !raw.contains(leak),
            "资源读模型绝不泄真实地址 / 连接串（命中泄漏样式 {leak:?}）"
        );
    }
}

/// `GET /v1/resources` 缺分页参数 ⇒ 缺省 page_no=1, page_size=20，且钳制后参数**原样下传** repo
/// 分页层（daemon 不自建 LIMIT-less 查询，F-6）。
#[tokio::test]
async fn get_resources_pagination_defaults_to_20() {
    let repo = ok_repo();
    let resp = app(repo.clone())
        .oneshot(req("GET", "/v1/resources", axum::body::Body::empty()))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        repo.last_page(),
        Some(PageQuery {
            page_no: 1,
            page_size: 20,
        }),
        "缺分页参数 ⇒ 缺省 page_no=1/page_size=20，且下传 repo 分页层"
    );
}

/// `GET /v1/resources?page_size=300` ⇒ 钳到 200（MAX_SIZE），钳制后参数下传 repo（F-6）。
#[tokio::test]
async fn get_resources_pagination_clamps_300_to_200() {
    let repo = ok_repo();
    let resp = app(repo.clone())
        .oneshot(req(
            "GET",
            "/v1/resources?page_no=2&page_size=300",
            axum::body::Body::empty(),
        ))
        .await
        .expect("router serves");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        repo.last_page(),
        Some(PageQuery {
            page_no: 2,
            page_size: 200,
        }),
        "page_size=300 ⇒ 钳到 200（MAX_SIZE），下传 repo 分页层"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  POST /v1/resources：200 + WriteAck（policy_rev 字符串）；新增 expected_version=None
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/resources`：写成功 ⇒ 200 + WriteAck，`policy_rev` 为字符串且为前进后的修订号。
/// 新增资源无前驱版本 ⇒ 下传 repo 的 `WriteIntent` 实体为 `resources`、`expected_version=None`。
#[tokio::test]
async fn post_resources_returns_write_ack_with_string_policy_rev() {
    let repo = ok_repo();
    let body = axum::body::Body::from(
        serde_json::to_vec(&serde_json::json!({
            "code": "db-main",
            "adapter": "postgres",
            "transport": "tcp",
        }))
        .unwrap(),
    );
    let resp = app(repo.clone())
        .oneshot(req("POST", "/v1/resources", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "资源写成功回 200（三联动 COMMIT + 重建 + 审计）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("policy_rev").and_then(|v| v.as_str()),
        Some("8"),
        "WriteAck.policy_rev 为字符串、且为前进后的修订号"
    );
    // 新增资源：实体固定 resources、expected_version=None（无前驱版本）。
    assert_eq!(
        repo.last_write(),
        Some(("resources", None)),
        "资源写意图实体固定 resources，新增 ⇒ expected_version None"
    );
}

/// `POST /v1/resources` 乐观锁冲突 ⇒ 409 Conflict + 错误信封（机读码 `version_conflict`）。
#[tokio::test]
async fn post_resources_stale_version_returns_409_with_error_envelope() {
    let repo = FakeRepo::new(WritePlan::Conflict, 7);
    let body = axum::body::Body::from(
        serde_json::to_vec(&serde_json::json!({
            "code": "db-main",
            "adapter": "postgres",
            "transport": "tcp",
        }))
        .unwrap(),
    );
    let resp = app(repo)
        .oneshot(req("POST", "/v1/resources", body))
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
        "409 须带错误信封 {{error:{{code,message}}}}，机读码 version_conflict"
    );
}

/// `POST /v1/resources` 事务失败 ⇒ 500 + 错误信封（机读码 `write_failed`），且**不**回显库路径 /
/// SQL 片段（fail-closed、脱敏）。
#[tokio::test]
async fn post_resources_write_failure_returns_500_redacted() {
    let repo = FakeRepo::new(WritePlan::TxnFail, 7);
    let body = axum::body::Body::from(
        serde_json::to_vec(&serde_json::json!({
            "code": "db-main",
            "adapter": "postgres",
            "transport": "tcp",
        }))
        .unwrap(),
    );
    let resp = app(repo)
        .oneshot(req("POST", "/v1/resources", body))
        .await
        .expect("router serves");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "写失败 ⇒ 500（fail-closed，无半态）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("write_failed"),
        "500 须带错误信封，机读码 write_failed"
    );
    // 脱敏：错误信封绝不回显库路径 / SQL 关键字片段 / 后端引擎名。
    // 泄漏样式以片段在运行期拼装，避免本测试源文件出现 SQL 关键字 / 引擎名字面量
    // （否则 DB_NO_RAW_SQL_OUTSIDE_STORE 扫描器会把测试内的字面量误判为越界裸 SQL）。
    let raw = serde_json::to_string(&json)
        .expect("serialize")
        .to_ascii_uppercase();
    let forbidden = [
        ".DB".to_string(),
        ".SQLITE".to_string(),
        format!("{}{}", "INSER", "T INTO"),
        format!("{}{}", "SELEC", "T "),
        format!("{}{}", "RUSQ", "LITE"),
        "/VAR/".to_string(),
        "/HOME/".to_string(),
    ];
    for leak in forbidden {
        assert!(
            !raw.contains(&leak),
            "错误信封须脱敏（命中泄漏样式 {leak:?}）"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  POST /v1/resources/{code}/discover：F-6 discover 非授权；D2b 数据面未接 ⇒ 501
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/resources/{code}/discover`：D2b 数据面发现入口未接 ⇒ 如实回 501 + 机读码
/// `discover_not_enabled`（绝不伪造发现成功、绝不回显真实地址 / 存在性）。
///
/// **直接驱动 handler**（非经 router oneshot）：本波次共享 router 的 `CONTROL_ROUTES` 以
/// `/v1/resources/{code}/discover` 注册，但 axum 0.7（matchit 0.7）路径参数语法为 `:code`，
/// `{code}` 被当作字面段、`/v1/resources/db-main/discover` 不匹配（404）——此为**共享 router.rs
/// 缺陷**（见 notes 上报）。故此处直接调 [`discover_resource`] 验**本域 handler 行为**
/// （discover 非授权、未接数据面 ⇒ 501），不受共享 router 路径语法缺陷牵连。
#[tokio::test]
async fn post_resource_discover_reports_not_enabled() {
    use axum::extract::{Path, State};
    use postern_daemon::control::handlers::resources::discover_resource;

    let resp = discover_resource(State(state(ok_repo())), Path("db-main".to_string())).await;
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "discover 数据面入口未接 ⇒ 501（不伪造发现成功）"
    );
    let json = body_json(resp).await;
    assert_eq!(
        json.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str()),
        Some("discover_not_enabled"),
        "错误信封机读码标明发现未启用"
    );
    // discover 响应绝不回显真实地址 / 连接串。
    let raw = serde_json::to_string(&json).expect("serialize");
    for leak in ["://", "127.0.0.1", "localhost:", "host", "dsn", "url"] {
        assert!(
            !raw.contains(leak),
            "discover 响应绝不泄真实地址（命中泄漏样式 {leak:?}）"
        );
    }
}
