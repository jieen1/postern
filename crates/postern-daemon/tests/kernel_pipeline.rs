//! kernel 管线单元行为测试（RED）。
//!
//! 钉死数据面请求内核 [0]→[10] 线性短路链与两阶段审计时序（模块文档 06 §3.2、§6.1
//! evaluate / §6.2 audit / §6.5 adapters、§8 F-3/F-10、L-3/L-4/L-5/L-12/L-13、CONS-8）。
//!
//! 驱动方式（06 §9 测试策略）：**内存 Fake 全插件注入** —— 纯内存 `PolicySnapshot` +
//! Fake `Authenticator` / `Adapter` / `ConditionPredicate` / `ConnAcquire`（建连缝）/
//! `AuditSink` / `Sanitizer`。每条只钉一个行为，断言「给定输入 → 管线调用序与决策/审计
//! 恰为某可观察结果」。失败路径一等公民：靠注入 Fake 失败触发各 fail-closed 分支再观察。
//!
//! 雷区纪律：本文件**零 SQL 标记**；需要 `ConnOrigin` 时以
//! `use postern_core::request::ConnOrigin as Origin` 别名构造（测试在 shells 外，绝不写
//! 字面 `ConnOrigin::` 变体）；**绝不构造** `ResolvedTarget`/`ResourceCredential`（建连缝
//! 直接交还不透明 `Channel`，Fake 永不触达机密类型）。异步用 `#[tokio::test]`。
//!
//! 实现为 RED 桩（`Kernel::new`/`submit`、`AuditPhase::*` 体为 `todo!()`），故构造内核 +
//! `submit` 即 panic → 观察到红。结构性断言（动词分类等纯函数）亦先于实现成立则单独标注。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use postern_core::decision::{Decision, DenyResponse};
use postern_core::domain::{
    Capability, ConditionSpec, ConstraintSpec, CredentialMeta, CredentialTier, CredentialView,
    EvalContext, GrantAction, GrantCell, PolicySnapshot, PresentedCredential, PrincipalId,
    ResourceCode, Role, TierDecl, Timestamp,
};
use postern_core::error::{
    AuditError, AuthError, ClassifyError, ConstraintError, DiscoverError, ExecError,
    PredicateError, Stage, TransportError,
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

use postern_daemon::error::OUTCOME_DOWNGRADED_CODE;
use postern_daemon::kernel::audit_phase::AuditClass;
use postern_daemon::kernel::pipeline::ConnAcquire;
use postern_daemon::kernel::Kernel;

// ════════════════════════════════════════════════════════════════════════════
//  可观察记录：管线调用序 / 审计事件序的共享探针
// ════════════════════════════════════════════════════════════════════════════

/// 管线各阶段被触达时按序追加的标记，供「调用序」断言比对（§8 单条 read 的
/// classify→check_constraint→evaluate→acquire→execute→scrub→record 序等）。
#[derive(Default)]
struct CallLog {
    events: Mutex<Vec<&'static str>>,
}

impl CallLog {
    fn record(&self, tag: &'static str) {
        self.events.lock().expect("call log not poisoned").push(tag);
    }

    fn snapshot(&self) -> Vec<&'static str> {
        self.events.lock().expect("call log not poisoned").clone()
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Fake 插件：全内存，按各自注入参数报成功/失败，并把触达记进共享 CallLog
// ════════════════════════════════════════════════════════════════════════════

/// 测试通用的 principal（雪花从原始构造；与快照 grants 键对齐时即「存在」）。
fn principal(raw: u64) -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(raw))
}

/// 固定墙钟（确定性：evaluate 只用入参 now，不读系统钟）。
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

/// 认证器：注入「成功→某 principal」或「失败→某 AuthError」。`kind()` 与请求出示物对齐。
struct FakeAuth {
    kind: &'static str,
    outcome: Result<PrincipalId, AuthError>,
    log: Arc<CallLog>,
}

impl Authenticator for FakeAuth {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn authenticate(
        &self,
        _presented: &PresentedCredential,
        _origin: &Origin,
        _creds: &CredentialView,
        _now: Timestamp,
    ) -> Result<PrincipalId, AuthError> {
        self.log.record("auth");
        self.outcome.clone()
    }
}

// ───────────────────────── Fake ConditionPredicate ─────────────────────────

/// 条件谓词：注入逐次求值结果（Ok(true)=过 / Ok(false)=不过 / Err=不可判定）。
struct FakePredicate {
    kind: &'static str,
    verdict: Result<bool, PredicateError>,
    log: Arc<CallLog>,
}

impl ConditionPredicate for FakePredicate {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn eval(&self, _ctx: &EvalContext, _spec: &serde_json::Value) -> Result<bool, PredicateError> {
        self.log.record("condition");
        self.verdict.clone()
    }
}

// ───────────────────────── Fake Adapter ─────────────────────────

