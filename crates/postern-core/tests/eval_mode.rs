//! 求值器步骤[5] **模式谓词**（失控切断的安全特性）的纯内存行为测试。
//!
//! 焦点是 §5.3 / 技设第 643 行 + 377-380 的覆盖规则：有效模式 = 全局与资源级取
//! 最严（meet，严格度 `Freeze > Observe > Maintain > Normal`），请求动词不在有效
//! 模式放行集内即 Deny（fail-closed，落 `condition` 阶），即便已命中授权格、细则
//! 已过、无任何条件谓词。覆盖集：
//!   - Freeze：任何动词都 deny；
//!   - Observe：`observe`/`query` 放行，`mutate`/`execute`/`manage`/`destroy` deny；
//!   - Maintain：`mutate`/`execute` 放行，`manage`/`destroy` deny；
//!   - Normal：不受 mode 限制（按 RBAC）；
//!   - 全局 vs 资源级 meet：资源 Freeze 在全局 Normal 下 → deny；全局 Freeze →
//!     所有资源 deny；
//!   - 有授权格但被 mode 拦的 deny，stage = `condition`。
//!
//! 测试以纯内存驱动（同兄弟单元 `eval_pipeline.rs`）：内存假 `Authenticator`
//! （恒 ok）、纯内存 `PolicySnapshot`（grants/tiers/modes 等 `BTreeMap`）。零 IO、
//! 无库、无网。每条只钉一个行为，断言精确到 `Decision` 变体与 `final_stage()`。
//!
//! 文本纪律：本文件刻意不出现求值吞错字样（契约 `EVAL_NO_ERROR_SWALLOWING`
//! 文本雷区），不构造或拼写机密族类型字面（只按引用透传 `PresentedCredential`）。
//! origin 经 `ConnOrigin as Origin` 别名构造，文本里不出现 `ConnOrigin::<变体>`。

use std::collections::BTreeMap;

use postern_core::decision::Decision;
use postern_core::domain::{
    Capability, CredentialTier, GrantAction, GrantCell, Mode, PolicySnapshot, PrincipalId,
    ResourceCode, Role, TierDecl, Timestamp,
};
use postern_core::domain::{CredentialView, PresentedCredential};
use postern_core::error::{AuthError, Stage};
use postern_core::eval::evaluator::{ConstraintCheck, Evaluator};
use postern_core::id::SnowflakeId;
use postern_core::plugin::{Authenticator, ConditionPredicate};
use postern_core::request::ConnOrigin as Origin;
use postern_core::request::{ClassifiedIntent, Intent, NormalizedRequest, ObjectRef};

// ============================================================================
// 参考 Fake 认证器（恒 ok，写实、确定性，无 todo!()）
// ============================================================================

const PRESENTED_KIND: &str = "api_key";
const RESOURCE: &str = "db-main";
const TIER_ALL: &str = "engine";

fn principal() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(0x0007_0000_0000_0001))
}

fn resource() -> ResourceCode {
    ResourceCode::new(RESOURCE)
}

/// 一个**不同于** `db-main` 的资源代号，用于全局模式波及多资源的断言。
fn other_resource() -> ResourceCode {
    ResourceCode::new("cache-edge")
}

struct OkAuth;
impl Authenticator for OkAuth {
    fn kind(&self) -> &'static str {
        PRESENTED_KIND
    }
    fn authenticate(
        &self,
        _presented: &PresentedCredential,
        _origin: &Origin,
        _creds: &CredentialView,
        _now: Timestamp,
    ) -> Result<PrincipalId, AuthError> {
        Ok(principal())
    }
}

fn evaluator() -> Evaluator {
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(PRESENTED_KIND, Box::new(OkAuth));
    let preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    Evaluator::new(auths, preds)
}

fn origin() -> Origin {
    Origin::Tcp {
        remote: "127.0.0.1:5432".parse().expect("static socket addr"),
    }
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_767_225_600_000)
}

/// 归一化请求：出示 `api_key`，目标 `db-main`，归类动词由调用方在 `ci` 给定。
fn request() -> NormalizedRequest {
    NormalizedRequest {
        presented: PresentedCredential::new(PRESENTED_KIND, b"opaque".to_vec()),
        origin: origin(),
        resource: resource(),
        intent: Intent::new(b"q-1".to_vec()),
    }
}

/// 归类产物：调用方指定动词，单一对象引用。
fn classified(capability: Capability) -> ClassifiedIntent {
    ClassifiedIntent {
        capability,
        objects: vec![ObjectRef::new("table:orders")],
    }
}

