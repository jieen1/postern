//! 结构化拒绝响应（DenyResponse）事实组装的行为测试（RED）。
//!
//! 本单元的焦点是 `eval::deny::assemble` —— 从 `(PolicySnapshot, principal,
//! resource, capability, objects, reason 事实)` **机械组装** `DenyResponse` 的
//! 纯函数（模块设计 §3.3 DenyResponse 组装·为什么只取快照、§5.1 字段集、
//! §8 L-5/L-6；详细设计 8.3 范围内·DenyResponse 事实组装、必守不变量
//! `DENY_RESPONSE_SCOPE_BOUNDED`、8.0 速查表·your_grants 事实=策略引擎从快照
//! 导出）。
//!
//! 每条只钉一个行为，测试名陈述行为，断言精确到 `DenyResponse` 的具体字段：
//!
//!   - `decision` 恒 `"deny"`；`denied` 装入参 `resource/capability/objects`；
//!   - `your_grants` 只导出 `snapshot.grants[principal]` 内本 Principal 自身
//!     授权资源代号 → 能力名列表（`Capability::as_str`），`BTreeMap` 保证序；
//!     缺 principal → 空 `BTreeMap`（fail-closed：空集合，不放行、不报错）；
//!   - **L-5 不可区分**：对同一 principal，分别以"Scope 外但存在的资源代号"
//!     与"完全不存在的资源代号"组装两次 → 二者逐字节相同（防拓扑探测）；
//!   - **L-6 request_hint**：动词在 `snapshot.grantable[resource]` 中 → 机械生成
//!     `postern elevate` 命令；不在其中（含 resource 缺失）→ `None`/`null`；
//!   - **L-6 operator_note**：`snapshot.deny_notes[(res,cap)]` 存在 → 原样
//!     `Some`；缺省 → `None` 且经 serde_json 序列化时该字段不出现。
//!
//! `assemble` 主体当前为 `todo!()`，故这些测试在 RED 阶段以 panic 失败；逻辑
//! 就位后逐条转绿。
//!
//! 文本纪律：本文件刻意不出现求值吞错字样（契约 `EVAL_NO_ERROR_SWALLOWING`
//! 文本雷区），亦不构造或引用机密族类型（只用 ResourceCode/Capability/
//! ObjectRef 等代号类型）。

use std::collections::BTreeMap;

use postern_core::decision::DenyResponse;
use postern_core::domain::{
    Capability, GrantAction, GrantCell, PolicySnapshot, PrincipalId, ResourceCode, Role,
};
use postern_core::eval::deny::assemble;
use postern_core::id::SnowflakeId;
use postern_core::request::ObjectRef;

// ----------------------------------------------------------------------------
// 构造辅助（写实、确定性，无 todo!()）
// ----------------------------------------------------------------------------

/// 固定的 principal（雪花 id 由原始值重建——确定性，不取系统时钟）。
fn principal() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(0x0007_0000_0000_0001))
}

/// 另一个 principal——验证 your_grants 绝不枚举他人世界。
fn other_principal() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(0x0007_0000_0000_0002))
}

fn res_target() -> ResourceCode {
    ResourceCode::new("db-main")
}

/// Scope 外但**存在**的资源代号（principal 的授权世界里没有它，但快照别处有）。
fn res_existing_out_of_scope() -> ResourceCode {
    ResourceCode::new("db-secret")
}

/// 根本**不存在**的资源代号（快照任何角落都没有）。
fn res_nonexistent() -> ResourceCode {
    ResourceCode::new("db-ghost")
}

/// 一格授权：放在某 principal 的 `(resource, capability)` 上。
fn grant_cell(resource: ResourceCode, capability: Capability) -> GrantCell {
    GrantCell {
        resource,
        capability,
        role: Role::new("observer"),
        action: GrantAction::Allow,
        constraints: vec![],
        conditions: vec![],
    }
}

