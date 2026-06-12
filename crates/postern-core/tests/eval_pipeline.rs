//! 求值器主流程（`Evaluator::evaluate`）的纯内存行为测试（RED）。
//!
//! 焦点是 §5.3 / 详细设计 4.1 的纯同步求值入口
//! `evaluate(req, ci, constraint, policy, now) -> (Decision, EvalTrace)`——
//! 短路串行管线 `[1]`(认证)→`[3]`(RBAC 查表)→`[4]`(读 `constraint.passed`)→
//! `[5]`(条件谓词)→`[6]`(动作分流 + 动词→tier 映射)。一切 `Err`/无法判定
//! fail-closed 为 `Deny`（公理二），escalate 折叠为 deny，无匹配 tier 即 deny。
//!
//! 测试以纯内存驱动：内存假 `Authenticator` / `ConditionPredicate`（写实、
//! 确定性、无 `todo!()`），纯内存 `PolicySnapshot`（grants/tiers/credentials
//! 等 `BTreeMap`），`NormalizedRequest`（`PresentedCredential::new` 在 core
//! 测试可构造）。零 IO、无库、无网。
//!
//! 每条只钉一个行为，测试名陈述行为，断言精确到 `Decision` 的具体变体与字段
//! （`Allow{grant,tier}` 的 grant 三元组与 tier、`Deny` 的 `denied` 与
//! `EvalTrace.final_stage()` 阶段）。`evaluate` 主体当前为 `todo!()`，故 RED
//! 阶段以 panic 失败；逻辑就位后逐条转绿。
//!
//! 文本纪律：本文件刻意不出现求值吞错字样（契约 `EVAL_NO_ERROR_SWALLOWING`
//! 文本雷区），亦不构造或拼写机密族类型字面（只按引用透传 `PresentedCredential`、
//! 用 `ResourceCode`/`Capability`/`CredentialTier` 等代号类型）。
//!
//! 注：求值器把 `req.origin` 整体透传给 `authenticate`，本文件不解构 `ConnOrigin`
//! 变体；需构造一个 origin 时经 `ConnOrigin as Origin` 别名（与已提交兄弟单元
//! `plugin_traits.rs` 同惯例），从而文本里绝不出现被禁的 `ConnOrigin::<变体>`
//! 双冒号拼写。

use std::collections::BTreeMap;

use postern_core::decision::Decision;
use postern_core::domain::{
    Capability, CredentialTier, EvalContext, GrantAction, GrantCell, PolicySnapshot, PrincipalId,
    ResourceCode, Role, TierDecl, Timestamp,
};
use postern_core::domain::{CredentialView, PresentedCredential};
use postern_core::error::{AuthError, PredicateError, Stage};
use postern_core::eval::evaluator::{ConstraintCheck, Evaluator};
use postern_core::id::SnowflakeId;
use postern_core::plugin::{Authenticator, ConditionPredicate};
use postern_core::request::ConnOrigin as Origin;
use postern_core::request::{ClassifiedIntent, Intent, NormalizedRequest, ObjectRef};

// ============================================================================
// 参考 Fake 插件（写实、确定性，无 todo!()）
// ============================================================================

/// 固定 principal（雪花 id 由原始值重建——确定性，不取系统时钟）。
fn principal() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(0x0007_0000_0000_0001))
}

/// 写实认证器：按构造时给定的结果，对任意出示物返回固定 `Ok(principal)` 或
/// 固定 `AuthError`。`kind` 决定注册表选型键。确定性、纯同步、无 IO。
struct FakeAuthenticator {
    kind: &'static str,
    result: Result<PrincipalId, AuthError>,
}

impl FakeAuthenticator {
    fn ok(kind: &'static str) -> Self {
        Self {
            kind,
            result: Ok(principal()),
        }
    }

    fn err(kind: &'static str, err: AuthError) -> Self {
        Self {
            kind,
            result: Err(err),
        }
    }
}

impl Authenticator for FakeAuthenticator {
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
        self.result.clone()
    }
}

/// 写实条件谓词：按构造时给定的固定结论对任意上下文返回 `Ok(true)`、
/// `Ok(false)` 或 `Err(PredicateError)`。`kind` 决定注册表选型键。
struct FakeCondition {
    kind: &'static str,
    verdict: Result<bool, PredicateError>,
}