/// 适配器：注入 classify / check_constraint / execute 三处的成功/失败，并把每次触达记入
/// CallLog（用于钉 [4] check_constraint 先于 evaluate、execute 在 acquire 之后等序）。
struct FakeAdapter {
    classify: Result<ClassifiedIntent, ClassifyError>,
    constraint: Result<bool, ConstraintError>,
    execute: Mutex<Option<Result<RawResponse, ExecError>>>,
    log: Arc<CallLog>,
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
        self.log.record("classify");
        self.classify.clone()
    }

    fn check_constraint(
        &self,
        _spec: &ConstraintSpec,
        _ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError> {
        self.log.record("check_constraint");
        self.constraint.clone()
    }

    async fn execute(&self, _ch: &mut Channel, _intent: &Intent) -> Result<RawResponse, ExecError> {
        self.log.record("execute");
        self.execute
            .lock()
            .expect("execute slot not poisoned")
            .take()
            .expect("execute should be exercised at most once per request")
    }

    async fn discover(&self, _ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError> {
        // 数据面 kernel 永不调用 discover（控制面路径）。
        todo!("discover is not exercised by the data-plane kernel unit")
    }
}

// ───────────────────────── Fake ConnAcquire（建连缝）─────────────────────────

/// 建连缝：注入「成功→不透明 Channel」或「失败→某 TransportError」。**绝不构造机密类型**：
/// 直接交还 Channel（其 handle 是任意 opaque 值）。记录 acquire 触达 + 命中的池键（tier）。
struct FakeAcquire {
    outcome: Mutex<Option<Result<(), TransportError>>>,
    seen_tier: Mutex<Option<CredentialTier>>,
    log: Arc<CallLog>,
}

impl FakeAcquire {
    fn ok(log: Arc<CallLog>) -> Self {
        Self {
            outcome: Mutex::new(Some(Ok(()))),
            seen_tier: Mutex::new(None),
            log,
        }
    }

    fn failing(err: TransportError, log: Arc<CallLog>) -> Self {
        Self {
            outcome: Mutex::new(Some(Err(err))),
            seen_tier: Mutex::new(None),
            log,
        }
    }
}

impl ConnAcquire for FakeAcquire {
    fn acquire<'a>(
        &'a self,
        _resource: &'a ResourceCode,
        tier: &'a CredentialTier,
    ) -> Pin<Box<dyn Future<Output = Result<Channel, TransportError>> + Send + 'a>> {
        self.log.record("acquire");
        *self.seen_tier.lock().expect("tier slot ok") = Some(tier.clone());
        let outcome = self
            .outcome
            .lock()
            .expect("acquire slot not poisoned")
            .take()
            .expect("acquire should be exercised at most once per request");
        Box::pin(async move {
            // 成功时交还一个不透明 Channel（handle 任意；绝不构造机密类型）。
            outcome.map(|()| Channel {
                handle: Box::new(()),
            })
        })
    }
}

// ───────────────────────── Fake AuditSink ─────────────────────────

/// 审计汇：把每条 record 的 (decision, stage) 摘要按序留痕；可注入「第 N 次写失败」以
/// 触发 read-fail / intent-fail / outcome-fail 三类两阶段分支。
struct FakeAudit {
    events: Mutex<Vec<(String, Option<Stage>)>>,
    fail_on_call: Option<usize>,
    fail_err: AuditError,
    calls: AtomicUsize,
    log: Arc<CallLog>,
}

impl FakeAudit {
    fn ok(log: Arc<CallLog>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            fail_on_call: None,
            fail_err: AuditError::WriteFailed,
            calls: AtomicUsize::new(0),
            log,
        }
    }

    /// 第 `nth` 次 record（1-based）返回 `err`，其余成功。
    fn fail_nth(nth: usize, err: AuditError, log: Arc<CallLog>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            fail_on_call: Some(nth),
            fail_err: err,
            calls: AtomicUsize::new(0),
            log,
        }
    }

    fn recorded(&self) -> Vec<(String, Option<Stage>)> {
        self.events.lock().expect("audit log ok").clone()
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AuditSink for FakeAudit {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.log.record("record");
        self.events
            .lock()
            .expect("audit log ok")
            .push((event.decision.clone(), event.stage));
        match self.fail_on_call {
            Some(target) if target == n => Err(self.fail_err.clone()),
            _ => Ok(()),
        }
    }
}

// ───────────────────────── Fake Sanitizer ─────────────────────────

/// 出口脱敏器：把触达记入 CallLog，原样返回（脱敏内容由 sanitize 单元另测；此处只钉
/// 「每条出口都过 sanitize」与「sanitize 在 record 序中的位置」）。
struct FakeSanitizer {
    log: Arc<CallLog>,
}

impl Sanitizer for FakeSanitizer {
    fn scrub(&self, payload: RawResponse, _declared: &[MaskRule]) -> SanitizedResponse {
        self.log.record("scrub");
        SanitizedResponse {
            payload: payload.payload,
        }
    }

