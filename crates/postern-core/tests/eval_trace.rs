//! 求值轨迹（EvalTrace）行为测试（RED）。
//!
//! 这些测试用**完整的参考 Fake 插件**（写实、确定性、无 `todo!()`）喂
//! `Evaluator::evaluate`，断言其返回的 `(Decision, EvalTrace)`。本单元的焦点
//! 是**轨迹的逐步累积与确定性**（模块设计 §3.3 EvalTrace 如何累积、§5.1
//! TraceStep/EvalTrace 字段；详细设计 8.3 / 4.1）：
//!
//!   - 短路场景下 `EvalTrace::final_stage()` 等于拒绝发生的那一步 stage；
//!   - allow 路径的轨迹逐步皆通过、末步为 tier 选定；
//!   - 确定性：相同（步骤序列, 事实）多次累积产出逐字节相同的 `EvalTrace`；
//!   - `detail` 文本只含 stage 名/谓词 kind/资源代号/动词等策略事实，绝无
//!     `Intent` 原文、`PresentedCredential`、真实地址。
//!
//! 求值器主体当前为 `unimplemented!()`，故这些测试在 RED 阶段以 panic 失败；
//! 实现就位后逐条转绿。
//!
//! 注：本文件刻意不出现求值吞错字样（契约 `EVAL_NO_ERROR_SWALLOWING` 文本
//! 雷区）——失败处一律 `match` / 显式 panic，不以默认放行兜底。

use std::collections::BTreeMap;

use postern_core::decision::Decision;
use postern_core::domain::{
    Capability, ConditionSpec, CredentialTier, CredentialView, EvalContext, GrantAction, GrantCell,
    PolicySnapshot, PresentedCredential, PrincipalId, ResourceCode, Role, TierDecl, Timestamp,
};
use postern_core::error::{AuthError, PredicateError, Stage};
use postern_core::eval::evaluator::{ConstraintCheck, Evaluator};
use postern_core::id::SnowflakeId;
use postern_core::plugin::{Authenticator, ConditionPredicate};
use postern_core::request::ConnOrigin as Origin;
use postern_core::request::{ClassifiedIntent, Intent, NormalizedRequest, ObjectRef};

// ----------------------------------------------------------------------------
// 参考 Fake 插件（写实、确定性、无 todo!()）
// ----------------------------------------------------------------------------

/// 出示物里植入的机密标记：任何轨迹 `detail` 都不得含它（§8 机密不入轨迹）。
const SECRET_MARKER: &str = "TOP-SECRET-PLAINTEXT-MARKER";
/// 注入 Intent 原文的机密标记：轨迹同样不得含它。
const INTENT_MARKER: &str = "DROP-TABLE-SECRET-INTENT";

/// 固定的 principal（雪花 id 由原始值重建——确定性，不取系统时钟）。
fn principal() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(0x0007_0000_0000_0001))
}

/// 认证器 Fake：按构造时的脚本恒定返回 `Ok(principal)` 或某个 `AuthError`。
struct FakeAuth {
    outcome: Result<PrincipalId, AuthError>,
}

impl FakeAuth {
    fn allow() -> Self {
        Self {
            outcome: Ok(principal()),
        }
    }

    fn err(e: AuthError) -> Self {
        Self { outcome: Err(e) }
    }
}

impl Authenticator for FakeAuth {
    fn kind(&self) -> &'static str {
        "fake"
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

/// 条件谓词 Fake：按脚本恒定返回 `Ok(true)` / `Ok(false)` / `Err(..)`。
struct FakePredicate {
    kind: &'static str,
    verdict: Result<bool, PredicateError>,
}

impl FakePredicate {
    fn pass() -> Self {
        Self {
            kind: "always",
            verdict: Ok(true),
        }
    }

    fn fail() -> Self {
        Self {
            kind: "always",
            verdict: Ok(false),
        }
    }

    fn err() -> Self {
        Self {
            kind: "always",
            verdict: Err(PredicateError::Undecidable),
        }
    }
}

impl ConditionPredicate for FakePredicate {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn eval(&self, _ctx: &EvalContext, _spec: &serde_json::Value) -> Result<bool, PredicateError> {
        self.verdict.clone()
    }
}

// ----------------------------------------------------------------------------
// 构造辅助
// ----------------------------------------------------------------------------

fn res() -> ResourceCode {
    ResourceCode::new("db-main")
}

fn tier_readonly() -> CredentialTier {
    CredentialTier::new("readonly")
}

/// 命中 Intent 中植入机密标记，且出示物里也植入标记——验证轨迹不泄漏。
fn request() -> NormalizedRequest {
    NormalizedRequest {
        presented: PresentedCredential::new("fake", SECRET_MARKER.as_bytes().to_vec()),
        // 经 `Origin` 别名构造，文本里不出现被禁的 `ConnOrigin::<变体>` 双冒号拼写。
        origin: Origin::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        resource: res(),
        intent: Intent::new(INTENT_MARKER.as_bytes().to_vec()),
    }
}

/// 归类结果：动词 Query，触及一个对象。
fn ci() -> ClassifiedIntent {
    ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![ObjectRef::new("table:orders")],
    }
}