/// 一格放行授权：放在 `(db-main, capability)` 上，无约束、无条件、action=Allow。
fn allow_cell(capability: Capability) -> GrantCell {
    GrantCell {
        resource: resource(),
        capability,
        role: Role::new("operator"),
        action: GrantAction::Allow,
        constraints: vec![],
        conditions: vec![],
    }
}

/// 装一张快照：给 `principal()` 在 `(db-main, capability)` 放一格放行 + 一个承载
/// **全部六动词**的 tier（确保非 mode 路径必放行，把唯一变量隔离为 mode）+ 由调用
/// 方给定的 `modes` 覆盖。
fn snapshot_with_modes(
    capability: Capability,
    modes: BTreeMap<Option<ResourceCode>, Mode>,
) -> PolicySnapshot {
    let mut per_principal: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    per_principal.insert((resource(), capability), allow_cell(capability));
    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);

    let mut tiers = BTreeMap::new();
    tiers.insert(
        resource(),
        vec![TierDecl {
            tier: CredentialTier::new(TIER_ALL),
            carries: vec![
                Capability::Observe,
                Capability::Query,
                Capability::Mutate,
                Capability::Execute,
                Capability::Manage,
                Capability::Destroy,
            ],
        }],
    );

    PolicySnapshot {
        policy_rev: 9,
        grants,
        tiers,
        modes,
        ..PolicySnapshot::default()
    }
}

/// 无任何 mode 覆盖（空 map = 各处 Normal）。
fn no_modes() -> BTreeMap<Option<ResourceCode>, Mode> {
    BTreeMap::new()
}

/// 单条全局模式覆盖（`modes[None] = mode`）。
fn global_mode(mode: Mode) -> BTreeMap<Option<ResourceCode>, Mode> {
    let mut m = BTreeMap::new();
    m.insert(None, mode);
    m
}

/// 单条资源级模式覆盖（`modes[Some(db-main)] = mode`）。
fn resource_mode(mode: Mode) -> BTreeMap<Option<ResourceCode>, Mode> {
    let mut m = BTreeMap::new();
    m.insert(Some(resource()), mode);
    m
}

