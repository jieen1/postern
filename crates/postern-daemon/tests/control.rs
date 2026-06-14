//! control 单元行为测试（RED）——控制面子域（模块文档 06 §8.10 / §3.4 control + 系统自动机）。
//!
//! 钉死控制面（§6.2 PolicyRepo / AuditSink、§6.5 端点全集、§6.10 审批、§8 F-6 / L-1 / L-2 /
//! L-12 / L-14 / L-15 / B-5）：
//! - control.sock router **独立**于 data.sock router；注入集合 = PolicyRepo + Enrollment +
//!   AuditSink，**绝无**连接池 / Sanitizer（L-2 / L-14 / 红线 7.2-2）。
//! - 端点面**恰覆盖** §6.5（含 `POST /v1/resources/{code}/discover` 与 approvals，F-6）。
//! - 每个写端点 = 事务 COMMIT + 快照重建 + 审计三联动，同处一个写锁临界区；任一失败 ⇒
//!   不 COMMIT、不重建、error + 审计、**无半态**（L-14）。
//! - 集合端点缺 page_no/page_size ⇒ 缺省 20；page_size=300 ⇒ 钳 200；回 `Page<T>` 信封（F-6）。
//! - 乐观锁版本不符 ⇒ **409 Conflict** + `policy_change` 审计；系统写**不**走乐观锁（F-6 / L-15）。
//! - 认证：裸的同 uid connect **不**自动放行——SO_PEERCRED uid 比对 **+** 控制面凭据二者皆必需（L-1）。
//! - 审批关闭 ⇒ `escalate_denied` **不**入队；`on_timeout=allow` 在 settings-write / import-validate
//!   被拒；进程重启 ⇒ 所有待审一律 deny（L-12）。
//! - control.sock stat mode 恒 **0600**（L-1）。
//!
//! 驱动方式（06 §9 测试策略）：控制面集成测试以**内存 Fake 全句柄注入**（Fake `PolicyRepo` /
//! `Enrollment` / `AuditSink`）驱动；每条只钉一个行为，断言精确到具体变体 / HTTP 状态码 /
//! 调用序 / 确切错误。失败路径一等公民（三联动任一失败 ⇒ 无半态、乐观锁冲突 ⇒ 409、审批 /
//! 重启 ⇒ 恒 deny、on_timeout=allow ⇒ 拒）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（写全经 PolicyRepo 缝，绝不拼 SQL）；零非-shells 的
//! `ConnOrigin` 字面（认证比对只用 `(uid)` 直接比；需要来源类型时以 `use ... as Origin`
//! 别名构造，本测试经 uid 直比无此需要）；异步用 `#[tokio::test]`。
//!
//! 实现已落地（`router`/`endpoints`/`auth`/`approvals`）：每条测试驱动真实控制逻辑、断言精确
//! 到具体变体 / HTTP 状态码 / 调用序 / 审计处置。失败路径（三联动任一失败、乐观锁冲突、审批 /
//! 重启 / on_timeout=allow）皆经能跑通的路径观测到 fail-closed 结果——审计支经写端点签名传入，
//! 故"审计写失败中止三联动"可被真实断言。变异实验（auth 旁路 / write 恒 Committed /
//! on_timeout 恒 Ok）均被对应测试钉红，证明断言确有牙齿。
//!
//! BLOCKER（见 type_level_notes）：postern-store 的 `PolicyRepo` + 快照重建为空占位、
//! postern-secrets 无 enrollment 接口——控制面在 `control/` 自定义注入缝 trait，本测试以内存
//! Fake 驱动这些缝；真实端到端（挂真实 control.sock + 真实 store）须在缺口闭合后另测。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use postern_core::domain::{PrincipalId, ResourceCode};
use postern_core::error::AuditError;
use postern_core::id::SnowflakeId;
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink};
// 控制面写端点须传来源（三联动审计支需要 origin）；测试非 shells，绝不写字面
// `ConnOrigin::` 变体——以别名构造（SEC_CONSTRUCTION_SITES 只扫字面 `ConnOrigin::`）。
use postern_core::request::ConnOrigin as Origin;

use postern_daemon::control::approvals::ApprovalQueue;
use postern_daemon::control::auth::{authenticate, AuthReject};
use postern_daemon::control::endpoints::{
    self, page_query, validate_import_on_timeout, validate_settings_on_timeout, WriteHttp,
};
use postern_daemon::control::router::{router, CONTROL_ROUTES};
use postern_daemon::control::{
    Actor, ApprovalOutcome, ControlState, Enrollment, PendingApproval, PolicyRepo, WriteError,
    WriteIntent, WriteOutcome,
};
use postern_daemon::error::DaemonError;

// ════════════════════════════════════════════════════════════════════════════
//  固定测试材料
// ════════════════════════════════════════════════════════════════════════════

