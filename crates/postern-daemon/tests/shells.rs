//! shells（HTTP / MCP 外壳服务端）单元行为测试（RED）。
//!
//! 钉死数据面外壳子域（模块文档 06 §3.3、§8 F-4 / F-5 / F-10 / L-4 / B-2、§6 装箱[0]）：
//! HTTP（axum）与 MCP（rmcp）两个 Router 共挂 data.sock、共用**同一注入集**与**同一装箱/
//! 提交入口**；listener 是 ConnOrigin 唯一构造点（SO_PEERCRED）、绝不采信请求自报来源；
//! MCP 暴露编译期固定动词工具面；`postern_surface` 只读快照投影不触后端；协议非法 4xx 仍过
//! 同一 Sanitizer 出常量安全文案。
//!
//! 驱动方式（06 §9）：**内存 Fake 全插件注入** —— Fake `Authenticator` / `Adapter` /
//! `ConditionPredicate` / `ConnAcquire` / `AuditSink` / `Sanitizer` + 纯内存 `PolicySnapshot`
//! 装出真实 `Kernel`，外壳测试经 `DataPlane` 提交并观察行为。每条只钉一个行为。
//!
//! 雷区纪律：本文件**零 SQL 标记**；需要 `ConnOrigin` 时以
//! `use postern_core::request::ConnOrigin as Origin` 别名构造（测试在 shells 外，绝不写字面
//! `ConnOrigin::` 变体）；**绝不构造** `ResolvedTarget` / `ResourceCredential`（建连缝直接
//! 交还不透明 `Channel`）。异步用 `#[tokio::test]`。
//!
//! 实现为 RED 桩（`box_request` / `surface` / `invalid_request_egress` 体为 `todo!()`，
//! `Kernel::submit` 同为桩），故凡触达装箱/提交/投影/非法出口的测试调用即 panic → 观察到红。
//! 纯结构层断言（固定工具面常量、注入集不含 PolicyRepo/vault 句柄的编译期事实）先于实现即绿。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use postern_core::domain::{
    Capability, ConstraintSpec, CredentialMeta, CredentialTier, CredentialView, EvalContext,
    GrantAction, GrantCell, PolicySnapshot, PresentedCredential, PrincipalId, ResourceCode, Role,
    TierDecl, Timestamp,
};
use postern_core::error::{
    AuditError, AuthError, ClassifyError, ConstraintError, DiscoverError, ExecError,
    PredicateError, TransportError,
};
use postern_core::eval::evaluator::Evaluator;
use postern_core::id::SnowflakeId;
use postern_core::plugin::sanitize::{MaskRule, SanitizedResponse, Sanitizer, StreamScrubber};
use postern_core::plugin::{
    Adapter, AuditEvent, AuditSink, Authenticator, CapabilitySurface, Channel, ConditionPredicate,
    RawResponse,
};
use postern_core::request::{ClassifiedIntent, Intent, NormalizedRequest, ObjectRef};
// 测试在 shells 外：需要请求来源以别名读/构造，绝不写字面 ConnOrigin:: 变体（雷区 2）。
use postern_core::request::ConnOrigin as Origin;

use postern_daemon::kernel::audit_phase::AuditClass;
use postern_daemon::kernel::pipeline::ConnAcquire;
use postern_daemon::kernel::Kernel;
use postern_daemon::shells::http::HttpSubmit;
use postern_daemon::shells::mcp::McpToolCall;
use postern_daemon::shells::{
    self, box_request, surface, DataPlane, SurfaceEntry, INVALID_REQUEST_SAFE_MESSAGE, MCP_TOOLS,
};

// ════════════════════════════════════════════════════════════════════════════
//  固定测试材料 + Fake 插件（全内存）
// ════════════════════════════════════════════════════════════════════════════

const RESOURCE: &str = "db-main";
const AUTH_KIND: &str = "api_key";
const COND_KIND: &str = "always";

fn principal(raw: u64) -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(raw))
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_700_000_000_000)
}

/// 测试来源：UNIX peer（uid/gid），以 Origin 别名构造（雷区 2：测试在 shells 外）。
fn unix_origin() -> Origin {
    Origin::UnixPeer {
        uid: 1000,
        gid: 1000,
    }
}

// ───────────────────────── Fake Authenticator ─────────────────────────

struct FakeAuth {
    outcome: Result<PrincipalId, AuthError>,
}

impl Authenticator for FakeAuth {
    fn kind(&self) -> &'static str {
        AUTH_KIND
    }

    fn authenticate(
        &self,
        _presented: &PresentedCredential,
        _origin: &Origin,
        _creds: &CredentialView,
        _now: Timestamp,
    ) -> Result<PrincipalId, AuthError> {
        self.outcome.clone()
    }
}