    fn scrub_stream(&self, _declared: &[MaskRule]) -> Box<dyn StreamScrubber> {
        self.log.record("scrub_stream");
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
//  夹具组装：纯内存 PolicySnapshot + Evaluator + Kernel
// ════════════════════════════════════════════════════════════════════════════

const RESOURCE: &str = "db-main";
const AUTH_KIND: &str = "api_key";
const COND_KIND: &str = "always";

/// 装一个「会放行」的快照：principal `p` 在 (RESOURCE, capability) 有一个 Allow 格，
/// 该格挂一条 always 条件 + 一条 constraint spec；RESOURCE 的 tier 声明承载该动词。
fn allow_snapshot(p: PrincipalId, capability: Capability, tier: &str) -> PolicySnapshot {
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
        conditions: vec![ConditionSpec {
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
            tier: CredentialTier::new(tier),
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

/// 以单一 (kind→Authenticator) + (kind→ConditionPredicate) 注册表组装 Evaluator。
fn evaluator(auth: FakeAuth, pred: FakePredicate) -> Evaluator {
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(AUTH_KIND, Box::new(auth));
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert(COND_KIND, Box::new(pred));
    Evaluator::new(auths, preds)
}

/// 一个针对 RESOURCE / AUTH_KIND 的归一化请求（intent 内容不被 Fake 解释）。
fn request() -> NormalizedRequest {
    NormalizedRequest {
        presented: PresentedCredential::new(AUTH_KIND, b"secret".to_vec()),
        origin: unix_origin(),
        resource: ResourceCode::new(RESOURCE),
        intent: Intent::new(b"probe".to_vec()),
    }
}

/// 所有注入件 + 共享探针的一束（持有以便测试在 submit 后读回 CallLog / 审计记录）。
struct Harness {
    kernel: Kernel,
    log: Arc<CallLog>,
    audit: Arc<FakeAudit>,
    acquire: Arc<FakeAcquire>,
}

/// 组装一个完整内核：注入各 Fake 的成功/失败语义由调用方先构造好传入。
#[allow(clippy::too_many_arguments)]
fn harness(
    capability: Capability,
    tier: &str,
    auth: FakeAuth,
    pred: FakePredicate,
    adapter: FakeAdapter,
    acquire: FakeAcquire,
    audit: FakeAudit,
    sanitizer: FakeSanitizer,
    log: Arc<CallLog>,
) -> Harness {
    let p = principal(42);
    let snapshot = Arc::new(allow_snapshot(p, capability, tier));
    let eval = Arc::new(evaluator(auth, pred));
    let adapters = Arc::new(postern_daemon::registry::AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>,
    ]));
    let acquire = Arc::new(acquire);
    let audit = Arc::new(audit);
    let kernel = Kernel::new(
        eval,
        adapters,
        acquire.clone() as Arc<dyn ConnAcquire>,
        audit.clone() as Arc<dyn AuditSink>,
        Arc::new(sanitizer) as Arc<dyn Sanitizer>,
        snapshot,
        now(),
    );
    Harness {
        kernel,
        log,
        audit,
        acquire,
    }
}

/// 默认「全成功」工厂：principal=42 认证通过，always 条件过，classify→capability，
/// constraint 过，execute Ok，acquire Ok，audit Ok。调用方再覆写需要失败的那一件。
fn passing_harness(capability: Capability, exec_ok: bool) -> Harness {
    let log = Arc::new(CallLog::default());
    let p = principal(42);
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(p),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(capability)),
        constraint: Ok(true),
        execute: Mutex::new(Some(if exec_ok {
            Ok(RawResponse {
                payload: b"result-bytes".to_vec(),
            })
        } else {
            Err(ExecError::ExecutionFailed)
        })),
        log: log.clone(),
    };
    let acquire = FakeAcquire::ok(log.clone());
    let audit = FakeAudit::ok(log.clone());
    let sanitizer = FakeSanitizer { log: log.clone() };
    harness(
        capability, "readonly", auth, pred, adapter, acquire, audit, sanitizer, log,
    )
}

/// 取 deny 的 stage（断言 fail-closed 分支用）。
fn deny_stage_of(resp: &DenyResponse) -> Option<Stage> {
    // DenyResponse 自身不直接暴露 stage 字段（stage 走审计/trace）；从 reason 文案对齐
    // 不稳。改由审计记录读 stage（见各测试经 harness.audit）。此处仅断言它确是一个 deny。
    let _ = resp;
    None
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-10：submit 签名 + 出口统一脱敏
// ════════════════════════════════════════════════════════════════════════════

// §8 F-10：submit 签名恰为 submit(req: NormalizedRequest) -> Result<SanitizedResponse,
// DenyResponse>。经一个显式标注返回类型的桥接 fn 钉死形状：若 submit 的入/出参类型漂移
// （例如回 KernelOutcome / 非 Result / 多吃一个参数），此 fn 编译失败。请求来源已在
// req.origin 内（外壳 listener 装箱），submit 单参即足。
#[allow(dead_code)]
async fn submit_signature_bridge(
    k: &Kernel,
    req: NormalizedRequest,
) -> Result<SanitizedResponse, DenyResponse> {
    k.submit(req).await
}

// §8 F-10/L-4：正常放行 —— execute 成功后回 Ok(SanitizedResponse)，且出口经 sanitize。
#[tokio::test]
async fn passed_read_request_returns_sanitized_ok_through_egress() {
    let h = passing_harness(Capability::Query, true);
    let out = h.kernel.submit(request()).await;
    assert!(
        out.is_ok(),
        "认证/RBAC/细则/条件全过 + execute 成功 → 放行回 Ok(SanitizedResponse)"
    );
    // 出口统一脱敏：成功路径必触达 scrub。
    assert!(
        h.log.snapshot().contains(&"scrub"),
        "正常出口必经 Sanitizer::scrub（F-10：每字节过同一脱敏）"
    );
}

// §8 F-10/L-4：deny 出口也经同一 Sanitizer —— RBAC 缺格判拒，回 Err(DenyResponse)，
// 且 deny 文案在跨边界前过 sanitize（与正常/执行错共用同一出口脱敏）。
#[tokio::test]
async fn deny_egress_passes_the_same_sanitizer() {
    // principal=99 在快照里无任何 grant 格（快照只给 principal=42）→ RBAC 缺格 deny。
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(99)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::ok(log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "RBAC 缺格 → 回 Err(DenyResponse)");
    assert!(
        h.log.snapshot().contains(&"scrub"),
        "deny 出口也必经 Sanitizer::scrub（F-10/L-4：deny 与正常共用同一脱敏）"
    );
    // 缺格 deny 既不应触达 acquire 也不应触达 execute（短路在 rbac 阶）。
    assert!(
        !h.log.snapshot().contains(&"execute"),
        "RBAC 缺格 deny 绝不到达 execute[8]（L-5：六个 fail-closed 分支无一到达 execute）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-3 / §6.5：read 动词的可观察调用序 + check_constraint 严格先于 evaluate
// ════════════════════════════════════════════════════════════════════════════

// §8 F-3：一条 passed read(query) 请求产生可观察调用序
//   classify → check_constraint → evaluate → acquire → execute → scrub → record（单条）。
// 关键不变量同时被钉：check_constraint 在 evaluate 之前（F-3 / CONS-8），且 read 动词只有
// **一条** record（在 execute 之后），无意图痕。
#[tokio::test]
async fn passed_read_yields_canonical_call_order_single_record() {
    let h = passing_harness(Capability::Query, true);
    let out = h.kernel.submit(request()).await;
    assert!(out.is_ok(), "read 全过应放行");

    let seq = h.log.snapshot();
    // 投影出我们钉序的关键标记（auth/condition 由 evaluate 内部触发，归并到 evaluate 锚点）。
    let key: Vec<&str> = seq
        .iter()
        .copied()
        .filter(|t| {
            matches!(
                *t,
                "classify" | "check_constraint" | "acquire" | "execute" | "scrub" | "record"
            )
        })
        .collect();
    assert_eq!(
        key,
        vec![
            "classify",
            "check_constraint",
            "acquire",
            "execute",
            "scrub",
            "record"
        ],
        "passed read 的可观察调用序须为 classify→check_constraint→…→acquire→execute→scrub→record"
    );

    // §8 F-3：check_constraint 严格先于 evaluate（evaluate 内部首调 auth；以 auth 为 evaluate 锚点）。
    let pos = |tag: &str| seq.iter().position(|t| *t == tag);
    assert!(
        pos("check_constraint") < pos("auth"),
        "check_constraint[4] 必须严格先于 evaluate（F-3 / CONS-8）"
    );

    // §8 F-3：read 动词恰一条 record（execute 之后），且全程无意图痕。
    assert_eq!(
        seq.iter().filter(|t| **t == "record").count(),
        1,
        "read 动词只产生单条 record（execute 之后），绝无意图痕（F-3）"
    );
    assert_eq!(h.audit.call_count(), 1, "read 动词审计恰一次写");
    let pos_exec = seq.iter().position(|t| *t == "execute");
    let pos_rec = seq.iter().position(|t| *t == "record");
    assert!(
        pos_exec < pos_rec,
        "read 动词的单条 record 必在 execute 之后（先做再留痕）"
    );
}

// §8 F-3 / CONS-8：check_constraint 的结果用于构造 ConstraintCheck 并按引用传入 evaluate ——
// constraint 注入 Ok(false) 时，evaluate 据此在 [4] 阶 deny（不到 acquire/execute）。
#[tokio::test]
async fn constraint_false_denies_at_constraint_stage_before_connect() {
    // 装一个 constraint=Ok(false) 的内核（passing_harness 默认 Ok(true)）。
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(42)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(false), // 细则不过
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::ok(log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "constraint 不过 → deny");
    // §8 L-5：constraint 阶 deny 绝不到达 execute。
    assert!(
        !h.log.snapshot().contains(&"acquire"),
        "constraint deny 绝不建连"
    );
    assert!(
        !h.log.snapshot().contains(&"execute"),
        "constraint deny 绝不到达 execute"
    );
    // 审计该 deny 的 stage 恰为 constraint。
    let recorded = h.audit.recorded();
    assert!(
        recorded.iter().any(|(_d, s)| *s == Some(Stage::Constraint)),
        "constraint deny 的审计 stage 必为 Stage::Constraint"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-5：六个 fail-closed 分支各带正确 Stage，无一到达 execute[8]
// ════════════════════════════════════════════════════════════════════════════

// §8 L-5（auth）：authenticate Err → Deny{stage=auth}，不到 execute。
#[tokio::test]
async fn auth_error_denies_at_stage_auth_no_execute() {
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Err(AuthError::InvalidCredential),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::ok(log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "auth 失败 → deny");
    assert!(
        !h.log.snapshot().contains(&"execute"),
        "auth deny 绝不到达 execute[8]（L-5）"
    );
    assert!(
        h.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Auth)),
        "auth deny 审计 stage 必为 Stage::Auth"
    );
}

// §8 L-5（classify）：ClassifyError → Deny{stage=classify}；classify 在最前，evaluate/连接均不触达。
#[tokio::test]
async fn classify_error_denies_at_stage_classify_no_execute() {
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(42)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Err(ClassifyError::Unclassifiable),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::ok(log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "classify 失败 → deny");
    let seq = h.log.snapshot();
    assert!(
        !seq.contains(&"check_constraint"),
        "classify deny 短路在 [2]，绝不进 check_constraint"
    );
    assert!(
        !seq.contains(&"execute"),
        "classify deny 绝不到达 execute[8]"
    );
    assert!(
        h.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Classify)),
        "classify deny 审计 stage 必为 Stage::Classify"
    );
}

// §8 L-5（rbac）：无 grant 格 → Deny{stage=rbac}，不到 execute。
#[tokio::test]
async fn no_grant_denies_at_stage_rbac_no_execute() {
    let log = Arc::new(CallLog::default());
    // principal=99 在快照（只给 42）里无格 → RBAC 缺格。
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(99)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::ok(log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "RBAC 缺格 → deny");
    assert!(
        !h.log.snapshot().contains(&"execute"),
        "rbac deny 绝不到达 execute"
    );
    assert!(
        h.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Rbac)),
        "rbac deny 审计 stage 必为 Stage::Rbac"
    );
}