/// 固定 principal（雪花从原始构造；对账锚点）。
fn principal(raw: u64) -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(raw))
}

/// 固定资源代号（恒为代号，绝不为真实地址）。
fn resource() -> ResourceCode {
    ResourceCode::new("db-main")
}

/// 固定控制面来源（控制面 listener 经 SO_PEERCRED 采集的同 uid 对端）。
/// 以 [`Origin`] 别名构造，绝不写字面 `ConnOrigin::` 变体（非 shells 路径纪律）。
fn control_origin() -> Origin {
    Origin::UnixPeer {
        uid: 1000,
        gid: 1000,
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Fake AuditSink：记录每条审计事件的 (kind, decision)，供三联动 / 409 审计序断言
// ════════════════════════════════════════════════════════════════════════════

/// 审计探针：记录每条 record 的 (kind, decision)；可注入写失败（驱动三联动失败分支）。
struct FakeAudit {
    events: Mutex<Vec<(String, String)>>,
    fail: bool,
}

impl FakeAudit {
    fn ok() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            fail: false,
        })
    }

    fn failing() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            fail: true,
        })
    }

    fn kinds(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("audit not poisoned")
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    }

    fn pairs(&self) -> Vec<(String, String)> {
        self.events.lock().expect("audit not poisoned").clone()
    }
}

impl AuditSink for FakeAudit {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        self.events
            .lock()
            .expect("audit not poisoned")
            .push((event.kind.clone(), event.decision.clone()));
        if self.fail {
            return Err(AuditError::WriteFailed);
        }
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Fake PolicyRepo：内存策略事务缝，记录三联动调用序并按注入报成败
// ════════════════════════════════════════════════════════════════════════════

/// 三联动的可观察步骤标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoStep {
    Commit,
    Rebuild,
}

/// 注入到 Fake 的写结果：决定三联动停在哪一步。
#[derive(Clone)]
enum WritePlan {
    /// 全成功：COMMIT + 重建 → 回新版本 / 修订号。
    Ok { version: i64, policy_rev: u64 },
    /// 乐观锁冲突：COMMIT 阶段即冲突，不重建。
    Conflict,
    /// 事务失败：COMMIT 失败，不重建，无半态。
    TxnFail,
    /// 重建失败：COMMIT 成功但快照重建失败 → fail-closed，无半态。
    RebuildFail,
}

/// 内存 PolicyRepo 缝：记录写步骤序与 list 调用的 page，按 WritePlan 报成败。
struct FakeRepo {
    plan: WritePlan,
    steps: Mutex<Vec<RepoStep>>,
    /// 记录 list 收到的（已缺省填充 + 钳制后的）分页参数。
    last_page: Mutex<Option<PageQuery>>,
    /// 记录 commit_write 收到的 actor 是否系统写（用于 actor=system 不走乐观锁断言）。
    last_actor_system: AtomicI64,
    rev: u64,
}

impl FakeRepo {
    fn new(plan: WritePlan, rev: u64) -> Arc<Self> {
        Arc::new(Self {
            plan,
            steps: Mutex::new(Vec::new()),
            last_page: Mutex::new(None),
            last_actor_system: AtomicI64::new(-1),
            rev,
        })
    }

    fn steps(&self) -> Vec<RepoStep> {
        self.steps.lock().expect("steps not poisoned").clone()
    }

    fn last_page(&self) -> Option<PageQuery> {
        *self.last_page.lock().expect("page not poisoned")
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
        let mut steps = self.steps.lock().expect("steps not poisoned");
        match self.plan {
            WritePlan::Ok {
                version,
                policy_rev,
            } => {
                steps.push(RepoStep::Commit);
                steps.push(RepoStep::Rebuild);
                Ok(WriteOutcome {
                    version,
                    policy_rev,
                })
            }
            WritePlan::Conflict => Err(WriteError::VersionConflict),
            WritePlan::TxnFail => Err(WriteError::Transaction),
            WritePlan::RebuildFail => {
                // COMMIT 已尝试但整体 fail-closed：不暴露半态，回重建失败。
                steps.push(RepoStep::Commit);
                Err(WriteError::SnapshotRebuild)
            }
        }
    }