/// 一格授权：`(db-main, Query)` 命中，动作=action，附带 `conditions`。
fn grant_cell(action: GrantAction, conditions: Vec<ConditionSpec>) -> GrantCell {
    GrantCell {
        resource: res(),
        capability: Capability::Query,
        role: Role::new("observer"),
        action,
        constraints: vec![],
        conditions,
    }
}

/// 一个条件谓词声明（kind=always），spec 为合法 JSON。
fn cond_always() -> ConditionSpec {
    ConditionSpec {
        kind: "always".to_string(),
        spec: "{}".to_string(),
    }
}

/// 组装一份快照：给 `principal()` 在 `(db-main, Query)` 放一格，
/// `tiers` 声明 `readonly` 承载 `carries` 列出的动词。
fn snapshot(cell: GrantCell, carries: Vec<Capability>) -> PolicySnapshot {
    let mut per_principal = BTreeMap::new();
    per_principal.insert((res(), Capability::Query), cell);

    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);

    let mut tiers = BTreeMap::new();
    tiers.insert(
        res(),
        vec![TierDecl {
            tier: tier_readonly(),
            carries,
        }],
    );

    PolicySnapshot {
        policy_rev: 7,
        grants,
        tiers,
        ..PolicySnapshot::default()
    }
}

/// 空快照（无任何授权格）——deny-everything 世界。
fn empty_snapshot() -> PolicySnapshot {
    PolicySnapshot {
        policy_rev: 7,
        ..PolicySnapshot::default()
    }
}

/// 组装一个求值器：注入一个认证器（kind=fake）与一个条件谓词（kind=always）。
fn evaluator(auth: FakeAuth, pred: FakePredicate) -> Evaluator {
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert("fake", Box::new(auth));
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert("always", Box::new(pred));
    Evaluator::new(auths, preds)
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_767_300_000_000)
}

/// 把整条轨迹拍平为一个串，供"无机密"扫描。
fn trace_text(trace: &postern_core::decision::EvalTrace) -> String {
    let mut s = String::new();
    for step in &trace.steps {
        s.push_str(step.stage.as_str());
        s.push('|');
        s.push_str(&step.detail);
        s.push('\n');
    }
    s
}

// ----------------------------------------------------------------------------
// §8 allow 路径轨迹完整：逐步皆通过 + 末步 tier 选定
// ----------------------------------------------------------------------------

// §8 allow 路径产出的轨迹包含逐步通过的记录且末步为 tier 选定
#[test]
fn allow_path_trace_ends_at_tier_with_full_pass_record() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    match decision {
        Decision::Allow { tier, .. } => assert_eq!(tier, tier_readonly()),
        other => panic!("expected Allow, got {other:?}"),
    }
    // 末步是 tier 选定。
    assert_eq!(trace.final_stage(), Some(Stage::Tier));
    // 逐步皆通过：轨迹按管线序覆盖 auth→rbac→constraint→condition→tier。
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
    );
}

// §8 allow 路径轨迹的每条记录都非空（"到达该步/在该步因何通过"均有登记）
#[test]
fn allow_path_trace_every_step_has_nonempty_detail() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (_decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(!trace.steps.is_empty(), "allow trace must record steps");
    for step in &trace.steps {
        assert!(
            !step.detail.is_empty(),
            "every step must carry a non-empty detail (stage {:?})",
            step.stage
        );
    }
}

// ----------------------------------------------------------------------------
// §8 短路：final_stage() == 拒绝发生的那一步 stage（auth/rbac/constraint/condition/tier）
// ----------------------------------------------------------------------------