// §8 L-5（condition）：谓词不过 → Deny{stage=condition}，不到 execute。
#[tokio::test]
async fn predicate_fail_denies_at_stage_condition_no_execute() {
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(42)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(false), // 条件不满足
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::ok(log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "条件不过 → deny");
    assert!(
        !h.log.snapshot().contains(&"execute"),
        "condition deny 绝不到达 execute"
    );
    assert!(
        h.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Condition)),
        "condition deny 审计 stage 必为 Stage::Condition"
    );
}

// §8 L-5（connect）：Allow{tier} 后 acquire 失败 → Deny{stage=connect/transport}，execute 不被调用。
#[tokio::test]
async fn acquire_fail_denies_at_stage_connect_no_execute() {
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(42)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::failing(TransportError::ConnectFailed, log.clone()),
        FakeAudit::ok(log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "建连失败 → deny");
    let seq = h.log.snapshot();
    assert!(seq.contains(&"acquire"), "Allow{{tier}} 后应尝试 acquire");
    assert!(
        !seq.contains(&"execute"),
        "acquire 失败后 execute[8] 绝不被调用（L-5：connect 分支不到 execute）"
    );
    assert!(
        h.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Transport)),
        "connect deny 审计 stage 必为 Stage::Transport（= connect 折叠）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-3：两阶段审计——意图痕在前、结果痕在后；已执行绝不返 deny
// ════════════════════════════════════════════════════════════════════════════