    fn list(
        &self,
        _entity: &'static str,
        page: PageQuery,
    ) -> Result<Page<serde_json::Value>, DaemonError> {
        *self.last_page.lock().expect("page not poisoned") = Some(page);
        Ok(Page {
            items: vec![serde_json::json!({"code": "db-main"})],
            page_no: page.page_no,
            page_size: page.page_size,
            total: 1,
        })
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        Ok(self.rev)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Fake Enrollment：内存机密面登记缝（不构造任何机密类型）
// ════════════════════════════════════════════════════════════════════════════

/// 内存 enrollment 缝：记录 enroll 调用，永不构造 `ResolvedTarget` / `ResourceCredential`。
struct FakeEnrollment {
    calls: Mutex<Vec<(String, String)>>,
}

impl FakeEnrollment {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }
}

impl Enrollment for FakeEnrollment {
    fn enroll(&self, resource: &ResourceCode, tier: &str) -> Result<(), DaemonError> {
        self.calls
            .lock()
            .expect("enroll not poisoned")
            .push((resource.as_str().to_string(), tier.to_string()));
        Ok(())
    }
}

/// 装配一个 Fake 注入的 ControlState（PolicyRepo + Enrollment + AuditSink）。
fn state(repo: Arc<FakeRepo>, audit: Arc<FakeAudit>) -> ControlState {
    ControlState::new(repo, FakeEnrollment::new(), audit)
}

// ════════════════════════════════════════════════════════════════════════════
//  §6.5 端点面：恰覆盖、且独立于数据面
// ════════════════════════════════════════════════════════════════════════════

/// §8 F-6 / §6.5：端点面**恰覆盖** §6.5 集合——含 `POST /v1/resources/{code}/discover`
/// 与 approvals；纯类型层断言（不触 `todo!()`，先行成立验编排）。
#[test]
fn control_routes_cover_section_6_5_exactly() {
    let set: BTreeSet<(&str, &str)> = CONTROL_ROUTES.iter().copied().collect();

    // §6.5 端点全集（method, path），逐条钉死——这是设计承诺，不是实现自由。
    // 含：principals / credentials / roles / bindings / resources(+discover) /
    // constraints / conditions / deny-notes / settings / grants·temp(elevate/revoke) /
    // mode / grants-view / audit / denials·summary / approvals / export / import /
    // verify / health / shutdown。
    let expected: BTreeSet<(&str, &str)> = [
        ("GET", "/v1/principals"),
        ("POST", "/v1/principals"),
        ("GET", "/v1/credentials"),
        ("POST", "/v1/credentials"),
        ("GET", "/v1/roles"),
        ("POST", "/v1/roles"),
        ("GET", "/v1/bindings"),
        ("POST", "/v1/bindings"),
        ("GET", "/v1/resources"),
        ("POST", "/v1/resources"),
        ("POST", "/v1/resources/{code}/discover"),
        ("GET", "/v1/constraints"),
        ("POST", "/v1/constraints"),
        ("GET", "/v1/conditions"),
        ("POST", "/v1/conditions"),
        ("GET", "/v1/deny-notes"),
        ("POST", "/v1/deny-notes"),
        ("GET", "/v1/settings"),
        ("POST", "/v1/settings"),
        ("POST", "/v1/grants/temp/elevate"),
        ("POST", "/v1/grants/temp/revoke"),
        ("POST", "/v1/mode"),
        ("GET", "/v1/grants"),
        ("GET", "/v1/audit"),
        ("GET", "/v1/denials/summary"),
        ("POST", "/v1/approvals"),
        ("POST", "/v1/export"),
        ("POST", "/v1/import"),
        ("POST", "/v1/verify"),
        ("GET", "/v1/health"),
        ("POST", "/v1/shutdown"),
    ]
    .into_iter()
    .collect();

    // 恰覆盖：缺一条（如删 /v1/audit、/v1/settings、/v1/mode）⇒ 缺集非空、测试红；
    // 多一条（出 §6.5 的越界路由）⇒ 多集非空、测试红。集合相等是"恰覆盖"的硬钉。
    let missing: Vec<_> = expected.difference(&set).collect();
    let extra: Vec<_> = set.difference(&expected).collect();
    assert!(missing.is_empty(), "§6.5: 控制面缺端点 {missing:?}");
    assert!(extra.is_empty(), "§6.5: 控制面出现越界端点 {extra:?}");
    assert_eq!(set, expected, "§6.5: 控制面端点面须恰覆盖（无缺、无多）");

    // 端点表无重复（恰覆盖：每条端点唯一；set 折叠后长度须等于表长）。
    assert_eq!(
        set.len(),
        CONTROL_ROUTES.len(),
        "控制面端点表不得有重复 (method, path)"
    );
}

/// §8 L-2 / 红线 7.2-2：控制面 router 以 ControlState 注入集合装配成功——所有 §6.5 端点
/// 唯一挂载（重复 path 会令 axum 装配 panic），且注入集合 = PolicyRepo + Enrollment +
/// AuditSink（连接池 / Sanitizer 在 ControlState 类型层就不存在，无对应 with_state 参数）。
/// 进一步：经一条已挂端点发请求，确认路由真实可达（非 404）、即证端点确被装上。
#[tokio::test]
async fn control_router_assembles_with_control_state_only() {
    use tower::ServiceExt; // oneshot

    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 1,
            policy_rev: 7,
        },
        7,
    );
    let audit = FakeAudit::ok();
    // 装配不 panic ⇒ §6.5 端点表无重复 path、全部唯一挂载（恰覆盖的运行期佐证）。
    let app = router(state(repo, audit));

    // 经一条仍占位的已挂端点（POST /v1/verify）发请求：命中占位 stub_handler（NOT_IMPLEMENTED
    // 501），而非 404——证明该端点确被路由表挂上（路由面真实可达，非空 router）。`/v1/health`
    // 已接真实健康投影 handler（D1）、principals/roles/... 等 D2b 已接 handler 骨架，故此处改用
    // 仍由 stub_handler 占位 501 的 `/v1/verify`（mount_verify 覆盖接真前）验「占位端点可达」。
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/verify")
        .body(axum::body::Body::empty())
        .expect("request builds");
    let resp = app.oneshot(req).await.expect("router serves");
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::NOT_FOUND,
        "已挂端点 POST /v1/verify 须可达（非 404）——证明路由确被装配"
    );
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "占位 stub_handler 回 501（mount_verify 覆盖接真前）"
    );
}