// ───────────────────────── Fake ConditionPredicate ─────────────────────────

struct FakePredicate {
    verdict: Result<bool, PredicateError>,
}

impl ConditionPredicate for FakePredicate {
    fn kind(&self) -> &'static str {
        COND_KIND
    }

    fn eval(&self, _ctx: &EvalContext, _spec: &serde_json::Value) -> Result<bool, PredicateError> {
        self.verdict.clone()
    }
}

// ───────────────────────── Fake Adapter ─────────────────────────

/// 适配器：记录每次触达（用于钉 `postern_surface` 绝不触达 `discover`，F-5）。
struct FakeAdapter {
    classify: Result<ClassifiedIntent, ClassifyError>,
    constraint: Result<bool, ConstraintError>,
    execute: Mutex<Option<Result<RawResponse, ExecError>>>,
    discover_calls: Arc<Mutex<usize>>,
}

impl FakeAdapter {
    fn classified(capability: Capability) -> ClassifiedIntent {
        ClassifiedIntent {
            capability,
            objects: vec![ObjectRef::new("obj:probe")],
        }
    }
}

#[async_trait]
impl Adapter for FakeAdapter {
    fn protocol(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[
            Capability::Observe,
            Capability::Query,
            Capability::Mutate,
            Capability::Execute,
            Capability::Manage,
            Capability::Destroy,
        ]
    }

    fn engine_enforced(&self) -> bool {
        false
    }

    fn classify(&self, _intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
        self.classify.clone()
    }

    fn check_constraint(
        &self,
        _spec: &ConstraintSpec,
        _ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError> {
        self.constraint.clone()
    }

    async fn execute(&self, _ch: &mut Channel, _intent: &Intent) -> Result<RawResponse, ExecError> {
        self.execute
            .lock()
            .expect("execute slot ok")
            .take()
            .expect("execute exercised at most once")
    }

    async fn discover(&self, _ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError> {
        // F-5 取证：`postern_surface` 绝不触达 discover；一旦被调用即记一次，断言其为 0。
        *self.discover_calls.lock().expect("discover counter ok") += 1;
        Ok(CapabilitySurface {
            capabilities: Vec::new(),
            objects: Vec::new(),
        })
    }
}

// ───────────────────────── Fake ConnAcquire ─────────────────────────

struct FakeAcquire;

impl ConnAcquire for FakeAcquire {
    fn acquire<'a>(
        &'a self,
        _resource: &'a ResourceCode,
        _tier: &'a CredentialTier,
    ) -> Pin<Box<dyn Future<Output = Result<Channel, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(Channel {
                handle: Box::new(()),
            })
        })
    }
}

// ───────────────────────── Fake AuditSink ─────────────────────────

struct FakeAudit;

impl AuditSink for FakeAudit {
    fn record(&self, _event: AuditEvent) -> Result<(), AuditError> {
        Ok(())
    }
}

// ───────────────────────── Recording AuditSink ─────────────────────────

/// 审计事件的可比对投影（`AuditEvent` 不实现 PartialEq/Clone；故投影其逐字段事实为可比较元组）。
///
/// 用于钉 F-4 的「审计」维度：同一逻辑请求经 HTTP 与 MCP 两路提交，落到审计汇的事件须逐字段
/// 相同。投影覆盖 v / kind / entry / origin / principal / resource / capability / objects /
/// decision / stage / reason / policy_rev 全部字段（无遗漏，确保「逐字段相同」真被观察）。
type AuditFacts = (
    u32,
    String,
    String,
    Origin,
    Option<PrincipalId>,
    ResourceCode,
    Option<Capability>,
    Vec<String>,
    String,
    Option<postern_core::error::Stage>,
    String,
    u64,
);

fn audit_facts(event: &AuditEvent) -> AuditFacts {
    (
        event.v,
        event.kind.clone(),
        event.entry.clone(),
        event.origin.clone(),
        event.principal,
        event.resource.clone(),
        event.capability,
        event
            .objects
            .iter()
            .map(|o| o.as_str().to_string())
            .collect(),
        event.decision.clone(),
        event.stage,
        event.reason.clone(),
        event.policy_rev,
    )
}

/// 记录型审计汇：捕获每条落汇事件的逐字段投影（供 F-4 审计等价对抗 —— 一个只 `Ok(())` 不留痕的
/// 汇即可骗过纯 `submit` 测试，故此处必须真正捕获事件内容/数量并断言）。
struct RecordingAudit {
    events: Arc<Mutex<Vec<AuditFacts>>>,
}