// §8 L-3 / F-3：side-effecting(mutate) 请求在 Adapter::execute **之前**发意图痕、之后发结果痕。
// 观察序：…→acquire→record(intent)→execute→record(outcome)，意图痕严格先于 execute。
#[tokio::test]
async fn side_effecting_emits_intent_before_execute_outcome_after() {
    let h = passing_harness(Capability::Mutate, true);
    let out = h.kernel.submit(request()).await;
    assert!(out.is_ok(), "mutate 全过应放行");

    let seq = h.log.snapshot();
    let records: Vec<usize> = seq
        .iter()
        .enumerate()
        .filter(|(_, t)| **t == "record")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        records.len(),
        2,
        "side-effecting 动词产生两条 record（意图痕 + 结果痕）"
    );
    let pos_exec = seq
        .iter()
        .position(|t| *t == "execute")
        .expect("execute 触达");
    assert!(
        records[0] < pos_exec,
        "意图痕[7a]必须在 Adapter::execute 之前（L-3 / F-3）"
    );
    assert!(
        records[1] > pos_exec,
        "结果痕[10]必须在 Adapter::execute 之后（L-3 / F-3）"
    );
    assert_eq!(h.audit.call_count(), 2, "side-effecting 两阶段共两次审计写");
}

// §8 L-3 第②分支：side-effecting 意图痕写失败 → deny 于 execute 之前，Adapter::execute 不被调用。
#[tokio::test]
async fn intent_write_fail_denies_pre_execute_execute_not_called() {
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(42)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Mutate)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: b"should-not-run".to_vec(),
        }))),
        log: log.clone(),
    };
    // 第 1 次 record（= 意图痕）写失败。
    let h = harness(
        Capability::Mutate,
        "readwrite",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::fail_nth(1, AuditError::WriteFailed, log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "意图痕写失败 → deny（pre-execute）");
    assert!(
        !h.log.snapshot().contains(&"execute"),
        "意图痕写失败后 Adapter::execute 绝不被调用（L-3 第②分支）"
    );
    assert_eq!(h.audit.call_count(), 1, "意图痕失败即短路，无结果痕");
}

// §8 L-3 第③分支：side-effecting 已 execute 后结果痕写失败 → 「executed but audit downgraded」
// 码，**绝不 deny**（已执行不变量）。即仍回 Ok(SanitizedResponse)，绝不回 Err(DenyResponse)。
#[tokio::test]
async fn outcome_write_fail_is_downgraded_never_deny_after_execute() {
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(42)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Mutate)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: b"applied".to_vec(),
        }))),
        log: log.clone(),
    };
    // 第 2 次 record（= 结果痕）写失败；第 1 次（意图痕）成功。
    let h = harness(
        Capability::Mutate,
        "readwrite",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::fail_nth(2, AuditError::WriteFailed, log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    // 已执行：绝不返 deny —— 仍是 Ok，不得是 Err(DenyResponse)。
    assert!(
        out.is_ok(),
        "已 execute 后结果痕失败必须返回成功(降级)，绝不返 deny（L-3 第③分支：已执行不变量）"
    );
    assert!(
        h.log.snapshot().contains(&"execute"),
        "结果痕降级路径里 execute 确已发生（已执行才谈降级）"
    );
    // 承重断言：降级必须**可观察**。返回的 Ok 响应必须携带可识别降级码
    // OUTCOME_DOWNGRADED_CODE（「executed but audit downgraded」），否则降级在内核边界
    // 完全不可见 = fail-open（一个把 outcome 失败静默当普通成功返回的回归仍会 PASS）。
    let degraded = out.expect("已执行不变量：必为 Ok");
    let body = String::from_utf8(degraded.payload.clone()).expect("降级信封为 UTF-8 JSON");
    assert!(
        body.contains(OUTCOME_DOWNGRADED_CODE),
        "outcome 写失败的成功响应必须携带可识别降级码 {OUTCOME_DOWNGRADED_CODE}（L-3③/F-10：\
         降级必须可观察，绝不与干净成功不可区分）；实测 body={body}"
    );

    // 回归护栏：同输入的**干净成功**（audit 全 Ok）出口必须与降级出口逐字节**可区分**。
    // 若实现回归为降级臂直接回干净 sanitized（与成功无别），此断言失败。
    let clean_log = Arc::new(CallLog::default());
    let clean = harness(
        Capability::Mutate,
        "readwrite",
        FakeAuth {
            kind: AUTH_KIND,
            outcome: Ok(principal(42)),
            log: clean_log.clone(),
        },
        FakePredicate {
            kind: COND_KIND,
            verdict: Ok(true),
            log: clean_log.clone(),
        },
        FakeAdapter {
            classify: Ok(FakeAdapter::classified(Capability::Mutate)),
            constraint: Ok(true),
            execute: Mutex::new(Some(Ok(RawResponse {
                payload: b"applied".to_vec(),
            }))),
            log: clean_log.clone(),
        },
        FakeAcquire::ok(clean_log.clone()),
        FakeAudit::ok(clean_log.clone()),
        FakeSanitizer {
            log: clean_log.clone(),
        },
        clean_log,
    );
    let clean_out = clean
        .kernel
        .submit(request())
        .await
        .expect("干净成功：必为 Ok");
    assert_ne!(
        degraded.payload, clean_out.payload,
        "降级出口必须与同输入的干净成功出口逐字节可区分（否则降级不可观察 = fail-open）"
    );
    assert_eq!(
        clean_out.payload,
        b"applied".to_vec(),
        "干净成功出口恰为脱敏后的执行结果字节（无降级信封包裹）"
    );
}

// §8 L-3（read-verb record fail）：只读动词的单条 record 写失败 → Deny{stage=audit}
// （不可留痕即不可放行）。已执行但 read 无副作用，按 deny 返回（区别于 side-effecting 的降级）。
#[tokio::test]
async fn read_verb_record_fail_denies_with_stage_audit() {
    let log = Arc::new(CallLog::default());
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(principal(42)),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Query)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: b"rows".to_vec(),
        }))),
        log: log.clone(),
    };
    // read 动词只有一条 record；令其（第 1 次）失败。
    let h = harness(
        Capability::Query,
        "readonly",
        auth,
        pred,
        adapter,
        FakeAcquire::ok(log.clone()),
        FakeAudit::fail_nth(1, AuditError::WriteFailed, log.clone()),
        FakeSanitizer { log: log.clone() },
        log.clone(),
    );
    let out = h.kernel.submit(request()).await;
    assert!(
        out.is_err(),
        "read 动词单条 record 写失败 → deny（L-3：不可留痕即不可放行）"
    );
    assert!(
        h.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Audit)),
        "read record-fail deny 的审计 stage 必为 Stage::Audit"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-12：Escalate（审批关闭）折叠为 deny-equivalent，kernel 内绝不挂起