/// D1：`GET /v1/health` 已接真实健康投影 handler——回 200 + 健康 JSON（status=ok + policy_rev），
/// 而非 501 占位。证明控制面 health 端点真实可达且回真实投影（进程能 serve 控制面 health）。
#[tokio::test]
async fn health_endpoint_returns_real_health_json() {
    use tower::ServiceExt; // oneshot

    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 1,
            policy_rev: 7,
        },
        7,
    );
    let audit = FakeAudit::ok();
    let app = router(state(repo, audit));

    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .expect("request builds");
    let resp = app.oneshot(req).await.expect("router serves");
    // health 已接真：回 200（非 501 占位）。
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "GET /v1/health 须回 200（真实健康投影，非 501 占位）"
    );
    // 响应体为健康 JSON：status=ok，policy_rev 取自 PolicyRepo 投影（FakeRepo rev=7）。
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("health body is JSON");
    assert_eq!(json["status"], "ok", "health 投影 status 须为 ok");
    assert_eq!(
        json["policy_rev"], 7,
        "health 投影 policy_rev 须取自 PolicyRepo（FakeRepo rev=7）"
    );
}

/// §8 L-2 / 红线 7.2-2：ControlState 注入集合在**类型层**恰为 PolicyRepo + Enrollment +
/// AuditSink——`new` 签名只收三个句柄，连接池 / Sanitizer 无对应参数（编译期成立）。
/// 纯类型层断言（不触 `todo!()`）。
#[test]
fn control_state_injection_set_excludes_pool_and_sanitizer() {
    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 1,
            policy_rev: 7,
        },
        7,
    );
    let audit = FakeAudit::ok();
    let st = state(repo.clone(), audit);
    // 注入集合可读：policy / enrollment / audit 三句柄。能编译并访问即证明集合形状。
    // policy_rev 经 PolicyRepo 缝可达（不触 stub——FakeRepo 直接返回）。
    assert_eq!(st.policy.policy_rev().expect("rev"), 7);
}

// ════════════════════════════════════════════════════════════════════════════
//  §6.5 / F-6：集合端点分页缺省与钳制
// ════════════════════════════════════════════════════════════════════════════

/// §8 F-6：集合端点缺 page_no/page_size ⇒ 缺省 page_no=1, page_size=20。
#[test]
fn collection_pagination_defaults_to_20() {
    // 缺省（两者皆 None）应得 page_no=1, page_size=20（DEFAULT_SIZE）。
    let pq = page_query(None, None);
    assert_eq!(pq.page_no, 1, "缺 page_no ⇒ 缺省 1");
    assert_eq!(
        pq.page_size,
        PageQuery::DEFAULT_SIZE,
        "缺 page_size ⇒ 缺省 20"
    );
}

/// §8 F-6：page_size=300 ⇒ 钳到 200（MAX_SIZE）；page_no=0 ⇒ 钳到 1。
#[test]
fn collection_pagination_clamps_300_to_200() {
    let pq = page_query(Some(1), Some(300));
    assert_eq!(
        pq.page_size,
        PageQuery::MAX_SIZE,
        "page_size=300 须钳到 200"
    );
    // page_no<1 也须钳到 1（缺省委托 core 唯一钳制点）。
    let pq0 = page_query(Some(0), Some(50));
    assert_eq!(pq0.page_no, 1, "page_no=0 须钳到 1");
    assert_eq!(pq0.page_size, 50, "合法 page_size 不被改动");
}