impl FakeCondition {
    fn satisfied(kind: &'static str) -> Self {
        Self {
            kind,
            verdict: Ok(true),
        }
    }

    fn unsatisfied(kind: &'static str) -> Self {
        Self {
            kind,
            verdict: Ok(false),
        }
    }

    fn erroring(kind: &'static str) -> Self {
        Self {
            kind,
            verdict: Err(PredicateError::Undecidable),
        }
    }
}

impl ConditionPredicate for FakeCondition {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn eval(&self, _ctx: &EvalContext, _spec: &serde_json::Value) -> Result<bool, PredicateError> {
        self.verdict.clone()
    }
}

// ============================================================================
// 构造辅助
// ============================================================================

const PRESENTED_KIND: &str = "api_key";
const RESOURCE: &str = "db-main";
const TIER_RO: &str = "readonly";

fn resource() -> ResourceCode {
    ResourceCode::new(RESOURCE)
}

fn origin() -> Origin {
    // 经 `Origin` 别名构造，文本里不出现被禁的 `ConnOrigin::<变体>` 双冒号拼写。
    Origin::Tcp {
        remote: "127.0.0.1:5432".parse().expect("static socket addr"),
    }
}

/// 归一化请求：出示 `api_key` 类凭证，目标 `db-main`，意图按引用透传（不解释）。
fn request() -> NormalizedRequest {
    NormalizedRequest {
        presented: PresentedCredential::new(PRESENTED_KIND, b"opaque".to_vec()),
        origin: origin(),
        resource: resource(),
        intent: Intent::new(b"q-orders-1".to_vec()),
    }
}

/// 归类产物：动词 Query，单一对象引用（kernel 先行物化的 ci 入参）。
fn classified() -> ClassifiedIntent {
    ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![ObjectRef::new("table:orders")],
    }
}

/// 一格授权：放在 `(resource, capability)` 上，动作/条件可定制。
fn grant_cell(
    capability: Capability,
    action: GrantAction,
    conditions: Vec<postern_core::domain::ConditionSpec>,
) -> GrantCell {
    GrantCell {
        resource: resource(),
        capability,
        role: Role::new("reader"),
        action,
        constraints: vec![],
        conditions,
    }
}

/// 一条条件声明（kind + 任意 JSON spec，谓词不解释 spec 内容）。
fn condition_spec(kind: &str) -> postern_core::domain::ConditionSpec {
    postern_core::domain::ConditionSpec {
        kind: kind.to_string(),
        spec: "{}".to_string(),
    }
}

/// 一条 spec 原文为非法 JSON 的条件声明（kind 仍正常选型，spec 字段不可解析）。
/// 用于钉死求值器自身「spec 非法 JSON → 拒」这一 fail-closed 路径。
fn condition_spec_with_raw(kind: &str, raw_spec: &str) -> postern_core::domain::ConditionSpec {
    postern_core::domain::ConditionSpec {
        kind: kind.to_string(),
        spec: raw_spec.to_string(),
    }
}

/// 装一张快照：给 `principal()` 在 `(resource, capability)` 放一格 + 给 resource
/// 声明一个承载该动词的 tier。`conditions` 可附在命中格上。
fn snapshot_allow(
    capability: Capability,
    action: GrantAction,
    conditions: Vec<postern_core::domain::ConditionSpec>,
    tier_carries: Vec<Capability>,
) -> PolicySnapshot {
    let mut per_principal: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    per_principal.insert(
        (resource(), capability),
        grant_cell(capability, action, conditions),
    );
    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);

    let mut tiers = BTreeMap::new();
    tiers.insert(
        resource(),
        vec![TierDecl {
            tier: CredentialTier::new(TIER_RO),
            carries: tier_carries,
        }],
    );

    PolicySnapshot {
        policy_rev: 7,
        grants,
        tiers,
        ..PolicySnapshot::default()
    }
}

/// 组装一个求值器：可注入一个认证器与零或多个条件谓词。
fn evaluator(auth: FakeAuthenticator, conditions: Vec<FakeCondition>) -> Evaluator {
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(auth.kind(), Box::new(auth));

    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    for c in conditions {
        preds.insert(c.kind(), Box::new(c));
    }
    Evaluator::new(auths, preds)
}