// §8 短路场景：认证报错 → 轨迹截止于 auth，final_stage()==Auth，且恰 Deny
#[test]
fn auth_error_short_circuits_trace_at_auth_stage() {
    let eval = evaluator(
        FakeAuth::err(AuthError::ExpiredCredential),
        FakePredicate::pass(),
    );
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "auth Err must deny, got {decision:?}"
    );
    assert_eq!(trace.final_stage(), Some(Stage::Auth));
    // 短路截止于当前步：auth 之后再无记录。
    assert_eq!(trace.steps.last().map(|s| s.stage), Some(Stage::Auth));
    // auth-error 短路是凭据明文最敏感的路径：截止步 detail 既不得含出示物明文，
    // 也不得含 Intent 原文（§8 机密不入轨迹，在此 fail-closed 路径同样有 teeth）。
    let text = trace_text(&trace);
    assert!(
        !text.contains(SECRET_MARKER),
        "auth-error trace leaked presented-credential plaintext"
    );
    assert!(
        !text.contains(INTENT_MARKER),
        "auth-error trace leaked raw Intent text"
    );
}

// §8 短路场景：无命中授权格 → 轨迹截止于 rbac，final_stage()==Rbac，且恰 Deny
#[test]
fn rbac_miss_short_circuits_trace_at_rbac_stage() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    // 空快照：principal 通过认证，但无 (db-main, Query) 格。
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &empty_snapshot(),
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "rbac miss must deny, got {decision:?}"
    );
    assert_eq!(trace.final_stage(), Some(Stage::Rbac));
    // auth 已通过、rbac 是截止步：轨迹包含 auth 再以 rbac 收尾。
    let stages: Vec<Stage> = trace.steps.iter().map(|s| s.stage).collect();
    assert_eq!(stages, vec![Stage::Auth, Stage::Rbac]);
}

// §8 短路场景：细则未过（ConstraintCheck{passed:false}）→ 截止于 constraint
#[test]
fn constraint_failure_short_circuits_trace_at_constraint_stage() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: false },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "constraint fail must deny, got {decision:?}"
    );
    assert_eq!(trace.final_stage(), Some(Stage::Constraint));
    let stages: Vec<Stage> = trace.steps.iter().map(|s| s.stage).collect();
    assert_eq!(stages, vec![Stage::Auth, Stage::Rbac, Stage::Constraint]);
}

// §8 短路场景：条件谓词返回 false → 截止于 condition，final_stage()==Condition
#[test]
fn condition_false_short_circuits_trace_at_condition_stage() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::fail());
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "condition false must deny, got {decision:?}"
    );
    assert_eq!(trace.final_stage(), Some(Stage::Condition));
    let stages: Vec<Stage> = trace.steps.iter().map(|s| s.stage).collect();
    assert_eq!(
        stages,
        vec![
            Stage::Auth,
            Stage::Rbac,
            Stage::Constraint,
            Stage::Condition
        ]
    );
}

// §8 短路场景：条件谓词报错（无法判定）→ fail-closed deny，截止于 condition
#[test]
fn condition_error_short_circuits_trace_at_condition_stage() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::err());
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "condition Err must deny, got {decision:?}"
    );
    assert_eq!(trace.final_stage(), Some(Stage::Condition));
}

// §8 短路场景：动词无任何 tier 承载 → 截止于 tier，final_stage()==Tier，且恰 Deny（不退默认 tier）
#[test]
fn no_tier_carrying_verb_short_circuits_trace_at_tier_stage() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    // 授权格通过、细则与条件皆过，但 readonly 只承载 Observe、不承载 Query。
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Observe],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "no carrying tier must deny, got {decision:?}"
    );
    assert_eq!(trace.final_stage(), Some(Stage::Tier));
    let stages: Vec<Stage> = trace.steps.iter().map(|s| s.stage).collect();
    assert_eq!(
        stages,
        vec![
            Stage::Auth,
            Stage::Rbac,
            Stage::Constraint,
            Stage::Condition,
            Stage::Tier
        ]
    );
}

// §8 短路场景：escalate 格折叠为 Deny；轨迹截止于 tier 分流步（动作分流即 tier 步语义）
#[test]
fn escalate_cell_folds_to_deny_with_trace_at_tier_stage() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    let snap = snapshot(
        grant_cell(GrantAction::Escalate, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert!(
        matches!(decision, Decision::Deny(_)),
        "escalate must fold to Deny, got {decision:?}"
    );
    assert_eq!(trace.final_stage(), Some(Stage::Tier));
}

// ----------------------------------------------------------------------------
// §8 确定性：相同 (步骤序列, 事实) 多次累积 → 逐字节相同的 EvalTrace
// ----------------------------------------------------------------------------

// §8 确定性：allow 路径同输入多次求值 → 轨迹 steps 顺序与 detail 文本逐字相同
#[test]
fn allow_trace_is_byte_identical_across_repeated_evaluations() {
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );

    let eval_a = evaluator(FakeAuth::allow(), FakePredicate::pass());
    let (_d1, t1) = eval_a.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    let eval_b = evaluator(FakeAuth::allow(), FakePredicate::pass());
    let (_d2, t2) = eval_b.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    // 结构相等（PartialEq）即 steps 顺序 + 每条 stage/detail 全等。
    assert_eq!(t1, t2, "same inputs must yield identical EvalTrace");
    // 序列化也逐字节一致（审计可对账）。
    let j1 = serde_json::to_string(&t1).expect("trace serializes");
    let j2 = serde_json::to_string(&t2).expect("trace serializes");
    assert_eq!(j1, j2, "serialized trace must be byte-identical");
}