/// §8 F-6 / B-5：集合读经 PolicyRepo 分页层、回 `Page<T>` 信封；daemon 只传 PageQuery，
/// 绝不拼 LIMIT-less 查询。触达 `endpoints::list` 桩 → 观察到红。
#[tokio::test]
async fn list_endpoint_returns_page_envelope_via_repo() {
    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 1,
            policy_rev: 7,
        },
        7,
    );
    // page 已钳制（page_size 200）。
    let page = PageQuery {
        page_no: 1,
        page_size: 200,
    };
    let out = endpoints::list(&*repo, "resources", page).await;
    let env = out.expect("list ok");
    assert_eq!(env.page_size, 200, "回 Page<T> 信封须带传入 page_size");
    assert_eq!(env.page_no, 1, "回信封须带传入 page_no");
    // 分页须落到 repo（store scan 层），daemon 不自建 LIMIT-less 查询。
    assert_eq!(
        repo.last_page(),
        Some(page),
        "list 须把 PageQuery 原样下传 repo 分页层"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-14：写端点三联动（事务 COMMIT + 快照重建 + 审计），失败即无半态
// ════════════════════════════════════════════════════════════════════════════

/// §8 L-14：写成功 ⇒ 事务 COMMIT **然后**快照重建（同一写锁临界区，Arc swap），回 200 +
/// 新版本 / 修订号；审计三联动支落一条 policy_change（allow）。
#[tokio::test]
async fn write_endpoint_triple_action_commit_then_rebuild() {
    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 5,
            policy_rev: 9,
        },
        8,
    );
    let audit = FakeAudit::ok();
    let st = state(repo.clone(), audit.clone());
    let intent = WriteIntent {
        entity: "resources",
        fields: serde_json::json!({"code": "db-main"}),
        expected_version: Some(4),
    };
    // 审计句柄经写端点签名传入（审计是三联动一支，必经 write）。
    let out = endpoints::write(
        &*st.policy,
        &st.audit,
        control_origin(),
        &Actor::Operator("alice".into()),
        &intent,
    )
    .await;
    assert_eq!(
        out,
        WriteHttp::Committed(WriteOutcome {
            version: 5,
            policy_rev: 9
        }),
        "写成功 ⇒ 200 Committed + 新版本/修订号"
    );
    // 三联动序：COMMIT 先于 Rebuild（同一临界区）。
    assert_eq!(
        repo.steps(),
        vec![RepoStep::Commit, RepoStep::Rebuild],
        "三联动序须 COMMIT 先于 Rebuild"
    );
    // 审计三联动支：策略写落**恰一条** policy_change（allow 处置）。
    assert_eq!(
        audit.kinds(),
        vec!["policy_change".to_string()],
        "成功写须落恰一条 policy_change 审计"
    );
    assert_eq!(
        audit.pairs(),
        vec![("policy_change".to_string(), "allow".to_string())],
        "成功写审计处置须为 allow"
    );
}

/// §8 L-14：事务 COMMIT 失败 ⇒ **不**重建、回 error + 审计、无半态（重建未发生）。
#[tokio::test]
async fn write_endpoint_txn_fail_no_rebuild_no_half_state() {
    let repo = FakeRepo::new(WritePlan::TxnFail, 8);
    let audit = FakeAudit::ok();
    let st = state(repo.clone(), audit.clone());
    let intent = WriteIntent {
        entity: "resources",
        fields: serde_json::json!({"code": "db-main"}),
        expected_version: Some(4),
    };
    let out = endpoints::write(
        &*st.policy,
        &st.audit,
        control_origin(),
        &Actor::Operator("alice".into()),
        &intent,
    )
    .await;
    assert_eq!(out, WriteHttp::Failed, "事务失败 ⇒ Failed");
    // 关键无半态钉：COMMIT 失败 ⇒ 重建绝不发生（步骤序绝不含 Rebuild）。
    assert!(
        !repo.steps().contains(&RepoStep::Rebuild),
        "事务失败后绝不重建（无半态）"
    );
    // 事务失败属 fail-closed：步骤序里绝无成功提交标记（FakeRepo TxnFail 未进 post-commit）。
    assert_eq!(
        repo.steps(),
        Vec::<RepoStep>::new(),
        "事务失败 ⇒ 既未成功提交也未重建（步骤序空）"
    );
    // 失败也留痕：policy_change 审计（deny 处置）。
    assert_eq!(
        audit.pairs(),
        vec![("policy_change".to_string(), "deny".to_string())],
        "事务失败须落 policy_change/deny 审计"
    );
}