impl AuditSink for RecordingAudit {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        self.events
            .lock()
            .expect("audit events lock ok")
            .push(audit_facts(&event));
        Ok(())
    }
}

// ───────────────────────── Fake Sanitizer ─────────────────────────

/// 出口脱敏器：在每条出口字节前后各包一个可识别哨兵，使「确实过了同一 Sanitizer」在出口
/// 字节上**可观察**（F-10 / L-4：协议非法 4xx 出口也必须过同一脱敏，而非旁路裸传）。
struct SentinelSanitizer;

const SCRUB_PREFIX: &[u8] = b"<scrubbed>";

impl Sanitizer for SentinelSanitizer {
    fn scrub(&self, payload: RawResponse, _declared: &[MaskRule]) -> SanitizedResponse {
        let mut out = SCRUB_PREFIX.to_vec();
        out.extend_from_slice(&payload.payload);
        SanitizedResponse { payload: out }
    }

    fn scrub_stream(&self, _declared: &[MaskRule]) -> Box<dyn StreamScrubber> {
        Box::new(PassthroughScrubber)
    }
}

struct PassthroughScrubber;

impl StreamScrubber for PassthroughScrubber {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        chunk.to_vec()
    }

    fn finish(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  夹具：纯内存快照 + Kernel + DataPlane
// ════════════════════════════════════════════════════════════════════════════

/// 装一个放行快照：principal `p` 在 (RESOURCE, capability) 有一个 Allow 格，挂 always 条件
/// 与一条 constraint spec；RESOURCE 的 tier 声明承载该动词。
fn allow_snapshot(p: PrincipalId, capability: Capability) -> PolicySnapshot {
    let resource = ResourceCode::new(RESOURCE);
    let cell = GrantCell {
        resource: resource.clone(),
        capability,
        role: Role::new("operator"),
        action: GrantAction::Allow,
        constraints: vec![ConstraintSpec {
            kind: "object_allow".into(),
            spec: r#"{"allow":["obj:probe"]}"#.into(),
        }],
        conditions: vec![postern_core::domain::ConditionSpec {
            kind: COND_KIND.into(),
            spec: "{}".into(),
        }],
    };
    let mut per_principal = BTreeMap::new();
    per_principal.insert((resource.clone(), capability), cell);
    let mut grants = BTreeMap::new();
    grants.insert(p, per_principal);

    let mut tiers = BTreeMap::new();
    tiers.insert(
        resource.clone(),
        vec![TierDecl {
            tier: CredentialTier::new("readonly"),
            carries: vec![capability],
        }],
    );

    PolicySnapshot {
        policy_rev: 7,
        grants,
        tiers,
        credentials: CredentialView {
            credentials: vec![CredentialMeta {
                principal: p,
                kind: AUTH_KIND.into(),
                secret_hash: "hash".into(),
                expires_at: None,
                revoked_at: None,
            }],
        },
        deny_notes: BTreeMap::new(),
        grantable: BTreeMap::new(),
        modes: BTreeMap::new(),
    }
}

/// 在一个放行快照上**额外**挂一个折叠为 deny 的 `Escalate` 格（同 principal、不同动词坐标）。
///
/// 用于钉死 F-5 投影的 fail-closed 过滤：`surface()` 只投 `GrantAction::Allow` 坐标，Escalate
/// 格在审批关闭时折叠为 deny、非已授权能力，故**绝不**入投影。若实现把过滤改弱/删除，这个
/// Escalate 坐标会作为「已授权能力」泄漏进 surface（fail-open 信息披露），本夹具据此构造对抗证据。
fn allow_plus_escalate_snapshot(
    p: PrincipalId,
    allow_cap: Capability,
    escalate_cap: Capability,
) -> PolicySnapshot {
    let mut snapshot = allow_snapshot(p, allow_cap);
    let resource = ResourceCode::new(RESOURCE);
    let escalate_cell = GrantCell {
        resource: resource.clone(),
        capability: escalate_cap,
        role: Role::new("operator"),
        // 折叠为 deny 的升格格：审批关闭时非已授权能力，绝不入 surface 投影（F-5）。
        action: GrantAction::Escalate,
        constraints: Vec::new(),
        conditions: Vec::new(),
    };
    snapshot
        .grants
        .get_mut(&p)
        .expect("allow_snapshot 已为 p 建立授权世界")
        .insert((resource, escalate_cap), escalate_cell);
    snapshot
}

fn evaluator() -> Evaluator {
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(
        AUTH_KIND,
        Box::new(FakeAuth {
            outcome: Ok(principal(42)),
        }),
    );
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert(COND_KIND, Box::new(FakePredicate { verdict: Ok(true) }));
    Evaluator::new(auths, preds)
}

/// 组装一个放行内核（principal=42、capability=Query），并返回它 + 共享快照 + discover 计数。
fn passing_kernel() -> (Arc<Kernel>, Arc<PolicySnapshot>, Arc<Mutex<usize>>) {
    let p = principal(42);
    let snapshot = Arc::new(allow_snapshot(p, Capability::Query));
    let discover_calls = Arc::new(Mutex::new(0usize));
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: b"rows".to_vec(),
        }))),
        discover_calls: discover_calls.clone(),
    };
    let adapters = Arc::new(postern_daemon::registry::AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>,
    ]));
    let kernel = Kernel::new(
        Arc::new(evaluator()),
        adapters,
        Arc::new(FakeAcquire) as Arc<dyn ConnAcquire>,
        Arc::new(FakeAudit) as Arc<dyn AuditSink>,
        Arc::new(SentinelSanitizer) as Arc<dyn Sanitizer>,
        snapshot.clone(),
        now(),
    );
    (Arc::new(kernel), snapshot, discover_calls)
}