/// 全绿求值器：认证器 ok，无条件谓词。
fn green_evaluator() -> Evaluator {
    evaluator(FakeAuthenticator::ok(PRESENTED_KIND), vec![])
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_767_225_600_000)
}

/// 取出 Deny 的 stage，方便断言短路阶段（非 Deny 直接 panic 暴露用例错配）。
fn deny_stage(decision: &Decision, trace_stage: Option<Stage>) -> Stage {
    match decision {
        Decision::Deny(_) => trace_stage.expect("deny trace must carry a final stage"),
        other => panic!("expected Deny, got {other:?}"),
    }
}

// ============================================================================
// F-3：签名一致、&self 不可变、now 入参（无内部可变状态）
// ============================================================================

// §8 F-3 evaluate 签名按 §5.3/4.1：(&self, &req, &ci, &constraint, &policy, now)
//         → (Decision, EvalTrace)；&self 不可变、now 为入参（编译即证签名一致）。
#[test]
fn evaluate_signature_matches_design_and_self_is_immutable() {
    let eval = green_evaluator();
    let req = request();
    let ci = classified();
    let constraint = ConstraintCheck { passed: true };
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![],
        vec![Capability::Query],
    );
    // &self 不可变：同一不可变借用可连续两次调用（无内部可变状态）。
    let (_d1, _t1) = eval.evaluate(&req, &ci, &constraint, &snap, now());
    let (_d2, _t2) = eval.evaluate(&req, &ci, &constraint, &snap, now());
}

// ============================================================================
// F-4：放行编排——命中 Allow 格 + 细则过 + 条件全满足 + tier 承载该动词
// ============================================================================

// §8 F-4 放行编排：命中授权格(action=Allow)、constraint.passed=true、条件全满足、
//         policy.tiers[resource] 有 TierDecl 承载 ci.capability → Allow{grant=正确
//         命中格的 MatchedGrant, tier=承载该动词的等级}。
#[test]
fn allows_with_matched_grant_and_carrying_tier_when_all_steps_pass() {
    let eval = green_evaluator();
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![],
        vec![Capability::Query],
    );
    let (decision, _trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    match decision {
        Decision::Allow { grant, tier } => {
            assert_eq!(grant.resource, resource(), "grant 资源为命中格资源");
            assert_eq!(
                grant.capability,
                Capability::Query,
                "grant 动词为命中格动词"
            );
            assert_eq!(
                grant.role,
                Role::new("reader"),
                "grant role 取自命中格 provenance"
            );
            assert_eq!(
                tier,
                CredentialTier::new(TIER_RO),
                "tier 为承载该动词的等级"
            );
        }
        other => panic!("expected Allow{{grant,tier}}, got {other:?}"),
    }
}

// §8 F-4 放行路径轨迹完整：逐步皆通过（auth→rbac→constraint→condition→tier），
//         末步落在 tier（选定等级），allow 轨迹同样完整。
#[test]
fn allow_trace_walks_every_step_and_ends_at_tier() {
    let eval = evaluator(
        FakeAuthenticator::ok(PRESENTED_KIND),
        vec![FakeCondition::satisfied("ttl")],
    );
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![condition_spec("ttl")],
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Allow { .. }),
        "全步通过 → Allow"
    );
    let stages: Vec<Stage> = trace.steps.iter().map(|s| s.stage).collect();
    assert_eq!(
        stages,
        vec![
            Stage::Auth,
            Stage::Rbac,
            Stage::Constraint,
            Stage::Condition,
            Stage::Tier,
        ],
        "allow 轨迹逐步皆通过，末步落在 tier 选择"
    );
}

// ============================================================================
// L-1：默认拒绝——无 (resource, capability) 格 → Deny，final_stage = Rbac
// ============================================================================

// §8 L-1 默认拒绝（公理一）：policy.grants 无 (resource,capability) 格 → Deny，
//         trace.final_stage() = Some(Rbac)，且非放行。
#[test]
fn denies_at_rbac_when_no_grant_cell_for_resource_capability() {
    // 快照给的是 Mutate 格，但请求归类为 Query → (resource, Query) 缺格。
    let snap = snapshot_allow(
        Capability::Mutate,
        GrantAction::Allow,
        vec![],
        vec![Capability::Mutate],
    );
    let (decision, trace) = green_evaluator().evaluate(
        &request(),
        &classified(), // Query
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "缺格 → Deny（非放行）"
    );
    assert_eq!(
        trace.final_stage(),
        Some(Stage::Rbac),
        "缺格短路阶段为 rbac（公理一）"
    );
}