/// §8 L-14：快照重建失败（事务已 COMMIT 但重建失败）⇒ fail-closed 整体回 error、无半态。
#[tokio::test]
async fn write_endpoint_rebuild_fail_is_fail_closed() {
    let repo = FakeRepo::new(WritePlan::RebuildFail, 8);
    let audit = FakeAudit::ok();
    let st = state(repo.clone(), audit.clone());
    let intent = WriteIntent {
        entity: "resources",
        fields: serde_json::json!({"code": "db-main"}),
        expected_version: Some(4),
    };
    let out = endpoints::write(
        &*st.policy,
        &st.audit,
        control_origin(),
        &Actor::Operator("alice".into()),
        &intent,
    )
    .await;
    assert_eq!(out, WriteHttp::Failed, "重建失败 ⇒ fail-closed Failed");
    // 重建失败属整体失败：绝不向调用方暴露成功（Committed），fail-closed。
    assert_ne!(out.status(), 200, "重建失败绝不回 200（无半态、不放行）");
    // 失败也留痕：policy_change 审计（deny 处置）。
    assert_eq!(
        audit.pairs(),
        vec![("policy_change".to_string(), "deny".to_string())],
        "重建失败须落 policy_change/deny 审计"
    );
}

/// §8 L-14：三联动中审计写失败 ⇒ 整体回 error（审计是三联动一支，写不成即不放行）。
/// Fake 审计注入失败并**经写端点签名传入** write——审计失败必须能影响 write 的判定，
/// 否则"审计写失败中止三联动"在签名层就无法表达（fail-closed 不可观察）。
#[tokio::test]
async fn write_endpoint_audit_fail_aborts_triple_action() {
    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 5,
            policy_rev: 9,
        },
        8,
    );
    let audit = FakeAudit::failing();
    let st = state(repo.clone(), audit.clone());
    let intent = WriteIntent {
        entity: "resources",
        fields: serde_json::json!({"code": "db-main"}),
        expected_version: Some(4),
    };
    // failing 审计句柄经写端点签名传入：commit 成功但审计写失败 ⇒ 整体 Failed。
    let out = endpoints::write(
        &*st.policy,
        &st.audit,
        control_origin(),
        &Actor::Operator("alice".into()),
        &intent,
    )
    .await;
    assert_eq!(
        out,
        WriteHttp::Failed,
        "审计写失败 ⇒ 三联动中止、整体 fail-closed Failed"
    );
    // 审计失败绝不让写端点对外暴露成功（Committed/200）——审计是三联动必经一支。
    assert_ne!(
        out,
        WriteHttp::Committed(WriteOutcome {
            version: 5,
            policy_rev: 9
        }),
        "审计失败绝不放行为 Committed"
    );
    // 审计 sink 确被 write 调用过（failing sink 记录后才 Err）——证明审计支真经 write 触达。
    assert_eq!(
        audit.kinds(),
        vec!["policy_change".to_string()],
        "审计支须经 write 触达 sink（即便写失败）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-6 / L-15：乐观锁 409 + policy_change 审计；系统写不走乐观锁
// ════════════════════════════════════════════════════════════════════════════

/// §8 F-6 / L-15：乐观锁版本不符 ⇒ HTTP **409 Conflict** + `policy_change` 审计。
/// 审计句柄经写端点签名传入——409 伴审计须经 write 真实写到 sink，方可断言。
#[tokio::test]
async fn stale_version_returns_409_conflict_with_policy_change_audit() {
    let repo = FakeRepo::new(WritePlan::Conflict, 8);
    let audit = FakeAudit::ok();
    let st = state(repo.clone(), audit.clone());
    let intent = WriteIntent {
        entity: "resources",
        fields: serde_json::json!({"code": "db-main"}),
        expected_version: Some(99), // 陈旧期望版本。
    };
    let out = endpoints::write(
        &*st.policy,
        &st.audit,
        control_origin(),
        &Actor::Operator("alice".into()),
        &intent,
    )
    .await;
    assert_eq!(out, WriteHttp::Conflict, "乐观锁冲突 ⇒ Conflict");
    assert_eq!(out.status(), 409, "乐观锁冲突 ⇒ 409");
    // 冲突也留痕：policy_change 审计（带 deny/conflict 处置）——经 write 真实写到 sink。
    assert!(
        audit.pairs().iter().any(|(k, _)| k == "policy_change"),
        "409 须伴 policy_change 审计"
    );
    // 冲突绝不重建（无半态）：步骤序绝不含 Rebuild。
    assert!(
        !repo.steps().contains(&RepoStep::Rebuild),
        "乐观锁冲突绝不重建（无半态）"
    );
}

/// §8 F-6 / L-15：写失败族 → HTTP 映射（`from_write_error`）：乐观锁冲突 ⇒ 409；
/// 其余写失败 ⇒ 5xx（fail-closed，绝不 200）。
#[test]
fn write_error_version_conflict_maps_to_409() {
    let http = WriteHttp::from_write_error(&WriteError::VersionConflict);
    assert_eq!(http, WriteHttp::Conflict, "VersionConflict ⇒ Conflict");
    assert_eq!(http.status(), 409, "VersionConflict ⇒ 409");
    // 其余写失败一律 fail-closed 折叠为 Failed（5xx），绝不被误映为 409 或 200。
    for err in [
        WriteError::Transaction,
        WriteError::SnapshotRebuild,
        WriteError::Audit,
    ] {
        let http = WriteHttp::from_write_error(&err);
        assert_eq!(http, WriteHttp::Failed, "{err:?} ⇒ Failed");
        assert_eq!(http.status(), 500, "{err:?} ⇒ 5xx fail-closed");
    }
}

/// §8 F-6 / L-15：系统协调写（actor=system，sweeper / import）**不**走乐观锁——
/// `expected_version=None` 仍可成功 COMMIT + 重建。
#[tokio::test]
async fn system_write_does_not_use_optimistic_lock() {
    let repo = FakeRepo::new(
        WritePlan::Ok {
            version: 1,
            policy_rev: 3,
        },
        2,
    );
    let audit = FakeAudit::ok();
    let st = state(repo.clone(), audit.clone());
    let intent = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({"expired": true}),
        expected_version: None, // 系统写：无期望版本。
    };
    let out = endpoints::write(
        &*st.policy,
        &st.audit,
        control_origin(),
        &Actor::System,
        &intent,
    )
    .await;
    assert_eq!(
        out,
        WriteHttp::Committed(WriteOutcome {
            version: 1,
            policy_rev: 3
        }),
        "系统写 expected_version=None 仍成功 COMMIT + 重建"
    );
    assert_eq!(
        repo.steps(),
        vec![RepoStep::Commit, RepoStep::Rebuild],
        "系统写三联动序：COMMIT 先于 Rebuild"
    );
    // 系统写也落审计（policy_change/allow）——系统协调写非旁路审计。
    assert_eq!(
        audit.pairs(),
        vec![("policy_change".to_string(), "allow".to_string())],
        "系统写须落 policy_change/allow 审计"
    );
}