// ════════════════════════════════════════════════════════════════════════════

// §8 L-12：命中 Escalate 格（审批关闭）→ 回 deny-equivalent（escalate_denied），绝不挂起、
// 绝不建连/执行。kernel 不持等待态。
#[tokio::test]
async fn escalate_with_approval_closed_folds_to_deny_never_suspends() {
    let log = Arc::new(CallLog::default());
    let p = principal(42);
    // 手装一个 action=Escalate 的快照（passing 工厂只造 Allow）。
    let resource = ResourceCode::new(RESOURCE);
    let cell = GrantCell {
        resource: resource.clone(),
        capability: Capability::Manage,
        role: Role::new("operator"),
        action: GrantAction::Escalate,
        constraints: Vec::new(),
        conditions: Vec::new(),
    };
    let mut per_principal = BTreeMap::new();
    per_principal.insert((resource.clone(), Capability::Manage), cell);
    let mut grants = BTreeMap::new();
    grants.insert(p, per_principal);
    let snapshot = Arc::new(PolicySnapshot {
        policy_rev: 7,
        grants,
        tiers: BTreeMap::new(),
        credentials: CredentialView {
            credentials: vec![CredentialMeta {
                principal: p,
                kind: AUTH_KIND.into(),
                secret_hash: "h".into(),
                expires_at: None,
                revoked_at: None,
            }],
        },
        deny_notes: BTreeMap::new(),
        grantable: BTreeMap::new(),
        modes: BTreeMap::new(),
    });
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(p),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Manage)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let eval = Arc::new(evaluator(auth, pred));
    let adapters = Arc::new(postern_daemon::registry::AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>,
    ]));
    let acquire = Arc::new(FakeAcquire::ok(log.clone()));
    let audit = Arc::new(FakeAudit::ok(log.clone()));
    let kernel = Kernel::new(
        eval,
        adapters,
        acquire as Arc<dyn ConnAcquire>,
        audit.clone() as Arc<dyn AuditSink>,
        Arc::new(FakeSanitizer { log: log.clone() }) as Arc<dyn Sanitizer>,
        snapshot,
        now(),
    );

    let out = kernel.submit(request()).await;
    assert!(
        out.is_err(),
        "Escalate（审批关闭）在 kernel 内折叠为 deny-equivalent（L-12：绝不挂起）"
    );
    assert!(
        !log.snapshot().contains(&"execute"),
        "escalate 折叠后绝不建连/执行"
    );
    // 审计该 deny 的 decision 词为 escalate_denied（区别于普通 deny）。
    assert!(
        audit
            .recorded()
            .iter()
            .any(|(d, _s)| d == "escalate_denied"),
        "escalate 折叠的审计 decision 词必为 escalate_denied（L-12）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-13：作用域有界 deny —— 越界但存在 vs 不存在，产出同一 DenyResponse(stage=rbac)
// ════════════════════════════════════════════════════════════════════════════

// §8 L-13：对「存在但越出 principal 作用域的资源」与「根本不存在的资源」两次请求，产出
// **不可区分**的 DenyResponse（均 stage=rbac）—— 不泄露资源存在性。
#[tokio::test]
async fn out_of_scope_and_nonexistent_yield_identical_rbac_deny() {
    // 两次请求都用 principal=99（快照只给 42），分别打 in-scope-existing 与 nonexistent 资源代号。
    // 返回 (DenyResponse 本体, 审计 (decision,stage) 序)：L-13 验收口径是**两次 DenyResponse 在
    // 存在性敏感字段上不可区分**（your_grants/reason/request_hint/denied.capability/denied.objects），
    // 故必须把 DenyResponse 本体一并捕获比对，绝不可只比对审计 stage 向量（那会放过这些字段泄露
    // 存在性）。denied.resource 例外：机械回显请求代号，零存在性泄露，见下方承重断言。
    async fn deny_for(resource_code: &str) -> (DenyResponse, Vec<(String, Option<Stage>)>) {
        let log = Arc::new(CallLog::default());
        let auth = FakeAuth {
            kind: AUTH_KIND,
            outcome: Ok(principal(99)),
            log: log.clone(),
        };
        let pred = FakePredicate {
            kind: COND_KIND,
            verdict: Ok(true),
            log: log.clone(),
        };
        let adapter = FakeAdapter {
            classify: Ok(FakeAdapter::classified(Capability::Query)),
            constraint: Ok(true),
            execute: Mutex::new(Some(Ok(RawResponse {
                payload: Vec::new(),
            }))),
            log: log.clone(),
        };
        let h = harness(
            Capability::Query,
            "readonly",
            auth,
            pred,
            adapter,
            FakeAcquire::ok(log.clone()),
            FakeAudit::ok(log.clone()),
            FakeSanitizer { log: log.clone() },
            log.clone(),
        );
        let req = NormalizedRequest {
            presented: PresentedCredential::new(AUTH_KIND, b"s".to_vec()),
            origin: unix_origin(),
            resource: ResourceCode::new(resource_code),
            intent: Intent::new(b"probe".to_vec()),
        };
        let out = h.kernel.submit(req).await;
        // SanitizedResponse 不实现 Debug，故用 match（而非 expect_err）取 DenyResponse 本体。
        let deny = match out {
            Err(deny) => deny,
            Ok(_) => panic!("越界/不存在均应 deny"),
        };
        (deny, h.audit.recorded())
    }

    // "db-main" 存在于快照（给 42）但越出 99 的作用域；"ghost-xyz" 根本不存在。
    let (existing_deny, existing) = deny_for(RESOURCE).await;
    let (nonexistent_deny, nonexistent) = deny_for("ghost-xyz").await;
    // 两者审计 stage 都为 rbac，且不可区分（L-13：越界与不存在同形）。
    assert!(
        existing.iter().any(|(_d, s)| *s == Some(Stage::Rbac)),
        "越界但存在 → stage=rbac"
    );
    assert!(
        nonexistent.iter().any(|(_d, s)| *s == Some(Stage::Rbac)),
        "不存在 → stage=rbac"
    );
    let stage_set = |v: &[(String, Option<Stage>)]| -> Vec<Option<Stage>> {
        v.iter().map(|(_d, s)| *s).collect()
    };
    assert_eq!(
        stage_set(&existing),
        stage_set(&nonexistent),
        "越界但存在 与 不存在 须产出不可区分的审计 stage（L-13：不泄露存在性）"
    );

    // 承重断言：L-13 验收口径是「越界但存在」与「根本不存在」对**同一探测代号**不可区分——
    // 即攻击者无法据 deny 响应区分「存在但越权」与「资源不存在」。其安全实质落在**存在性敏感
    // 字段**：your_grants / request_hint / reason / denied.capability / denied.objects。这些字段
    // 任一在两路出现差异即泄露资源存在性，故须逐一相等。
    //
    // 唯一例外是 denied.resource：core 文档钉死其语义为「Resource the request targeted」——它机械
    // 回显**攻击者自己输入的请求代号**（db-main / ghost-xyz），零存在性泄露（攻击者输入什么就
    // 回显什么，不查表、不泄露快照拓扑）。故两路 denied.resource **各自等于其请求代号**、并不相等，
    // 这正确而非缺陷；旧断言只因 kernel bug 抹空 denied.resource 才整体相等，是过强（口径错位）。
    assert_eq!(
        existing_deny.your_grants, nonexistent_deny.your_grants,
        "L-13：your_grants 两路须不可区分（不泄露存在性）"
    );
    assert_eq!(
        existing_deny.request_hint, nonexistent_deny.request_hint,
        "L-13：request_hint 两路须不可区分（均 None，不暗示可授性/存在性）"
    );
    assert_eq!(
        existing_deny.reason, nonexistent_deny.reason,
        "L-13：reason 两路须不可区分（不泄露存在性）"
    );
    assert_eq!(
        existing_deny.denied.capability, nonexistent_deny.denied.capability,
        "L-13：denied.capability 两路须不可区分（源自请求方意图，非快照查表）"
    );
    assert_eq!(
        existing_deny.denied.objects, nonexistent_deny.denied.objects,
        "L-13：denied.objects 两路须不可区分（源自请求方意图，非快照查表）"
    );
    // denied.resource 机械回显各自请求代号（攻击者自己的输入），不相等且正确。
    assert_eq!(
        existing_deny.denied.resource.as_str(),
        RESOURCE,
        "denied.resource 机械回显请求代号 db-main（攻击者输入，零存在性泄露）"
    );
    assert_eq!(
        nonexistent_deny.denied.resource.as_str(),
        "ghost-xyz",
        "denied.resource 机械回显请求代号 ghost-xyz（攻击者输入，零存在性泄露）"
    );
    // 并显式钉住 stage=rbac 的承载事实：denied.capability/objects 与 reason 均为 rbac 缺格
    // 形态，且 your_grants 只含 principal 自身授权世界（此处 principal=99 在快照无任何格 →
    // 自身世界为空），不含被探测资源的任何存在性痕迹。
    assert!(
        existing_deny.your_grants.is_empty(),
        "principal=99 自身授权世界为空：your_grants 不得含任何被探测资源（不泄露存在性）"
    );
    assert_eq!(
        existing_deny.decision, "deny",
        "L-13 两路均为普通 deny（非 escalate_denied）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  deny 归因（已认证、持授权格的 principal 越权）：kernel 出口须沿用求值器已正确归因的
//  DenyResponse —— your_grants 反映该 principal 真实授权世界、denied.resource = 请求代号
// ════════════════════════════════════════════════════════════════════════════

// 求值后 deny 的归因咬合：principal=42 持 (db-main, Query) 授权格，但请求 db-main 的 **Mutate**
// （越权）→ RBAC 缺格 deny。kernel 边界回出的 DenyResponse 必须是求值器已正确归因的那一个：
//   - denied.resource == 请求资源代号 db-main（**非空串**）；
//   - your_grants == 该 principal 自身授权世界，含 {db-main:[query]}；
//   - stage == rbac。
// 在未修复实现下，kernel 丢弃求值器的 DenyResponse、以 unattributed() 零 principal + 空代号
// 重组 → denied.resource 恒空串、your_grants 恒空 → 本测试红。修复后转绿（证明测试咬住缺陷）。
#[tokio::test]
async fn post_eval_deny_carries_evaluator_attribution_resource_and_grants() {
    let log = Arc::new(CallLog::default());
    let p = principal(42);
    // 快照给 principal=42 一个 (db-main, Query) 的 Allow 格（其真实授权世界）。
    let snapshot = Arc::new(allow_snapshot(p, Capability::Query, "readonly"));
    let auth = FakeAuth {
        kind: AUTH_KIND,
        outcome: Ok(p),
        log: log.clone(),
    };
    let pred = FakePredicate {
        kind: COND_KIND,
        verdict: Ok(true),
        log: log.clone(),
    };
    // 适配器把请求归类为 **Mutate**（越权：42 只持 query 格）→ (db-main, Mutate) 缺格。
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(Capability::Mutate)),
        constraint: Ok(true),
        execute: Mutex::new(Some(Ok(RawResponse {
            payload: Vec::new(),
        }))),
        log: log.clone(),
    };
    let eval = Arc::new(evaluator(auth, pred));
    let adapters = Arc::new(postern_daemon::registry::AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>,
    ]));
    let audit = Arc::new(FakeAudit::ok(log.clone()));
    let kernel = Kernel::new(
        eval,
        adapters,
        Arc::new(FakeAcquire::ok(log.clone())) as Arc<dyn ConnAcquire>,
        audit.clone() as Arc<dyn AuditSink>,
        Arc::new(FakeSanitizer { log: log.clone() }) as Arc<dyn Sanitizer>,
        snapshot,
        now(),
    );

    let out = kernel.submit(request()).await;
    let deny = match out {
        Err(d) => d,
        Ok(_) => panic!("越权 mutate（42 仅持 query 格）应 deny"),
    };

    // denied.resource 必为请求资源代号 db-main（机械回显请求代号，非空串）。
    assert_eq!(
        deny.denied.resource,
        ResourceCode::new(RESOURCE),
        "求值后 deny 的 denied.resource 须为请求资源代号 db-main（非空串）——\
         kernel 须沿用求值器已正确归因的 DenyResponse，绝不用空代号兜底重组"
    );
    // denied.capability 为被请求的真实动词 Mutate（归类已穿透，归因不丢动词）。
    assert_eq!(
        deny.denied.capability,
        Capability::Mutate,
        "求值后 deny 的归因动词须为被请求的 Mutate"
    );
    // your_grants 必反映 principal=42 真实授权世界，含 {db-main:[query]}。
    let caps = deny
        .your_grants
        .get(&ResourceCode::new(RESOURCE))
        .expect("your_grants 须含 principal 自身已授的 db-main 格（kernel 须沿用求值器归因）");
    assert_eq!(
        caps,
        &vec!["query".to_string()],
        "your_grants 须只列 principal=42 真实已授的 query 格（不泄露被探测 mutate）"
    );
    // stage=rbac（越权动词缺格拒），由审计读回。
    assert!(
        audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Rbac)),
        "越权 mutate deny 的审计 stage 必为 Stage::Rbac"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8（execute 失败）：执行错出口经同一 Sanitizer，回 Err(DenyResponse{stage=exec})