// §8 L-1 完全空快照（deny-everything 世界）→ Deny at rbac（principal 无任何格）。
#[test]
fn denies_at_rbac_on_empty_snapshot() {
    let snap = PolicySnapshot::default();
    let (decision, trace) = green_evaluator().evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(
        deny_stage(&decision, trace.final_stage()),
        Stage::Rbac,
        "空快照无格 → rbac deny"
    );
}

// ============================================================================
// L-2：tier 选择——承载该动词 → Allow{tier}；无任一 tier 承载 → Deny stage=tier
// ============================================================================

// §8 L-2 tier 选择：有 TierDecl 承载该动词 → Allow{tier=该等级}。
#[test]
fn selects_carrying_tier_on_allow() {
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![],
        vec![Capability::Observe, Capability::Query], // 该 tier 承载 Query
    );
    let (decision, _trace) = green_evaluator().evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    match decision {
        Decision::Allow { tier, .. } => {
            assert_eq!(tier, CredentialTier::new(TIER_RO), "选承载该动词的 tier");
        }
        other => panic!("expected Allow, got {other:?}"),
    }
}

// §8 L-2 无任一 tier 承载该动词 → Deny，stage=tier（不退默认 tier、不 panic）。
#[test]
fn denies_at_tier_when_no_tier_carries_the_verb() {
    // 命中 Allow 格、细则过、无条件，但 tier 只承载 Observe，不承载请求的 Query。
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![],
        vec![Capability::Observe], // 不含 Query
    );
    let (decision, trace) = green_evaluator().evaluate(
        &request(),
        &classified(), // Query
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert!(
        matches!(decision, Decision::Deny(_)),
        "无 tier 承载 → Deny（不退默认 tier）"
    );
    assert_eq!(
        trace.final_stage(),
        Some(Stage::Tier),
        "无 tier 承载短路阶段为 tier"
    );
}

// §8 L-2 resource 在 policy.tiers 中根本无声明 → 同样 Deny stage=tier（缺声明=不承载）。
#[test]
fn denies_at_tier_when_resource_has_no_tier_decl_at_all() {
    let mut per_principal: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    per_principal.insert(
        (resource(), Capability::Query),
        grant_cell(Capability::Query, GrantAction::Allow, vec![]),
    );
    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);
    let snap = PolicySnapshot {
        policy_rev: 7,
        grants,
        // tiers 留空 → resource 无任何 TierDecl
        ..PolicySnapshot::default()
    };
    let (decision, trace) = green_evaluator().evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(
        deny_stage(&decision, trace.final_stage()),
        Stage::Tier,
        "resource 无 tier 声明 → tier deny"
    );
}

// ============================================================================
// L-3：一切 Err 即拒（fail-closed 核心，公理二）——三种 stage 各自正确
// ============================================================================

// §8 L-3 注入假 Authenticator::authenticate 返回 Err → Deny，stage=auth。
#[test]
fn denies_at_auth_when_authenticator_returns_err() {
    let eval = evaluator(
        FakeAuthenticator::err(PRESENTED_KIND, AuthError::ExpiredCredential),
        vec![],
    );
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![],
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert!(
        matches!(decision, Decision::Deny(_)),
        "认证 Err → Deny（不放行）"
    );
    assert_eq!(
        trace.final_stage(),
        Some(Stage::Auth),
        "认证 Err 短路阶段为 auth"
    );
}

// §8 L-3 出示物 kind 在认证器注册表无对应（无法判定来源）→ Deny stage=auth。
#[test]
fn denies_at_auth_when_no_authenticator_registered_for_presented_kind() {
    // 注册的是 "token" 认证器，但请求出示 "api_key" → 选型未命中。
    let eval = evaluator(FakeAuthenticator::ok("token"), vec![]);
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![],
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(
        deny_stage(&decision, trace.final_stage()),
        Stage::Auth,
        "无认证器选型 → auth deny（无法判定即拒）"
    );
}