/// 跑一次求值，返回 `(Decision, final_stage)`。
fn run(capability: Capability, modes: BTreeMap<Option<ResourceCode>, Mode>) -> (Decision, Option<Stage>) {
    let snap = snapshot_with_modes(capability, modes);
    let (decision, trace) = evaluator().evaluate(
        &request(),
        &classified(capability),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    let stage = trace.final_stage();
    (decision, stage)
}

/// 断言：在给定模式下，该动词被 mode 拦下 → Deny 且 stage = condition。
fn assert_mode_denies(capability: Capability, modes: BTreeMap<Option<ResourceCode>, Mode>) {
    let (decision, stage) = run(capability, modes);
    assert!(
        matches!(decision, Decision::Deny(_)),
        "capability {:?} must be denied under this mode (not allowed)",
        capability
    );
    assert_eq!(
        stage,
        Some(Stage::Condition),
        "mode-barred capability {:?} denies at the condition stage (step [5])",
        capability
    );
}

/// 断言：在给定模式下，该动词放行（mode 不拦）→ Allow（其余步全绿）。
fn assert_mode_allows(capability: Capability, modes: BTreeMap<Option<ResourceCode>, Mode>) {
    let (decision, _stage) = run(capability, modes);
    assert!(
        matches!(decision, Decision::Allow { .. }),
        "capability {:?} must pass under this mode (RBAC + tier already green)",
        capability
    );
}

// ============================================================================
// Freeze：任何动词都 deny（kill-switch 全拒）
// ============================================================================

#[test]
fn freeze_denies_observe() {
    assert_mode_denies(Capability::Observe, global_mode(Mode::Freeze));
}

#[test]
fn freeze_denies_query() {
    assert_mode_denies(Capability::Query, global_mode(Mode::Freeze));
}

#[test]
fn freeze_denies_mutate() {
    assert_mode_denies(Capability::Mutate, global_mode(Mode::Freeze));
}

#[test]
fn freeze_denies_execute() {
    assert_mode_denies(Capability::Execute, global_mode(Mode::Freeze));
}

#[test]
fn freeze_denies_manage() {
    assert_mode_denies(Capability::Manage, global_mode(Mode::Freeze));
}

#[test]
fn freeze_denies_destroy() {
    assert_mode_denies(Capability::Destroy, global_mode(Mode::Freeze));
}

// ============================================================================
// Observe：observe/query 放行；mutate/execute/manage/destroy deny
// ============================================================================

#[test]
fn observe_allows_observe() {
    assert_mode_allows(Capability::Observe, global_mode(Mode::Observe));
}

#[test]
fn observe_allows_query() {
    assert_mode_allows(Capability::Query, global_mode(Mode::Observe));
}

#[test]
fn observe_denies_mutate() {
    assert_mode_denies(Capability::Mutate, global_mode(Mode::Observe));
}

#[test]
fn observe_denies_execute() {
    assert_mode_denies(Capability::Execute, global_mode(Mode::Observe));
}

#[test]
fn observe_denies_manage() {
    assert_mode_denies(Capability::Manage, global_mode(Mode::Observe));
}

#[test]
fn observe_denies_destroy() {
    assert_mode_denies(Capability::Destroy, global_mode(Mode::Observe));
}

// ============================================================================
// Maintain：observe/query/mutate/execute 放行；manage/destroy deny
// ============================================================================

#[test]
fn maintain_allows_observe() {
    assert_mode_allows(Capability::Observe, global_mode(Mode::Maintain));
}

#[test]
fn maintain_allows_query() {
    assert_mode_allows(Capability::Query, global_mode(Mode::Maintain));
}

#[test]
fn maintain_allows_mutate() {
    assert_mode_allows(Capability::Mutate, global_mode(Mode::Maintain));
}

#[test]
fn maintain_allows_execute() {
    assert_mode_allows(Capability::Execute, global_mode(Mode::Maintain));
}

#[test]
fn maintain_denies_manage() {
    assert_mode_denies(Capability::Manage, global_mode(Mode::Maintain));
}

#[test]
fn maintain_denies_destroy() {
    assert_mode_denies(Capability::Destroy, global_mode(Mode::Maintain));
}

// ============================================================================
// Normal：不受 mode 限制（按 RBAC）——含 manage/destroy 在内全放行
// ============================================================================

#[test]
fn normal_does_not_restrict_destroy() {
    // 显式 Normal 全局覆盖与「无覆盖」等价：mode 不设限，destroy 仍按 RBAC 放行。
    assert_mode_allows(Capability::Destroy, global_mode(Mode::Normal));
}

#[test]
fn empty_modes_map_does_not_restrict_any_capability() {
    // 空 modes（Default 形态）：各处 Normal，manage 不被 mode 拦，按 RBAC 放行。
    assert_mode_allows(Capability::Manage, no_modes());
}

// ============================================================================
// 全局 vs 资源级 meet（取最严）
// ============================================================================

#[test]
fn resource_freeze_under_global_normal_denies_that_resource() {
    // 全局 Normal（不设限）+ 资源级 Freeze → 该资源有效模式 = Freeze → 任何动词 deny。
    let mut modes = global_mode(Mode::Normal);
    modes.insert(Some(resource()), Mode::Freeze);
    assert_mode_denies(Capability::Query, modes);
}

#[test]
fn resource_only_freeze_denies_even_without_global_entry() {
    // 仅资源级 Freeze（无全局条目，全局取默认 Normal）→ 该资源 meet 后为 Freeze → deny。
    assert_mode_denies(Capability::Query, resource_mode(Mode::Freeze));
}

#[test]
fn global_freeze_denies_a_resource_with_no_resource_level_override() {
    // 全局 Freeze、无资源级覆盖 → 该资源有效模式 = Freeze → deny（全局波及所有资源）。
    assert_mode_denies(Capability::Observe, global_mode(Mode::Freeze));
}

#[test]
fn global_freeze_bars_a_request_to_a_second_resource_too() {
    // 全局 Freeze 必须波及**每个**资源：构造请求落在 cache-edge（非 db-main），
    // 仍因全局 Freeze 而 deny——证明全局条目对无自有覆盖的资源同样生效。
    let cap = Capability::Query;
    let mut per_principal: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    per_principal.insert(
        (other_resource(), cap),
        GrantCell {
            resource: other_resource(),
            capability: cap,
            role: Role::new("operator"),
            action: GrantAction::Allow,
            constraints: vec![],
            conditions: vec![],
        },
    );
    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);
    let mut tiers = BTreeMap::new();
    tiers.insert(
        other_resource(),
        vec![TierDecl {
            tier: CredentialTier::new(TIER_ALL),
            carries: vec![Capability::Query],
        }],
    );
    let snap = PolicySnapshot {
        policy_rev: 9,
        grants,
        tiers,
        modes: global_mode(Mode::Freeze),
        ..PolicySnapshot::default()
    };
    let req = NormalizedRequest {
        presented: PresentedCredential::new(PRESENTED_KIND, b"opaque".to_vec()),
        origin: origin(),
        resource: other_resource(),
        intent: Intent::new(b"q-2".to_vec()),
    };
    let (decision, trace) = evaluator().evaluate(
        &req,
        &classified(cap),
        &ConstraintCheck { passed: true },
        &snap,
        now(),
    );
    assert!(
        matches!(decision, Decision::Deny(_)),
        "global Freeze bars a request to a second resource with no own override"
    );
    assert_eq!(
        trace.final_stage(),
        Some(Stage::Condition),
        "the global-Freeze deny lands at the condition stage (step [5])"
    );
}