/// 给 `principal()` 在 `db-main` 上放 Observe + Query 两格（用于 your_grants 导出）。
fn snapshot_with_self_grants() -> PolicySnapshot {
    let mut per_principal: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    per_principal.insert(
        (res_target(), Capability::Observe),
        grant_cell(res_target(), Capability::Observe),
    );
    per_principal.insert(
        (res_target(), Capability::Query),
        grant_cell(res_target(), Capability::Query),
    );

    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);

    PolicySnapshot {
        policy_rev: 7,
        grants,
        ..PolicySnapshot::default()
    }
}

/// 默认 reason 文本（策略事实级，机械取轨迹截止步导出文本）。
fn reason() -> String {
    "rbac: no grant cell for db-main verb mutate".to_string()
}

fn objects() -> Vec<ObjectRef> {
    vec![ObjectRef::new("table:orders")]
}

// ----------------------------------------------------------------------------
// §8 decision 恒为 "deny" / denied 装入参事实
// ----------------------------------------------------------------------------

// §8 decision 字段恒为 "deny"（DenyResponse 不表达 allow）
#[test]
fn decision_field_is_always_deny() {
    let snap = PolicySnapshot::default();
    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );
    assert_eq!(resp.decision, "deny");
}

// §8 denied 装入参 resource/capability/objects（取自 ClassifiedIntent 轨迹事实，代号类型）
#[test]
fn denied_facts_carry_input_resource_capability_objects() {
    let snap = PolicySnapshot::default();
    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );
    assert_eq!(resp.denied.resource, res_target());
    assert_eq!(resp.denied.capability, Capability::Mutate);
    assert_eq!(resp.denied.objects, objects());
}

// §8 reason 机械取入参文本（引用策略事实，不编造、不泄露目标存在性）
#[test]
fn reason_is_the_input_policy_fact_text_verbatim() {
    let snap = PolicySnapshot::default();
    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );
    assert_eq!(resp.reason, reason());
}

// ----------------------------------------------------------------------------
// §8 your_grants 只导出 snapshot.grants[principal] 自身授权世界（Scope 受限）
// ----------------------------------------------------------------------------

// §8 your_grants 从 grants[principal] 导出：资源代号 → 能力名列表（用 Capability::as_str）
#[test]
fn your_grants_lists_self_resource_to_capability_names() {
    let snap = snapshot_with_self_grants();
    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    let mut expected: BTreeMap<ResourceCode, Vec<String>> = BTreeMap::new();
    // 两格 (Observe, Query)，能力名用 as_str；BTreeMap 键序确定 → Observe 在 Query 前
    // （as_str 文本分别为 "observe"/"query"，按底层 (ResourceCode, Capability) 键序导出）。
    expected.insert(
        res_target(),
        vec![
            Capability::Observe.as_str().to_string(),
            Capability::Query.as_str().to_string(),
        ],
    );
    assert_eq!(resp.your_grants, expected);
}

// §8 your_grants 能力名恰为 Capability::as_str 的小写动词文本（"observe"/"query"，非 Debug 形态）
#[test]
fn your_grants_capability_names_use_as_str_lowercase() {
    let snap = snapshot_with_self_grants();
    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    let caps = resp
        .your_grants
        .get(&res_target())
        .expect("self-granted resource must appear in your_grants");
    assert!(
        caps.contains(&"observe".to_string()),
        "expected lowercase as_str verb name"
    );
    assert!(
        caps.contains(&"query".to_string()),
        "expected lowercase as_str verb name"
    );
    assert!(
        !caps.contains(&"Observe".to_string()),
        "must not use Debug-form capability name"
    );
}

// §8 fail-closed：grants 缺 principal → your_grants 为空 BTreeMap（空集合，不放行、不报错）
#[test]
fn your_grants_is_empty_when_snapshot_has_no_grants_for_principal() {
    let snap = snapshot_with_self_grants();
    // 用一个快照里没有授权的 principal 组装。
    let resp = assemble(
        &snap,
        &other_principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );
    assert!(
        resp.your_grants.is_empty(),
        "missing principal must yield empty your_grants (fail-closed), not a grant"
    );
}