/// 组装一个 DataPlane（数据面注入集）+ 旁带 discover 计数（供 F-5 取证）。
fn data_plane() -> (DataPlane, Arc<PolicySnapshot>, Arc<Mutex<usize>>) {
    let (kernel, snapshot, discover_calls) = passing_kernel();
    let dp = DataPlane::new(
        kernel,
        snapshot.clone(),
        Arc::new(SentinelSanitizer) as Arc<dyn Sanitizer>,
    );
    (dp, snapshot, discover_calls)
}

/// 组装一个放行内核，注入**记录型**审计汇（principal=42、capability=Query），返回内核 +
/// 捕获到的审计事件投影句柄（供 F-4 审计等价对抗）。装配方式与 `passing_kernel` 完全一致，
/// 仅把 `FakeAudit` 替换为 `RecordingAudit`，使落汇事件可观察。
fn recording_kernel() -> (Arc<Kernel>, Arc<Mutex<Vec<AuditFacts>>>) {
    let p = principal(42);
    let snapshot = Arc::new(allow_snapshot(p, Capability::Query));
    let discover_calls = Arc::new(Mutex::new(0usize));
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: b"rows".to_vec(),
        }))),
        discover_calls,
    };
    let adapters = Arc::new(postern_daemon::registry::AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>,
    ]));
    let events = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::new(
        Arc::new(evaluator()),
        adapters,
        Arc::new(FakeAcquire) as Arc<dyn ConnAcquire>,
        Arc::new(RecordingAudit {
            events: events.clone(),
        }) as Arc<dyn AuditSink>,
        Arc::new(SentinelSanitizer) as Arc<dyn Sanitizer>,
        snapshot.clone(),
        now(),
    );
    (Arc::new(kernel), events)
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-4：HTTP 与 MCP 同一逻辑请求 → 字节等价的 NormalizedRequest
// ════════════════════════════════════════════════════════════════════════════

/// 把一个 NormalizedRequest 投影成可逐字段比对的元组（NormalizedRequest 不实现 PartialEq；
/// 且 presented/intent 的 Debug 恒 REDACTED，故比对它们的**实际承载字节/种类**而非 Debug）。
fn projected(req: &NormalizedRequest) -> (String, Vec<u8>, Origin, String, Vec<u8>) {
    (
        req.presented.kind().to_string(),
        req.presented.secret().to_vec(),
        req.origin.clone(),
        req.resource.as_str().to_string(),
        req.intent.payload().to_vec(),
    )
}

// §8 F-4：HTTP 与 MCP 两路对**同一逻辑请求**装出的 NormalizedRequest 逐字段字节等价 ——
// 内核看到的归一化请求与外壳无关（公理七：归一化后请求 shell-agnostic）。两路都收敛到同一
// box_request，故装箱结果不因外壳而异。
#[tokio::test]
async fn http_and_mcp_box_byte_equivalent_normalized_request() {
    let origin = unix_origin();

    // 同一逻辑请求：相同出示物 / 资源代号 / intent 字节，分别经 HTTP DTO 与 MCP 工具调用装箱。
    let http = HttpSubmit {
        auth_kind: AUTH_KIND.to_string(),
        secret: b"secret-bytes".to_vec(),
        resource: RESOURCE.to_string(),
        intent: b"the-same-payload".to_vec(),
    };
    let mcp = McpToolCall {
        tool: "postern_query".to_string(),
        auth_kind: AUTH_KIND.to_string(),
        secret: b"secret-bytes".to_vec(),
        resource: RESOURCE.to_string(),
        intent: b"the-same-payload".to_vec(),
    };

    // 触达 box_request（RED 桩）→ panic → 观察到红。
    let from_http = http.normalize(origin.clone());
    let from_mcp = mcp.normalize(origin.clone());

    assert_eq!(
        projected(&from_http),
        projected(&from_mcp),
        "HTTP 与 MCP 同一逻辑请求须装出逐字段字节等价的 NormalizedRequest（F-4：归一化后 \
         shell-agnostic）"
    );
}