// ════════════════════════════════════════════════════════════════════════════

// §8 L-4/F-10：Adapter::execute 失败（read 动词，无副作用）→ Deny{stage=exec}，出口经 sanitize。
#[tokio::test]
async fn execute_error_denies_at_stage_exec_through_sanitizer() {
    let h = passing_harness(Capability::Query, false); // execute => Err(ExecutionFailed)
    let out = h.kernel.submit(request()).await;
    assert!(out.is_err(), "execute 失败（read 无副作用）→ deny");
    assert!(
        h.log.snapshot().contains(&"execute"),
        "execute 确已被调用（在 acquire 之后）"
    );
    assert!(
        h.log.snapshot().contains(&"scrub"),
        "执行错出口也经同一 Sanitizer（F-10：五类出口同脱敏）"
    );
    assert!(
        h.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Exec)),
        "execute 失败 deny 审计 stage 必为 Stage::Exec"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8（tier 不共享 / CONS-8 tier 传入建连）：Allow{tier} 选出的 tier 用作池键
// ════════════════════════════════════════════════════════════════════════════

// §8 / CONS-8：evaluate 回 Allow{tier} 后，kernel 用该 tier 作 acquire 池键 —— acquire 看到的
// tier 恰为快照声明的承载 tier（tier 不共享：不同 tier 走不同子池）。
#[tokio::test]
async fn allow_tier_is_used_as_pool_key_for_acquire() {
    let h = passing_harness(Capability::Query, true); // tier 声明为 "readonly"
    let out = h.kernel.submit(request()).await;
    assert!(out.is_ok(), "全过应放行");
    let seen = h
        .acquire
        .seen_tier
        .lock()
        .expect("tier slot ok")
        .clone()
        .expect("acquire 必被调用且记录其 tier 池键");
    assert_eq!(
        seen.as_str(),
        "readonly",
        "acquire 的 tier 池键必为 evaluate 选出的承载 tier（tier 不共享，CONS-8）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §6.2 动词分类（纯函数）：read vs side-effecting 边界——审计两阶段时序的根
// ════════════════════════════════════════════════════════════════════════════

// §6.2：AuditClass::of 把 observe/query 判为 Read，mutate/execute/manage/destroy 判为
// SideEffecting —— 这是两阶段时序的判别根（read 单痕、side-effecting 意图+结果双痕）。
#[test]
fn audit_class_partitions_read_vs_side_effecting_verbs() {
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

// ════════════════════════════════════════════════════════════════════════════
//  哑用：保留 Decision/deny_stage_of 命名被引用（避免 dead_code），并锚定决策类型形状
// ════════════════════════════════════════════════════════════════════════════

// 决策类型形状锚点：Allow 携带 grant+tier、Deny 携带结构化响应（kernel 据此分流出口）。
#[test]
fn decision_shape_anchor_allow_carries_grant_and_tier() {
    fn is_allow(d: &Decision) -> bool {
        matches!(d, Decision::Allow { .. })
    }
    fn is_deny(d: &Decision) -> bool {
        matches!(d, Decision::Deny(_))
    }
    // 仅用类型形状（不构造真实决策）：保证下列判别式编译且语义稳定。
    let _ = (
        is_allow as fn(&Decision) -> bool,
        is_deny as fn(&Decision) -> bool,
    );
    // 保留 deny_stage_of 命名引用（其实现注释说明 stage 由审计读，不从 DenyResponse 取）。
    let _ = deny_stage_of as fn(&DenyResponse) -> Option<Stage>;
}