// §8 L-3 传入 ConstraintCheck{passed:false} → Deny，stage=constraint。
#[test]
fn denies_at_constraint_when_constraint_check_failed() {
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![],
        vec![Capability::Query],
    );
    let (decision, trace) = green_evaluator().evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: false }, // kernel 物化为不通过
        &snap,
        now(),
    );
    assert!(matches!(decision, Decision::Deny(_)), "细则未过 → Deny");
    assert_eq!(
        trace.final_stage(),
        Some(Stage::Constraint),
        "细则未过短路阶段为 constraint"
    );
}

// §8 L-3 注入假 ConditionPredicate::eval 返回 Ok(false) → Deny，stage=condition。
#[test]
fn denies_at_condition_when_predicate_returns_false() {
    let eval = evaluator(
        FakeAuthenticator::ok(PRESENTED_KIND),
        vec![FakeCondition::unsatisfied("time_window")],
    );
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![condition_spec("time_window")],
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert!(matches!(decision, Decision::Deny(_)), "条件 false → Deny");
    assert_eq!(
        trace.final_stage(),
        Some(Stage::Condition),
        "条件 false 短路阶段为 condition"
    );
}

// §8 L-3 注入假 ConditionPredicate::eval 返回 Err（无法判定）→ Deny，stage=condition。
#[test]
fn denies_at_condition_when_predicate_returns_err() {
    let eval = evaluator(
        FakeAuthenticator::ok(PRESENTED_KIND),
        vec![FakeCondition::erroring("ttl")],
    );
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![condition_spec("ttl")],
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(
        deny_stage(&decision, trace.final_stage()),
        Stage::Condition,
        "条件 Err（无法判定）→ condition deny（公理二）"
    );
}

// §8 L-3 命中格附带的条件 kind 在谓词注册表无对应（未注册 = 无法判定）→
//         Deny stage=condition（不得静默跳过当作通过）。
#[test]
fn denies_at_condition_when_predicate_kind_unregistered() {
    // 格上声明 "rate_limit" 条件，但注册表里没有该 kind 的谓词。
    let eval = evaluator(FakeAuthenticator::ok(PRESENTED_KIND), vec![]);
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![condition_spec("rate_limit")],
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(
        deny_stage(&decision, trace.final_stage()),
        Stage::Condition,
        "未注册谓词 kind → condition deny（不静默放行）"
    );
}

// §8 L-3 命中格附带的条件 spec 原文不是合法 JSON（求值器自身 serde_json::from_str
//         失败）→ Deny stage=condition。此处谓词 kind 已注册且为「恒满足」(Ok(true))，
//         若求值器把非法 spec 静默当作 null/空规格塞给谓词，谓词会返回 Ok(true) 而
//         整条管线 Allow——本用例正是要把那条 fail-open 回归钉死：spec 无法解析即
//         「条件无法判定」，必须 fail-closed 为 condition deny（公理二），绝不放行。
#[test]
fn denies_at_condition_when_condition_spec_is_invalid_json() {
    // 谓词恒满足且已注册：唯一能让本格不放行的，只有求值器对非法 spec 的拒绝分支。
    let eval = evaluator(
        FakeAuthenticator::ok(PRESENTED_KIND),
        vec![FakeCondition::satisfied("rate_limit")],
    );
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        // spec 原文为截断的、不可解析的 JSON 片段（非 "{}"）。
        vec![condition_spec_with_raw("rate_limit", "{ this is not json")],
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    // 必须 Deny：若实现把非法 spec 当作可放行（fail-open），恒满足谓词会让此处变成
    // Allow——断言到此即捕获该回归。
    assert!(
        matches!(decision, Decision::Deny(_)),
        "条件 spec 非法 JSON → 必须 Deny（不得静默放行恒满足谓词）"
    );
    assert_eq!(
        deny_stage(&decision, trace.final_stage()),
        Stage::Condition,
        "条件 spec 无法解析 → condition deny（无法判定即拒，公理二）"
    );
}

// ============================================================================
// L-4：escalate 折叠——命中 action=Escalate 格 → Deny（取 fallback），不挂起
// ============================================================================