// §8 B-2：listener 采集的来源**按值**进 NormalizedRequest；请求体/工具入参里的自报来源被
// 忽略 —— box_request 取的 origin 恒为 listener 传入者，与 DTO 内容无关。DTO 刻意无来源字段，
// 故同一来源 + 不同 DTO 业务内容时，归一化请求的 origin 恒等于 listener 来源。
#[tokio::test]
async fn boxed_origin_is_listener_supplied_not_self_reported() {
    let listener_origin = unix_origin();

    let http = HttpSubmit {
        auth_kind: AUTH_KIND.to_string(),
        secret: b"s".to_vec(),
        resource: RESOURCE.to_string(),
        intent: b"x".to_vec(),
    };
    // 触达 box_request（RED 桩）→ panic → 观察到红。
    let req = http.normalize(listener_origin.clone());
    assert_eq!(
        req.origin, listener_origin,
        "归一化请求的 origin 必恰为 listener 采集来源（B-2：自报来源绝不被采信）"
    );
}

// §8 B-2（直接装箱缝）：box_request 把传入 origin 原样落入 NormalizedRequest.origin，
// intent_bytes 原样裹入（只搬运不解释，绝不预解析），presented/resource 原样落位。
#[tokio::test]
async fn box_request_carries_inputs_verbatim() {
    let origin = unix_origin();
    // 触达 box_request（RED 桩）→ panic → 观察到红。
    let req = box_request(
        PresentedCredential::new(AUTH_KIND, b"sek".to_vec()),
        origin.clone(),
        ResourceCode::new(RESOURCE),
        b"raw-intent-bytes".to_vec(),
    );
    assert_eq!(req.origin, origin, "origin 原样落位");
    assert_eq!(req.resource.as_str(), RESOURCE, "资源代号原样落位");
    assert_eq!(req.presented.kind(), AUTH_KIND, "出示物种类原样落位");
    assert_eq!(req.presented.secret(), b"sek", "出示物秘密字节原样落位");
    assert_eq!(
        req.intent.payload(),
        b"raw-intent-bytes",
        "intent 原样裹入，外壳绝不预解析（公理七：只搬运不解释）"
    );
}

// §8 F-4：同一逻辑请求**分别经 HTTP 外壳 DTO 与 MCP 工具调用各自的归一化路径**装箱，再经同一
// 内核 submit，产出逐字节相同的出口 —— 两路在内核侧不可区分。关键：HTTP 路真正走
// `HttpSubmit::normalize`、MCP 路真正走 `McpToolCall::normalize`（而非两侧用同一辅助以相同入参
// 直调 submit）。若将来某一外壳 DTO 的 normalize 路径分叉（如 MCP 私自改写 intent/注入不同
// sanitizer/篡改来源），两路装出的 NormalizedRequest 即不同，出口字节随之分叉，本断言变红。
#[tokio::test]
async fn same_request_via_http_and_mcp_yields_identical_egress() {
    let (kernel_http, _) = recording_kernel();
    let (kernel_mcp, _) = recording_kernel();
    let origin = unix_origin();

    // 同一逻辑请求：相同出示物 / 资源代号 / intent 字节，分别经各自外壳 DTO 的 normalize 装箱。
    let http_req = HttpSubmit {
        auth_kind: AUTH_KIND.to_string(),
        secret: b"secret".to_vec(),
        resource: RESOURCE.to_string(),
        intent: b"probe".to_vec(),
    }
    .normalize(origin.clone());
    let mcp_req = McpToolCall {
        tool: "postern_query".to_string(),
        auth_kind: AUTH_KIND.to_string(),
        secret: b"secret".to_vec(),
        resource: RESOURCE.to_string(),
        intent: b"probe".to_vec(),
    }
    .normalize(origin.clone());

    // 各自经外壳 normalize 装出的 NormalizedRequest 交同一内核入口 submit。
    let via_http = kernel_http.submit(http_req).await;
    let via_mcp = kernel_mcp.submit(mcp_req).await;

    let http_bytes = via_http.expect("HTTP 路放行").payload;
    let mcp_bytes = via_mcp.expect("MCP 路放行").payload;
    assert_eq!(
        http_bytes, mcp_bytes,
        "同一逻辑请求经 HTTP 外壳与 MCP 外壳各自的 normalize 路径提交须产出逐字节相同的出口\
         （F-4：归一化后 shell-agnostic，两路装箱收敛同一缝）"
    );
    // 出口确经同一 Sanitizer（哨兵前缀可观察）。
    assert!(
        http_bytes.starts_with(SCRUB_PREFIX),
        "正常出口须过同一 Sanitizer（哨兵前缀可观察）"
    );
}