// §8 DENY_RESPONSE_SCOPE_BOUNDED：your_grants 绝不枚举他人 principal 的授权资源
#[test]
fn your_grants_never_leaks_other_principals_world() {
    // principal() 自己授权空；other_principal() 在 db-main 有格。
    let mut other_per: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    other_per.insert(
        (res_target(), Capability::Query),
        grant_cell(res_target(), Capability::Query),
    );
    let mut grants = BTreeMap::new();
    grants.insert(other_principal(), other_per);
    let snap = PolicySnapshot {
        policy_rev: 7,
        grants,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );
    assert!(
        resp.your_grants.is_empty(),
        "your_grants must reflect only the requesting principal's own world"
    );
}

// ----------------------------------------------------------------------------
// §8 L-5 不可区分：Scope 外但存在 vs 根本不存在 → 两次 DenyResponse 完全相等
// ----------------------------------------------------------------------------

// §8 L-5：对同一 principal，以"Scope 外但存在的资源"与"不存在的资源"组装两次 → 结构相等
#[test]
fn l5_out_of_scope_and_nonexistent_resources_are_structurally_indistinguishable() {
    // 快照：principal() 只授权 db-main；db-secret 存在于他人世界但不在 principal 的 scope；
    // db-ghost 任何角落都没有。两类目标都不该让响应有任何可区分痕迹。
    let mut self_per: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    self_per.insert(
        (res_target(), Capability::Query),
        grant_cell(res_target(), Capability::Query),
    );
    let mut other_per: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    other_per.insert(
        (res_existing_out_of_scope(), Capability::Query),
        grant_cell(res_existing_out_of_scope(), Capability::Query),
    );
    let mut grants = BTreeMap::new();
    grants.insert(principal(), self_per);
    grants.insert(other_principal(), other_per);

    // grantable / deny_notes 也只覆盖 db-main，使两类越界目标在这些字段上同样无差别。
    let mut grantable = BTreeMap::new();
    grantable.insert(res_target(), vec![Capability::Mutate]);
    let mut deny_notes = BTreeMap::new();
    deny_notes.insert(
        (res_target(), Capability::Mutate),
        "ask the owner".to_string(),
    );

    let snap = PolicySnapshot {
        policy_rev: 7,
        grants,
        grantable,
        deny_notes,
        ..PolicySnapshot::default()
    };

    let common_reason = "rbac: no grant cell".to_string();
    let resp_existing = assemble(
        &snap,
        &principal(),
        &res_existing_out_of_scope(),
        Capability::Query,
        &objects(),
        common_reason.clone(),
    );
    let resp_ghost = assemble(
        &snap,
        &principal(),
        &res_nonexistent(),
        Capability::Query,
        &objects(),
        common_reason,
    );

    // 两次响应的 your_grants / request_hint / operator_note / reason 必须全相等。
    assert_eq!(
        resp_existing.your_grants, resp_ghost.your_grants,
        "your_grants must not distinguish existing-vs-nonexistent target"
    );
    assert_eq!(
        resp_existing.request_hint, resp_ghost.request_hint,
        "request_hint must not distinguish existing-vs-nonexistent target"
    );
    assert_eq!(
        resp_existing.operator_note, resp_ghost.operator_note,
        "operator_note must not distinguish existing-vs-nonexistent target"
    );
    assert_eq!(
        resp_existing.reason, resp_ghost.reason,
        "reason must not distinguish existing-vs-nonexistent target"
    );
}