// §8 L-4 escalate 折叠：命中 action=Escalate 的格（其余步全通过）→ 返回 Deny
//         （审批关闭恒取 fallback 折叠为 deny），不挂起、不引入等待状态。
#[test]
fn escalate_cell_folds_to_deny_when_approval_closed() {
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Escalate, // 命中格动作为 escalate
        vec![],
        vec![Capability::Query],
    );
    let (decision, _trace) = green_evaluator().evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    // core 不挂起、不引入 Escalate 等待态：escalate 折叠为 Deny。
    assert!(
        matches!(decision, Decision::Deny(_)),
        "escalate 折叠为 Deny（不挂起、审批关恒 deny）"
    );
}

// ============================================================================
// L-3 / L-5 边界：拒绝响应只承载 denied 代号事实，不泄露 Scope 外资源代号
// ============================================================================

// §8 L-3/L-5 拒绝响应 denied 只装本请求自身代号事实（resource/capability/objects），
//            不夹带任何其他资源代号（不泄露存在性）。
#[test]
fn deny_response_carries_only_this_requests_facts() {
    let snap = snapshot_allow(
        Capability::Mutate, // 缺 Query 格 → rbac deny
        GrantAction::Allow,
        vec![],
        vec![Capability::Mutate],
    );
    let (decision, _trace) = green_evaluator().evaluate(
        &request(),
        &classified(), // Query
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    match decision {
        Decision::Deny(resp) => {
            assert_eq!(resp.decision, "deny", "decision 字段恒 deny");
            assert_eq!(resp.denied.resource, resource(), "denied 资源为本请求资源");
            assert_eq!(
                resp.denied.capability,
                Capability::Query,
                "denied 动词为本请求动词"
            );
            assert_eq!(
                resp.denied.objects,
                vec![ObjectRef::new("table:orders")],
                "denied 对象为本请求归类对象"
            );
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

// ============================================================================
// L-7：确定性——同输入多次调用，(Decision, EvalTrace) 完全相同
// ============================================================================

// §8 L-7 确定性：同一 (req,ci,constraint,policy,now) 多次调用 → (Decision,
//         EvalTrace) 完全相同（不读系统时钟、只用入参 now、不用随机）。
#[test]
fn evaluate_is_deterministic_across_repeated_calls_on_allow() {
    let eval = evaluator(
        FakeAuthenticator::ok(PRESENTED_KIND),
        vec![FakeCondition::satisfied("ttl")],
    );
    let snap = snapshot_allow(
        Capability::Query,
        GrantAction::Allow,
        vec![condition_spec("ttl")],
        vec![Capability::Query],
    );
    let (d1, t1) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    let (d2, t2) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(d1, d2, "同输入 → Decision 逐字相同");
    assert_eq!(t1, t2, "同输入 → EvalTrace 逐字相同");
}

// §8 L-7 确定性也覆盖 deny 路径：同一缺格请求多次 → Deny 与 trace 完全相同。
#[test]
fn evaluate_is_deterministic_across_repeated_calls_on_deny() {
    let snap = snapshot_allow(
        Capability::Mutate, // 缺 Query 格
        GrantAction::Allow,
        vec![],
        vec![Capability::Mutate],
    );
    let (d1, t1) = green_evaluator().evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    let (d2, t2) = green_evaluator().evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(d1, d2, "deny 路径同输入 → Decision 逐字相同");
    assert_eq!(t1, t2, "deny 路径同输入 → EvalTrace 逐字相同");
}

// ============================================================================
// 短路顺序：auth 在 rbac 之前——认证失败时 rbac 缺格不改变 auth 归因
// ============================================================================

// §8 F-4/L-3 短路顺序 [1]→[3]：认证 Err 时，即便 rbac 也会缺格，短路阶段仍恒为
//            auth（管线按 [1]→[3]→[4]→[5]→[6] 顺序，任一步拒即就地短路）。
#[test]
fn auth_failure_short_circuits_before_rbac() {
    // 既让认证失败，又让 rbac 缺格（snapshot 给 Mutate、请求 Query）；
    // 正确实现应在 auth 处短路，绝不前进到 rbac。
    let eval = evaluator(
        FakeAuthenticator::err(PRESENTED_KIND, AuthError::InvalidCredential),
        vec![],
    );
    let snap = snapshot_allow(
        Capability::Mutate,
        GrantAction::Allow,
        vec![],
        vec![Capability::Mutate],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &classified(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert_eq!(
        deny_stage(&decision, trace.final_stage()),
        Stage::Auth,
        "认证先于 rbac 短路：阶段恒为 auth"
    );
}