// §8 F-4（审计维度）：同一逻辑请求分别经 HTTP 外壳 DTO 与 MCP 工具调用归一化、提交后，落到
// 审计汇的事件须**逐字段相同**（决策/匿名化/脱敏/审计四项中的审计项）。关键：用记录型审计汇
// 捕获两路真实落汇的事件，逐字段比对 —— 一个只 `Ok(())` 不留痕的汇骗不过此断言（它要求事件
// 数量 == 1 且内容逐字段相等）。若 MCP 路私自改写归一化请求（改 intent/来源/资源），两路审计
// 事件即不同，本断言变红。
#[tokio::test]
async fn http_and_mcp_yield_identical_audit_events() {
    let (kernel_http, http_events) = recording_kernel();
    let (kernel_mcp, mcp_events) = recording_kernel();
    let origin = unix_origin();

    let http_req = HttpSubmit {
        auth_kind: AUTH_KIND.to_string(),
        secret: b"secret".to_vec(),
        resource: RESOURCE.to_string(),
        intent: b"probe".to_vec(),
    }
    .normalize(origin.clone());
    let mcp_req = McpToolCall {
        tool: "postern_query".to_string(),
        auth_kind: AUTH_KIND.to_string(),
        secret: b"secret".to_vec(),
        resource: RESOURCE.to_string(),
        intent: b"probe".to_vec(),
    }
    .normalize(origin.clone());

    kernel_http.submit(http_req).await.expect("HTTP 路放行");
    kernel_mcp.submit(mcp_req).await.expect("MCP 路放行");

    let http_log = http_events.lock().expect("http events lock ok").clone();
    let mcp_log = mcp_events.lock().expect("mcp events lock ok").clone();

    // 读动词（Query）放行路径恰写一条结果痕（F-3：读动词无意图痕，单条 record）。
    assert_eq!(
        http_log.len(),
        1,
        "Query 放行须恰写一条审计事件（读动词单条结果痕）——只 Ok(()) 不留痕的汇会令此处为 0 而变红"
    );
    assert_eq!(mcp_log.len(), 1, "MCP 路同理恰写一条审计事件");
    // F-4 审计项：两路落汇事件逐字段相同（origin/principal/resource/capability/objects/decision/
    // stage/reason/policy_rev 等全字段，见 audit_facts）。
    assert_eq!(
        http_log, mcp_log,
        "同一逻辑请求经 HTTP 与 MCP 须落下逐字段相同的审计事件（F-4：审计项 shell-agnostic）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-4：MCP 工具面编译期固定，不随授权动态增减
// ════════════════════════════════════════════════════════════════════════════

// §8 F-4：MCP 工具面恰为这八个固定动词工具，顺序/名称逐一钉死（编译期常量）。
#[test]
fn mcp_toolset_is_the_fixed_eight_verb_tools() {
    assert_eq!(
        MCP_TOOLS,
        [
            "postern_grants",
            "postern_query",
            "observe",
            "mutate",
            "execute",
            "manage",
            "destroy",
            "postern_surface",
        ],
        "MCP 工具面须恰为固定八动词工具（F-4：编译期固定，名称/顺序不漂移）"
    );
    assert_eq!(MCP_TOOLS.len(), 8, "工具面恰八个工具，不多不少");
    // mcp::tools() 交还的就是同一固定常量（外壳不自造工具）。
    assert_eq!(
        postern_daemon::shells::mcp::tools(),
        &MCP_TOOLS,
        "mcp::tools() 须交还固定 MCP_TOOLS（工具面不随调用动态构造）"
    );
}

// §8 F-4：工具面与授权**无关** —— 对一个空授权快照（principal 无任何 grant）与一个满授权
// 快照，工具面均为同一固定八工具（鉴权在 submit 之后的内核求值，而非工具面裁剪）。
#[test]
fn mcp_toolset_does_not_change_with_authorization() {
    // 工具面是编译期常量，与任何快照/principal 无关：直接断言它不因授权世界而变。
    let empty = PolicySnapshot::default(); // 空快照 = deny-everything 世界
    let full = allow_snapshot(principal(42), Capability::Query);
    // 工具面不接受快照入参——它恒为同一固定集合（这正是 F-4 的承诺）。
    let _ = (&empty, &full);
    assert_eq!(
        postern_daemon::shells::mcp::tools(),
        &MCP_TOOLS,
        "工具面与授权无关：满/空授权下均为同一固定八工具（F-4）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-5：postern_surface 只读快照投影，绝不 Adapter::discover、绝不触后端
// ════════════════════════════════════════════════════════════════════════════

// §8 F-5：postern_surface 返回当前快照里该 principal **已授权**对象的子集 —— 投影即
// snapshot.grants[principal] 的 (资源, 动词) 坐标，确定性、只含被授权坐标。
#[test]
fn surface_projects_authorized_subset_of_snapshot() {
    let p = principal(42);
    let snapshot = allow_snapshot(p, Capability::Query);
    // 触达 surface（RED 桩）→ panic → 观察到红。
    let projection = surface(&snapshot, p);
    assert_eq!(
        projection,
        vec![SurfaceEntry {
            resource: ResourceCode::new(RESOURCE),
            capability: Capability::Query,
        }],
        "surface 须恰为快照中该 principal 已授权的 (资源, 动词) 子集（F-5：授权能力投影）"
    );
}

// §8 F-5（fail-closed 对抗）：surface 投影是「已授权能力子集」—— 折叠为 deny 的 Escalate 格
// **绝不**入投影。给同一 principal 旁挂一个 Escalate 格（不同动词坐标），断言投影**恰**含 Allow
// 坐标、**不含** Escalate 坐标。这钉死 surface() 的 `action == Allow` 过滤：若该过滤被改弱/删除，
// Escalate 格会作为「已授权能力」泄漏进 surface（fail-open 信息披露），本断言据此变红。
#[test]
fn surface_excludes_escalate_cell_only_allow_projected() {
    let p = principal(42);
    // Query 格 Allow（应入投影），Mutate 格 Escalate（折叠为 deny，绝不入投影）。
    let snapshot = allow_plus_escalate_snapshot(p, Capability::Query, Capability::Mutate);
    let projection = surface(&snapshot, p);
    assert_eq!(
        projection,
        vec![SurfaceEntry {
            resource: ResourceCode::new(RESOURCE),
            capability: Capability::Query,
        }],
        "surface 须恰含 Allow 坐标、排除折叠为 deny 的 Escalate 坐标（F-5：投影是已授权能力子集，\
         Escalate 非已授权能力，绝不泄漏进投影）"
    );
    // 显式钉「Escalate 动词坐标不在投影里」—— 即便将来 Allow 集变化，这一排除不变量也独立成立。
    assert!(
        !projection
            .iter()
            .any(|e| e.capability == Capability::Mutate),
        "折叠为 deny 的 Escalate 格（Mutate）绝不出现在 surface 投影（F-5 fail-closed 过滤）"
    );
}

// §8 F-5：未授权 principal 的投影为空 —— 不泄露任何资源存在性（投影只来自自身授权世界）。
#[test]
fn surface_of_unauthorized_principal_is_empty() {
    let snapshot = allow_snapshot(principal(42), Capability::Query);
    // 触达 surface（RED 桩）→ panic → 观察到红。
    let projection = surface(&snapshot, principal(99));
    assert!(
        projection.is_empty(),
        "无授权 principal 的 surface 须为空（F-5：投影只来自自身授权世界，不泄露存在性）"
    );
}

// §8 F-5（取证）：经 DataPlane::surface 取投影时，Adapter::discover **绝不被调用**、绝不
// 建连/触后端 —— 投影纯读快照。借共享 discover 计数器观测其恒为 0。
#[test]
fn surface_makes_no_discover_call_no_backing_touch() {
    let (dp, _snapshot, discover_calls) = data_plane();
    // 触达 DataPlane::surface → surface（RED 桩）→ panic → 观察到红。
    let _ = dp.surface(principal(42));
    assert_eq!(
        *discover_calls.lock().expect("counter ok"),
        0,
        "postern_surface 绝不触达 Adapter::discover（F-5：投影纯读快照，不触后端资源）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-10 / L-4：协议语法非法 4xx 仍过同一 Sanitizer，出常量安全文案
// ════════════════════════════════════════════════════════════════════════════

// §8 F-10 / L-4：协议非法请求的安全出口字节须过同一 Sanitizer（哨兵前缀可观察），且其
// 文案恒为常量 INVALID_REQUEST_SAFE_MESSAGE（不回显请求字节、不泄露内部细节）。
#[test]
fn protocol_invalid_egress_passes_same_sanitizer_constant_message() {
    let sanitizer = SentinelSanitizer;
    // 触达 invalid_request_egress（RED 桩）→ panic → 观察到红。
    let out = shells::invalid_request_egress(&sanitizer);
    assert!(
        out.payload.starts_with(SCRUB_PREFIX),
        "协议非法 4xx 出口字节须过同一 Sanitizer（F-10 / L-4：非法出口不旁路裸传）"
    );
    // 哨兵前缀之后即常量安全文案（不回显请求、不泄露细节）。
    let body = &out.payload[SCRUB_PREFIX.len()..];
    assert_eq!(
        body,
        INVALID_REQUEST_SAFE_MESSAGE.as_bytes(),
        "协议非法 4xx 文案恒为常量安全文案（F-10 / L-4：constant safe message）"
    );
}

// §8 F-10 / L-4：协议非法出口与正常出口共用同一 Sanitizer 形态 —— 经 DataPlane 注入的脱敏器
// 取非法出口，其字节同样带哨兵前缀（两路出口同脱敏，非法不旁路）。
#[test]
fn data_plane_invalid_egress_is_sanitized() {
    let (dp, _s, _d) = data_plane();
    // 触达 DataPlane::invalid_request_egress → invalid_request_egress（RED 桩）→ panic → 红。
    let out = dp.invalid_request_egress();
    assert!(
        out.payload.starts_with(SCRUB_PREFIX),
        "DataPlane 的非法出口须过注入的同一 Sanitizer（F-10 / L-4）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-2 / B-2：数据面注入集（DataPlane）不含 PolicyRepo / vault 句柄
// ════════════════════════════════════════════════════════════════════════════

// §8 L-2 / B-2：DataPlane 的构造签名恰为 (Arc<Kernel>, Arc<PolicySnapshot>, Arc<dyn Sanitizer>)
// —— 无任何 PolicyRepo / vault / CredentialProvider 入参。经一个显式标注的桥接 fn 钉死形状：
// 若 DataPlane::new 多吃一个控制面写句柄或机密句柄，此 fn 编译失败（红线 7.2-2 编译期事实）。
#[allow(dead_code)]
fn data_plane_injection_set_excludes_policy_repo_and_vault(
    kernel: Arc<Kernel>,
    snapshot: Arc<PolicySnapshot>,
    sanitizer: Arc<dyn Sanitizer>,
) -> DataPlane {
    // 注入集恰为数据面三件套；PolicyRepo 写句柄与 vault/机密句柄绝不在此（L-2 / B-2）。
    DataPlane::new(kernel, snapshot, sanitizer)
}

// 显式引用上述桥接 fn 的类型形状，保证它编译且被实例化（避免 dead_code 让形状检查空转）。
#[test]
fn data_plane_new_signature_is_data_plane_only() {
    let _shape = data_plane_injection_set_excludes_policy_repo_and_vault
        as fn(Arc<Kernel>, Arc<PolicySnapshot>, Arc<dyn Sanitizer>) -> DataPlane;
    // 形状成立即证：注入集只含数据面三件套（L-2 / B-2）。无运行期断言；编译通过即此不变量成立。
    let _ = _shape;
}

// ════════════════════════════════════════════════════════════════════════════
//  §6.2 动词分类（纯函数）锚点：MCP 固定动词与审计动词类别一致
// ════════════════════════════════════════════════════════════════════════════

// §6.2 / F-4：MCP 固定动词工具里的能力动词（observe/mutate/execute/manage/destroy）与
// query 各自归入正确审计类别 —— 工具面动词与内核审计动词类别一致（read vs side-effecting）。
#[test]
fn mcp_verb_tools_map_to_audit_classes() {
    assert_eq!(AuditClass::of(Capability::Observe), AuditClass::Read);
    assert_eq!(AuditClass::of(Capability::Query), AuditClass::Read);
    assert_eq!(
        AuditClass::of(Capability::Mutate),
        AuditClass::SideEffecting
    );
    assert_eq!(
        AuditClass::of(Capability::Execute),
        AuditClass::SideEffecting
    );
    assert_eq!(
        AuditClass::of(Capability::Manage),
        AuditClass::SideEffecting
    );
    assert_eq!(
        AuditClass::of(Capability::Destroy),
        AuditClass::SideEffecting
    );
}