// §8 L-5：两类越界目标的 DenyResponse 序列化逐字节相同（防拓扑探测，端到端）
#[test]
fn l5_serialized_deny_responses_are_byte_identical_for_existing_and_nonexistent() {
    let mut self_per: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    self_per.insert(
        (res_target(), Capability::Query),
        grant_cell(res_target(), Capability::Query),
    );
    let mut other_per: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    other_per.insert(
        (res_existing_out_of_scope(), Capability::Query),
        grant_cell(res_existing_out_of_scope(), Capability::Query),
    );
    let mut grants = BTreeMap::new();
    grants.insert(principal(), self_per);
    grants.insert(other_principal(), other_per);

    let snap = PolicySnapshot {
        policy_rev: 7,
        grants,
        ..PolicySnapshot::default()
    };

    let common_reason = "rbac: no grant cell".to_string();
    let resp_existing = assemble(
        &snap,
        &principal(),
        &res_existing_out_of_scope(),
        Capability::Query,
        &objects(),
        common_reason.clone(),
    );
    let resp_ghost = assemble(
        &snap,
        &principal(),
        &res_nonexistent(),
        Capability::Query,
        &objects(),
        common_reason,
    );

    // denied.resource 显然不同（请求的就是不同代号），故只比较"你的授权世界"
    // 相关三字段的序列化，它们才是会泄露目标存在性的探测面。
    let j_existing =
        serde_json::to_string(&ScopeView::from(&resp_existing)).expect("scope view serializes");
    let j_ghost =
        serde_json::to_string(&ScopeView::from(&resp_ghost)).expect("scope view serializes");
    assert_eq!(
        j_existing, j_ghost,
        "scope-bounded fields must serialize byte-identically (no topology leak)"
    );
}

/// 仅取 DenyResponse 中"会泄露目标存在性"的三字段做序列化对比（denied.resource
/// 本就因请求不同而不同，不在不可区分约束内）。
#[derive(serde::Serialize)]
struct ScopeView<'a> {
    your_grants: &'a BTreeMap<ResourceCode, Vec<String>>,
    request_hint: &'a Option<String>,
    operator_note: &'a Option<String>,
}

impl<'a> From<&'a DenyResponse> for ScopeView<'a> {
    fn from(r: &'a DenyResponse) -> Self {
        Self {
            your_grants: &r.your_grants,
            request_hint: &r.request_hint,
            operator_note: &r.operator_note,
        }
    }
}

// ----------------------------------------------------------------------------
// §8 L-6 request_hint：grantable 含动词 → postern elevate 命令；否则 None/null
// ----------------------------------------------------------------------------