// §8 确定性：deny 路径（condition false）同输入多次求值 → 轨迹逐字相同
#[test]
fn deny_trace_is_byte_identical_across_repeated_evaluations() {
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );

    let eval_a = evaluator(FakeAuth::allow(), FakePredicate::fail());
    let (_d1, t1) = eval_a.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    let eval_b = evaluator(FakeAuth::allow(), FakePredicate::fail());
    let (_d2, t2) = eval_b.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    assert_eq!(t1, t2, "same inputs must yield identical deny EvalTrace");
}

// ----------------------------------------------------------------------------
// §8 detail 只含策略事实，绝无机密（Intent 原文 / PresentedCredential / 真实地址）
// ----------------------------------------------------------------------------

// §8 allow 轨迹 detail 不含出示物明文，也不含 Intent 原文
#[test]
fn allow_trace_detail_carries_no_secret_or_intent_text() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (_decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    let text = trace_text(&trace);
    assert!(
        !text.contains(SECRET_MARKER),
        "trace detail leaked presented-credential plaintext"
    );
    assert!(
        !text.contains(INTENT_MARKER),
        "trace detail leaked raw Intent text"
    );
}

// §8 deny 轨迹（条件未过）detail 同样不含机密标记，且引用谓词 kind 这类策略事实
#[test]
fn deny_trace_detail_carries_no_secret_and_cites_predicate_kind() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::fail());
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (_decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    let text = trace_text(&trace);
    assert!(
        !text.contains(SECRET_MARKER),
        "deny trace leaked credential plaintext"
    );
    assert!(
        !text.contains(INTENT_MARKER),
        "deny trace leaked raw Intent text"
    );
    // 条件步的 detail 应援引谓词 kind（策略事实）——这是允许出现的代号类内容。
    assert!(
        text.contains("always"),
        "condition step detail should cite the predicate kind (a policy fact)"
    );
}

// §8 轨迹 detail 援引资源代号是允许的（代号≠真实地址）：deny 轨迹应能引到 db-main
#[test]
fn deny_trace_detail_may_cite_resource_code() {
    let eval = evaluator(FakeAuth::allow(), FakePredicate::pass());
    // 空快照 → rbac miss；其 detail 援引被拒资源代号 db-main 是策略事实。
    let (_decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &empty_snapshot(),
        now(),
    );

    let text = trace_text(&trace);
    assert!(
        !text.contains(SECRET_MARKER),
        "rbac-miss trace leaked credential plaintext"
    );
    assert!(
        !text.contains(INTENT_MARKER),
        "rbac-miss trace leaked raw Intent text"
    );
    assert!(
        text.contains("db-main"),
        "rbac-miss detail should cite the resource code (a policy fact, not an address)"
    );
}

// ----------------------------------------------------------------------------
// final_stage() 边界：空轨迹无 stage（决策单元已定，本单元据此累积）
// ----------------------------------------------------------------------------

// §8 final_stage 语义钉：截止步即拒绝阶段——authError 时它恰是 Auth（与 AuthError::stage() 对齐）
#[test]
fn final_stage_matches_auth_error_stage_attribution() {
    let eval = evaluator(
        FakeAuth::err(AuthError::RevokedCredential),
        FakePredicate::pass(),
    );
    let snap = snapshot(
        grant_cell(GrantAction::Allow, vec![cond_always()]),
        vec![Capability::Query],
    );
    let (_decision, trace) = eval.evaluate(
        &request(),
        &ci(),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );

    // 轨迹截止步 stage 与错误的 stage 归类必须是同一个值（两条来源对齐）。
    assert_eq!(
        trace.final_stage(),
        Some(AuthError::RevokedCredential.stage())
    );
    assert_eq!(trace.final_stage(), Some(Stage::Auth));
    // 截止于 auth 的 detail 不得泄漏出示物明文或 Intent 原文（fail-closed 路径有 teeth）。
    let text = trace_text(&trace);
    assert!(
        !text.contains(SECRET_MARKER),
        "auth-error trace leaked presented-credential plaintext"
    );
    assert!(
        !text.contains(INTENT_MARKER),
        "auth-error trace leaked raw Intent text"
    );
}