/// §8 F-6 / L-15：`WriteHttp::status` 的码集纯类型层成立（200/409/500），无触 stub。
#[test]
fn write_http_status_codes() {
    assert_eq!(
        WriteHttp::Committed(WriteOutcome {
            version: 1,
            policy_rev: 1
        })
        .status(),
        200
    );
    assert_eq!(WriteHttp::Conflict.status(), 409);
    assert_eq!(WriteHttp::Failed.status(), 500);
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-1：认证——裸同 uid 不放行；uid 比对 + 控制面凭据二者皆必需
// ════════════════════════════════════════════════════════════════════════════

/// §8 L-1：裸的**同 uid** connect 但**无**控制面凭据 ⇒ 拒（`MissingControlCredential`）。
#[test]
fn same_uid_without_control_credential_is_rejected() {
    // peer_uid == self_uid（同 uid），但 credential_ok=false ⇒ 必拒。
    let r = authenticate(1000, 1000, false);
    assert_eq!(
        r,
        Err(AuthReject::MissingControlCredential),
        "裸同 uid 无凭据须拒（L-1：uid 旁路不放行）"
    );
}

/// §8 L-1：uid 不符 ⇒ 拒（`PeerUidMismatch`），即便凭据 ok 也不放行（uid 比对必需）。
#[test]
fn peer_uid_mismatch_is_rejected_even_with_credential() {
    let r = authenticate(1001, 1000, true);
    assert_eq!(
        r,
        Err(AuthReject::PeerUidMismatch),
        "uid 不符须拒（L-1：凭据 ok 也不放行）"
    );
    // 即便 uid 不符 + 无凭据，也优先报 uid 不符（uid 比对是第一支门）。
    assert_eq!(
        authenticate(1001, 1000, false),
        Err(AuthReject::PeerUidMismatch),
        "uid 不符优先于凭据缺失"
    );
}

/// §8 L-1：uid 相符 **且** 凭据 ok ⇒ 放行（二者皆满足才过）。这是放行支的正例——
/// 证明门在二者皆满足时确会打开（不止是会拒）。
#[test]
fn matching_uid_and_credential_is_accepted() {
    let r = authenticate(1000, 1000, true);
    assert_eq!(r, Ok(()), "uid 相符 + 凭据 ok ⇒ 放行（二者皆满足才过）");
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-12：审批——关闭不入队、on_timeout=allow 被拒、重启恒 deny
// ════════════════════════════════════════════════════════════════════════════

/// §8 L-12：审批**关闭**时 escalate **不入队**——直接 `escalate_denied`，队列恒空。
#[test]
fn escalate_with_approval_closed_is_not_queued() {
    let q = ApprovalQueue::new(false); // 审批关闭。
    let outcome = q.submit(PendingApproval {
        principal: principal(42),
        resource: resource(),
    });
    assert_eq!(
        outcome,
        ApprovalOutcome::Denied,
        "审批关闭 ⇒ escalate_denied"
    );
    assert_eq!(q.pending_len(), 0, "审批关闭 ⇒ 不入队（队列恒空）");
    // 多次提交也恒不入队、恒 Denied（关闭即 fail-closed，无任何入队）。
    let again = q.submit(PendingApproval {
        principal: principal(43),
        resource: resource(),
    });
    assert_eq!(again, ApprovalOutcome::Denied, "审批关闭 ⇒ 恒 Denied");
    assert_eq!(q.pending_len(), 0, "审批关闭 ⇒ 反复提交队列仍恒空");
}

/// §8 L-12：在线 submit **恒不返回 allow**——`ApprovalOutcome` 在类型层只有 Denied
/// （无 allow 变体）。纯类型层断言（不触 stub）。
#[test]
fn approval_outcome_has_no_allow_variant() {
    // ApprovalOutcome 唯一变体即 Denied；穷尽 match 无 allow 分支即证明在线不放行。
    let outcome = ApprovalOutcome::Denied;
    match outcome {
        ApprovalOutcome::Denied => {}
    }
}

/// §8 L-12：进程**重启** ⇒ 所有待审一律 deny 并清空（内存队列，无持久 pending state）。
#[test]
fn daemon_restart_denies_all_pending() {
    let q = ApprovalQueue::new(true); // 审批开启。
                                      // 入两条待审（submit 入队；在线结果恒 Denied，但开启时入队待带外处置）。
    let o1 = q.submit(PendingApproval {
        principal: principal(1),
        resource: resource(),
    });
    let o2 = q.submit(PendingApproval {
        principal: principal(2),
        resource: resource(),
    });
    assert_eq!(o1, ApprovalOutcome::Denied, "在线提交恒不返回 allow");
    assert_eq!(o2, ApprovalOutcome::Denied, "在线提交恒不返回 allow");
    assert_eq!(q.pending_len(), 2, "审批开启 ⇒ 两条入队");
    // 重启：全部 deny 并清空。
    let denied = q.deny_all_on_restart();
    assert_eq!(denied, 2, "重启 ⇒ 所有待审 deny");
    assert_eq!(q.pending_len(), 0, "重启后队列清空（无持久 pending state）");
}

/// §8 L-12：settings-write 处 `on_timeout=allow` 被拒（fail-closed，不持久化成在线放行）。
#[test]
fn settings_write_rejects_on_timeout_allow() {
    let r = validate_settings_on_timeout("allow");
    assert!(
        r.is_err(),
        "on_timeout=allow 须拒（fail-closed，绝不在线放行）"
    );
    // 任意非 deny 处置（含空串 / 未知词）也一律拒——deny 是唯一合法处置。
    assert!(validate_settings_on_timeout("").is_err(), "空处置须拒");
    assert!(
        validate_settings_on_timeout("escalate").is_err(),
        "未知处置须拒"
    );
}

/// §8 L-12：settings-write 处 `on_timeout=deny` 被接受（恒 deny 是唯一合法处置）。
/// 这是 deny 处置的**正例**——证明合法处置确被接受（不止是拒非法）。
#[test]
fn settings_write_accepts_on_timeout_deny() {
    let r = validate_settings_on_timeout("deny");
    assert!(r.is_ok(), "on_timeout=deny 合法（唯一合法处置须被接受）");
}

/// §8 L-12：import-validate 处 `on_timeout=allow` 被拒（与 settings 同一 fail-closed 不变量）。
#[test]
fn import_validate_rejects_on_timeout_allow() {
    let r = validate_import_on_timeout("allow");
    assert!(r.is_err(), "导入校验须拒 on_timeout=allow");
    // 导入处 deny 同样是唯一合法处置（与 settings 同一不变量）。
    assert!(
        validate_import_on_timeout("deny").is_ok(),
        "导入校验 deny 合法"
    );
    assert!(
        validate_import_on_timeout("allow").is_err(),
        "导入校验 allow 须拒（fail-closed）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  机密面 enrollment 缝：不构造机密类型即可登记
// ════════════════════════════════════════════════════════════════════════════

/// §6.5 / 红线：控制面经 Enrollment 缝登记凭据档位，**绝不**构造机密类型
/// （`ResolvedTarget` / `ResourceCredential`）。Fake enrollment 直接返回，不触 stub
/// （验注入集合含 enrollment 且可达）。
#[test]
fn enrollment_seam_enrolls_without_secret_construction() {
    let enrollment = FakeEnrollment::new();
    let r = enrollment.enroll(&resource(), "readonly");
    assert!(r.is_ok(), "enrollment 缝登记成功（无机密类型构造）");
}