// §8 L-6：动词在 grantable[resource] 中 → request_hint 为机械生成的 postern elevate 命令
#[test]
fn l6_request_hint_is_postern_elevate_command_when_capability_is_grantable() {
    let mut grantable = BTreeMap::new();
    grantable.insert(res_target(), vec![Capability::Observe, Capability::Mutate]);
    let snap = PolicySnapshot {
        policy_rev: 7,
        grantable,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    let hint = resp
        .request_hint
        .expect("grantable capability must yield a request_hint");
    // 机械生成的提升命令：含 postern elevate + 资源代号 + 动词（代号类事实，绝无机密）。
    assert!(
        hint.contains("postern elevate"),
        "hint must be a postern elevate command, got {hint:?}"
    );
    assert!(hint.contains("db-main"), "hint must cite the resource code");
    assert!(hint.contains("mutate"), "hint must cite the requested verb");
}

// §8 L-6：动词不在 grantable[resource] 中 → request_hint 为 None（序列化为 null）
#[test]
fn l6_request_hint_is_none_when_capability_not_grantable() {
    let mut grantable = BTreeMap::new();
    // grantable 只含 Observe，不含被请求的 Mutate。
    grantable.insert(res_target(), vec![Capability::Observe]);
    let snap = PolicySnapshot {
        policy_rev: 7,
        grantable,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    assert_eq!(
        resp.request_hint, None,
        "ungrantable capability must yield request_hint = None"
    );
    // request_hint 不带 skip_serializing_if → None 序列化为显式 null。
    let json = serde_json::to_value(&resp).expect("response serializes");
    assert_eq!(
        json.get("request_hint"),
        Some(&serde_json::Value::Null),
        "None request_hint must serialize as JSON null"
    );
}

// §8 L-6：resource 完全缺失于 grantable → request_hint 为 None（缺格→None，不吞错）
#[test]
fn l6_request_hint_is_none_when_resource_absent_from_grantable() {
    // 空 grantable：被请求资源根本不在表里。
    let snap = PolicySnapshot {
        policy_rev: 7,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    assert_eq!(
        resp.request_hint, None,
        "missing resource in grantable must yield request_hint = None"
    );
}

// ----------------------------------------------------------------------------
// §8 L-6 operator_note：deny_notes 含 (res,cap) → 原样 Some；缺省 None 且不序列化
// ----------------------------------------------------------------------------

// §8 L-6：deny_notes[(res,cap)] 存在 → operator_note 取出原样为 Some（人亲笔，原样转述）
#[test]
fn l6_operator_note_is_some_verbatim_when_deny_note_present() {
    let note = "Contact db-team before requesting write access.".to_string();
    let mut deny_notes = BTreeMap::new();
    deny_notes.insert((res_target(), Capability::Mutate), note.clone());
    let snap = PolicySnapshot {
        policy_rev: 7,
        deny_notes,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    assert_eq!(
        resp.operator_note,
        Some(note),
        "present deny note must be relayed verbatim as Some"
    );
}

// §8 L-6：operator_note 按 (resource, capability) 精确匹配——动词不符 → None
#[test]
fn l6_operator_note_is_none_when_note_keyed_on_different_capability() {
    // deny_notes 注记在 (db-main, Observe)，但请求的是 (db-main, Mutate)。
    let mut deny_notes = BTreeMap::new();
    deny_notes.insert(
        (res_target(), Capability::Observe),
        "note for observe".to_string(),
    );
    let snap = PolicySnapshot {
        policy_rev: 7,
        deny_notes,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    assert_eq!(
        resp.operator_note, None,
        "deny note keyed on a different capability must not be relayed"
    );
}

// §8 L-6：缺省 operator_note → None，且经 serde_json 序列化时该字段完全不出现（skip_serializing_if）
#[test]
fn l6_operator_note_none_is_absent_from_serialized_json() {
    let snap = PolicySnapshot {
        policy_rev: 7,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    assert_eq!(resp.operator_note, None, "absent deny note must yield None");
    let json = serde_json::to_value(&resp).expect("response serializes");
    let obj = json
        .as_object()
        .expect("response serializes as a JSON object");
    assert!(
        !obj.contains_key("operator_note"),
        "None operator_note must be ABSENT from the JSON (skip_serializing_if)"
    );
}

// ----------------------------------------------------------------------------
// §8 确定性：相同入参多次组装 → DenyResponse 逐字节相同（审计可对账前提）
// ----------------------------------------------------------------------------

// §8 确定性：同一 (snapshot, principal, resource, capability, objects, reason) 多次组装 → 全等
#[test]
fn assemble_is_deterministic_across_repeated_calls() {
    let snap = snapshot_with_self_grants();

    let r1 = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );
    let r2 = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    assert_eq!(r1, r2, "same inputs must yield identical DenyResponse");
    let j1 = serde_json::to_string(&r1).expect("serializes");
    let j2 = serde_json::to_string(&r2).expect("serializes");
    assert_eq!(j1, j2, "serialized DenyResponse must be byte-identical");
}

// §8 your_grants 的 BTreeMap 键序确定：多资源自身授权按资源代号字典序导出
#[test]
fn your_grants_btreemap_orders_resources_deterministically() {
    // principal() 在两个资源上各有授权：插入序故意"乱"，期望导出按代号字典序。
    let mut per_principal: BTreeMap<(ResourceCode, Capability), GrantCell> = BTreeMap::new();
    let res_a = ResourceCode::new("aaa-first");
    let res_z = ResourceCode::new("zzz-last");
    per_principal.insert(
        (res_z.clone(), Capability::Query),
        grant_cell(res_z.clone(), Capability::Query),
    );
    per_principal.insert(
        (res_a.clone(), Capability::Query),
        grant_cell(res_a.clone(), Capability::Query),
    );
    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);
    let snap = PolicySnapshot {
        policy_rev: 7,
        grants,
        ..PolicySnapshot::default()
    };

    let resp = assemble(
        &snap,
        &principal(),
        &res_target(),
        Capability::Mutate,
        &objects(),
        reason(),
    );

    // BTreeMap 迭代序即键的字典序：aaa-first 在 zzz-last 之前。
    let keys: Vec<&str> = resp.your_grants.keys().map(|k| k.as_str()).collect();
    assert_eq!(
        keys,
        vec!["aaa-first", "zzz-last"],
        "your_grants must iterate resource codes in deterministic BTreeMap order"
    );
}