#[test]
fn resource_observe_is_stricter_than_global_maintain_for_mutate() {
    // 全局 Maintain（放行 mutate）但资源级 Observe（只读）→ meet 取 Observe → mutate deny。
    let mut modes = global_mode(Mode::Maintain);
    modes.insert(Some(resource()), Mode::Observe);
    assert_mode_denies(Capability::Mutate, modes);
}

#[test]
fn global_observe_with_resource_normal_still_takes_observe() {
    // 全局 Observe + 资源级 Normal（更松）→ meet 取 Observe（最严）→ mutate deny，
    // 证明 meet 绝不取最松（loosest-wins 回归钉死）。
    let mut modes = global_mode(Mode::Observe);
    modes.insert(Some(resource()), Mode::Normal);
    assert_mode_denies(Capability::Mutate, modes);
}

// ============================================================================
// 有授权格但被 mode 拦：deny 的 stage 正确（落 condition，而非 rbac/tier）
// ============================================================================

#[test]
fn mode_deny_with_a_valid_grant_cell_lands_at_condition_not_rbac() {
    // 命中 Allow 授权格、tier 承载该动词、细则已过——唯一拦截来自 mode（Freeze）。
    // 短路阶段必须是 condition（步骤[5]），证明 mode 拦截发生在 rbac/constraint 之后、
    // tier 之前，且 deny 归因不被误记到 rbac 或 tier。
    let (decision, stage) = run(Capability::Mutate, global_mode(Mode::Freeze));
    assert!(matches!(decision, Decision::Deny(_)), "mode bars a granted cell");
    assert_eq!(
        stage,
        Some(Stage::Condition),
        "a grant-backed request barred by mode denies at condition (step [5]), not rbac/tier"
    );
}

// ============================================================================
// Mode::allows / Mode::meet 直接单元（覆盖表与 meet 半格律的直断言）
// ============================================================================

#[test]
fn mode_allows_table_is_exactly_the_design_coverage() {
    // Normal：六动词全放行。
    for cap in [
        Capability::Observe,
        Capability::Query,
        Capability::Mutate,
        Capability::Execute,
        Capability::Manage,
        Capability::Destroy,
    ] {
        assert!(Mode::Normal.allows(cap), "Normal admits {:?} (RBAC governs)", cap);
        assert!(!Mode::Freeze.allows(cap), "Freeze admits nothing, not {:?}", cap);
    }
    // Observe：仅 observe/query。
    assert!(Mode::Observe.allows(Capability::Observe));
    assert!(Mode::Observe.allows(Capability::Query));
    assert!(!Mode::Observe.allows(Capability::Mutate));
    assert!(!Mode::Observe.allows(Capability::Execute));
    assert!(!Mode::Observe.allows(Capability::Manage));
    assert!(!Mode::Observe.allows(Capability::Destroy));
    // Maintain：observe/query/mutate/execute，不含 manage/destroy。
    assert!(Mode::Maintain.allows(Capability::Observe));
    assert!(Mode::Maintain.allows(Capability::Query));
    assert!(Mode::Maintain.allows(Capability::Mutate));
    assert!(Mode::Maintain.allows(Capability::Execute));
    assert!(!Mode::Maintain.allows(Capability::Manage));
    assert!(!Mode::Maintain.allows(Capability::Destroy));
}

#[test]
fn mode_meet_takes_the_stricter_and_is_commutative() {
    // 严格度 Freeze > Observe > Maintain > Normal：meet 取严。
    assert_eq!(Mode::Normal.meet(Mode::Freeze), Mode::Freeze);
    assert_eq!(Mode::Freeze.meet(Mode::Normal), Mode::Freeze, "meet 交换律");
    assert_eq!(Mode::Maintain.meet(Mode::Observe), Mode::Observe);
    assert_eq!(Mode::Observe.meet(Mode::Maintain), Mode::Observe, "meet 交换律");
    assert_eq!(Mode::Normal.meet(Mode::Maintain), Mode::Maintain);
    // 幂等。
    assert_eq!(Mode::Observe.meet(Mode::Observe), Mode::Observe);
}
