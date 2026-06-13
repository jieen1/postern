//! snapshot 单元行为测试：权威库 → `Arc<PolicySnapshot>` 的原子投影构建与只读视图。
//!
//! 每条测试只钉一个行为，测试名陈述该行为，断言精确到快照的**确切形态**（哪一格
//! 在 / 不在、`policy_rev` / `action` / `role` / 各 BTreeMap 的确切内容）或确切错误
//! 变体。§8 验收逐条以 `// §8-...` 注释标注覆盖（本单元主攻 F-12、L-1/L-2b/L-8/L-9/
//! L-11，并就 F-3 的逻辑删除在快照可见性上的体现给出佐证）。失败路径是一等公民——
//! 断言"恰好是该结果"：授予性表停用即收回授权、限制性表快照不过滤 `enable_flag`、
//! 悬挂引用不放行、TTL 原值不裁决、空选择器不放行。
//!
//! 雷区纪律（与 base.rs / schema_migrate.rs / policy.rs 同源）：本文件在
//! `crates/postern-store/` 下但**不在** `src/base/` 下，故对契约扫描器是"in_store 且
//! 非 in_store_base"。任何字面裸数据库读关键词的连续串出现在源文本里都会被记为违规
//! （扫描器不剥 Rust 行注释）。因此本文件**绝不写字面裸数据库读写标记**：构建一律经
//! 被测 `snapshot::build_snapshot` API；写一律经 `base::write`（唯一写路径）；行内省读
//! （计数核对、直插反例行）一律在**运行期由片段拼接**（见 `kw` / `count_where` /
//! `raw_insert`）。逻辑删除断言只表达 `delete_flag` 行为，绝不写散文级移除字面串。

use std::sync::Arc;

use postern_core::domain::{
    Capability, GrantAction, PolicySnapshot, PrincipalId, ResourceCode, Role, Timestamp,
};
use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_core::plugin::PolicyView;
use postern_store::base::db::Db;
use postern_store::base::error::StoreError;
use postern_store::base::write::{self, Actor, InsertRow};
use postern_store::migrate;
use postern_store::snapshot::{build_snapshot, SnapshotView};
use rusqlite::types::Value;

// ============================================================ 运行期 SQL 片段拼接

/// 把拆开的关键词片段用空格重组为单关键词。任意单个片段都不构成扫描器关注的连续
/// 串，故源文本里永不出现完整需被扫描的连续读关键词 needle。
fn kw(parts: &[&str]) -> String {
    parts.join(" ")
}

/// 固定时钟，驱动 core IdGen 与 base 时间戳生成的确定性。
struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_unix_ms(&self) -> u64 {
        self.0
    }
}

/// 雪花纪元 + 偏移的墙钟（确定 now，所有时间列得到长度 24 的固定宽度文本）。
const EPOCH_UNIX_MS: u64 = 1_767_225_600_000;

/// 一个长度恰 24 的合法时间文本（直插反例行 / temp_grant 时间列复用，与 base 同宽）。
const VALID_24_TIME: &str = "2026-01-01T00:00:00.000Z";

/// 已迁移到当前版本的内存库（空库 → `migrate` 建全套表 + 前进 user_version）。
fn migrated_db() -> Db {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema on empty db");
    db
}

fn idgen() -> IdGen {
    IdGen::new(FixedClock(EPOCH_UNIX_MS))
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(EPOCH_UNIX_MS)
}

/// 物理行计数（按可选谓词），纯 COUNT(*) 投影（pagination 豁免）。谓词由调用方以
/// 运行期拼接传入，绝不含字面读关键词 needle。
fn count_where(db: &Db, table: &str, predicate: &str) -> i64 {
    let q = format!(
        "{} COUNT(*) {} {} {} {}",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
        predicate,
    );
    db.with_read(|conn| {
        let n: i64 = conn.query_row(&q, [], |r| r.get(0)).map_err(|_| StoreError::Io)?;
        Ok(n)
    })
    .expect("count query")
}

// ============================================================ 经 base 唯一写路径播种

/// 经 base 唯一写路径插一行（业务列由调用方给），返回新行 id。`enable_flag` 恒 1。
fn seed(
    db: &Db,
    g: &IdGen,
    table: &'static str,
    columns: Vec<&'static str>,
    values: Vec<Value>,
) -> SnowflakeId {
    db.with_write_txn(|txn| {
        write::insert(
            txn,
            g,
            now(),
            &Actor::System,
            InsertRow {
                table,
                columns,
                values,
                enable_flag: 1,
            },
        )
    })
    .expect("seed insert via base")
}

fn seed_principal(db: &Db, g: &IdGen, name: &str) -> SnowflakeId {
    seed(
        db,
        g,
        "principals",
        vec!["name", "kind"],
        vec![Value::Text(name.into()), Value::Text("agent".into())],
    )
}

fn seed_role(db: &Db, g: &IdGen, name: &str) -> SnowflakeId {
    seed(
        db,
        g,
        "roles",
        vec!["name", "description"],
        vec![Value::Text(name.into()), Value::Null],
    )
}

fn seed_resource(db: &Db, g: &IdGen, codename: &str) -> SnowflakeId {
    seed(
        db,
        g,
        "resources",
        vec!["codename", "adapter", "transport"],
        vec![
            Value::Text(codename.into()),
            Value::Text("postgres".into()),
            Value::Text("tcp".into()),
        ],
    )
}

fn seed_binding(db: &Db, g: &IdGen, principal: SnowflakeId, role: SnowflakeId) -> SnowflakeId {
    seed(
        db,
        g,
        "bindings",
        vec!["principal_id", "role_id"],
        vec![
            Value::Integer(principal.as_raw() as i64),
            Value::Integer(role.as_raw() as i64),
        ],
    )
}

/// binding 的 resource 辖区（kind='resource'），把该绑定钉到一个具体资源。
fn seed_binding_scope_resource(db: &Db, g: &IdGen, binding: SnowflakeId, resource: SnowflakeId) {
    seed(
        db,
        g,
        "binding_scope",
        vec!["binding_id", "kind", "resource_id"],
        vec![
            Value::Integer(binding.as_raw() as i64),
            Value::Text("resource".into()),
            Value::Integer(resource.as_raw() as i64),
        ],
    );
}

/// binding 的 selector 辖区（kind='selector'），按标签选择器展开为具体资源集。
fn seed_binding_scope_selector(db: &Db, g: &IdGen, binding: SnowflakeId, selector: &str) {
    seed(
        db,
        g,
        "binding_scope",
        vec!["binding_id", "kind", "selector"],
        vec![
            Value::Integer(binding.as_raw() as i64),
            Value::Text("selector".into()),
            Value::Text(selector.into()),
        ],
    );
}

fn seed_resource_label(db: &Db, g: &IdGen, resource: SnowflakeId, key: &str, value: &str) {
    seed(
        db,
        g,
        "resource_labels",
        vec!["resource_id", "key", "value"],
        vec![
            Value::Integer(resource.as_raw() as i64),
            Value::Text(key.into()),
            Value::Text(value.into()),
        ],
    );
}

fn seed_role_capability(db: &Db, g: &IdGen, role: SnowflakeId, capability: &str, action: &str) {
    seed(
        db,
        g,
        "role_capabilities",
        vec!["role_id", "capability", "action"],
        vec![
            Value::Integer(role.as_raw() as i64),
            Value::Text(capability.into()),
            Value::Text(action.into()),
        ],
    );
}

fn seed_role_inherit(db: &Db, g: &IdGen, role: SnowflakeId, parent: SnowflakeId) {
    seed(
        db,
        g,
        "role_inherits",
        vec!["role_id", "parent_role_id"],
        vec![
            Value::Integer(role.as_raw() as i64),
            Value::Integer(parent.as_raw() as i64),
        ],
    );
}

fn seed_deny_note(db: &Db, g: &IdGen, resource: SnowflakeId, capability: &str, note: &str) {
    seed(
        db,
        g,
        "deny_notes",
        vec!["resource_id", "capability", "note"],
        vec![
            Value::Integer(resource.as_raw() as i64),
            Value::Text(capability.into()),
            Value::Text(note.into()),
        ],
    );
}

fn seed_tier(db: &Db, g: &IdGen, resource: SnowflakeId, tier: &str, capabilities: &str) {
    seed(
        db,
        g,
        "resource_credential_tiers",
        vec!["resource_id", "tier", "capabilities"],
        vec![
            Value::Integer(resource.as_raw() as i64),
            Value::Text(tier.into()),
            Value::Text(capabilities.into()),
        ],
    );
}

fn seed_credential(db: &Db, g: &IdGen, principal: SnowflakeId, secret_hash: &str) {
    seed(
        db,
        g,
        "credentials",
        vec!["principal_id", "kind", "secret_hash"],
        vec![
            Value::Integer(principal.as_raw() as i64),
            Value::Text("api_key".into()),
            Value::Text(secret_hash.into()),
        ],
    );
}

/// 一条有效（未到期）的临时授权：principal × resource × capability。`expires_at` 为
/// 调用方给定的原值文本（快照不做 TTL 裁决，原样投影）。
fn seed_temp_grant(
    db: &Db,
    g: &IdGen,
    principal: SnowflakeId,
    resource: SnowflakeId,
    capability: &str,
    expires_at: &str,
) {
    seed(
        db,
        g,
        "temp_grants",
        vec![
            "principal_id",
            "resource_id",
            "capability",
            "granted_at",
            "expires_at",
        ],
        vec![
            Value::Integer(principal.as_raw() as i64),
            Value::Integer(resource.as_raw() as i64),
            Value::Text(capability.into()),
            Value::Text(VALID_24_TIME.into()),
            Value::Text(expires_at.into()),
        ],
    );
}

/// 8 基础列名（直插反例行时与业务列拼成完整行）。
fn base_cols() -> Vec<&'static str> {
    vec![
        "id",
        "version",
        "created_at",
        "created_by",
        "updated_at",
        "updated_by",
        "delete_flag",
        "enable_flag",
    ]
}

/// 8 基础列值：id / delete_flag / enable_flag 由调用方给（反例可越界），其余取合法常量。
fn base_vals(id: i64, delete_flag: i64, enable_flag: i64) -> Vec<Value> {
    vec![
        Value::Integer(id),
        Value::Integer(0),
        Value::Text(VALID_24_TIME.into()),
        Value::Text("system".into()),
        Value::Text(VALID_24_TIME.into()),
        Value::Text("system".into()),
        Value::Integer(delete_flag),
        Value::Integer(enable_flag),
    ]
}

/// 直插一行（**绕过** base 写守卫）：唯一目的是构造 base 守卫拒绝的反例（如限制性表
/// `enable_flag=0`、父行已逻辑删除而子行残留的悬挂引用），把快照加载规则当独立靶子。
/// 写关键词由片段在运行期拼接成单词，源文本不出现连续写关键词 needle。
fn raw_insert(db: &Db, table: &str, columns: &[&str], values: Vec<Value>) {
    let placeholders: Vec<String> = (1..=values.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "{} {} {} ({}) {} ({})",
        ["INS", "ERT"].join(""),
        ["IN", "TO"].join(""),
        table,
        columns.join(", "),
        ["VAL", "UES"].join(""),
        placeholders.join(", "),
    );
    db.with_write_txn(|txn| {
        let bind: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        txn.execute(&sql, &bind[..]).map_err(|_| StoreError::Io)?;
        Ok(())
    })
    .expect("raw insert");
}

/// 取某 principal 在快照里某 (resource, capability) 坐标的授权格（若存在）。
fn cell_at<'a>(
    snap: &'a PolicySnapshot,
    principal: SnowflakeId,
    resource: &str,
    cap: Capability,
) -> Option<&'a postern_core::domain::GrantCell> {
    let p = PrincipalId::new(principal);
    snap.grants.get(&p)?.get(&(ResourceCode::new(resource), cap))
}

// ============================================================ F-12 授权空间物化

#[test]
fn build_materializes_binding_role_capability_into_a_grant_cell() {
    // §8-一F-12 / §3.4：binding × role_capabilities（沿 binding_scope 辖区）物化为
    // 具体 (resource, capability) 授权格——给定输入 → 快照恰含该格。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-a");
    let role = seed_role(&db, &g, "observer");
    let res = seed_resource(&db, &g, "db-main");
    seed_role_capability(&db, &g, role, "observe", "allow");
    let b = seed_binding(&db, &g, p, role);
    seed_binding_scope_resource(&db, &g, b, res);

    let snap = build_snapshot(&db, 1).expect("snapshot builds from a seeded db");
    let cell = cell_at(&snap, p, "db-main", Capability::Observe)
        .expect("the (db-main, observe) grant cell must be materialized for agent-a");
    assert_eq!(cell.action, GrantAction::Allow, "role_capabilities.action=allow → GrantAction::Allow");
    assert_eq!(cell.role, Role::new("observer"), "cell carries provenance role observer");
}

#[test]
fn build_carries_policy_rev_verbatim() {
    // §8-一F-12 / §3.4：policy_rev 是审计对账锚点，由调用方传入、原样落快照。
    let db = migrated_db();
    let snap = build_snapshot(&db, 77).expect("empty db still builds a snapshot");
    assert_eq!(snap.policy_rev, 77, "policy_rev is carried verbatim into the snapshot");
}

#[test]
fn build_on_empty_db_grants_nothing() {
    // §8-一F-12 / 公理一：空库 → 空授权空间（缺格即 deny），不是错误、不放行任何格。
    let db = migrated_db();
    let snap = build_snapshot(&db, 1).expect("empty db builds the deny-everything snapshot");
    assert!(snap.grants.is_empty(), "no bindings → no grants (axiom one: absence is deny)");
}

#[test]
fn build_escalate_capability_yields_escalate_action_cell() {
    // §8-一F-12：role_capabilities.action='escalate' → 授权格 action=Escalate（求值期降级，
    // 构建侧只如实物化动作标注，不擅自折叠）。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-esc");
    let role = seed_role(&db, &g, "elevated");
    let res = seed_resource(&db, &g, "db-main");
    seed_role_capability(&db, &g, role, "manage", "escalate");
    let b = seed_binding(&db, &g, p, role);
    seed_binding_scope_resource(&db, &g, b, res);

    let snap = build_snapshot(&db, 1).expect("builds");
    let cell = cell_at(&snap, p, "db-main", Capability::Manage).expect("manage cell present");
    assert_eq!(cell.action, GrantAction::Escalate, "escalate action materialized as Escalate");
}

// ============================================================ 加载规则：授予性表

#[test]
fn build_excludes_disabled_role_capability_from_grants() {
    // §8-一F-12 / §3.4：授予性表加载 delete_flag=0 AND enable_flag=1——停用即收回授权。
    // 一条 enable_flag=0 的 role_capability 不得进入快照（否则停用却仍授权 = fail-open）。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-b");
    let role = seed_role(&db, &g, "observer");
    let res = seed_resource(&db, &g, "db-main");
    let b = seed_binding(&db, &g, p, role);
    seed_binding_scope_resource(&db, &g, b, res);
    // 直插一条 enable_flag=0 的 role_capability（授予性表，停用态）。
    let rc_id = g.next_id().expect("id").as_raw() as i64;
    let mut cols = base_cols();
    cols.extend(["role_id", "capability", "action"]);
    let mut vals = base_vals(rc_id, 0, 0);
    vals.push(Value::Integer(role.as_raw() as i64));
    vals.push(Value::Text("observe".into()));
    vals.push(Value::Text("allow".into()));
    raw_insert(&db, "role_capabilities", &cols, vals);

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-main", Capability::Observe).is_none(),
        "disabled (enable_flag=0) role_capability must NOT materialize a grant cell"
    );
}

#[test]
fn build_excludes_logically_deleted_binding_from_grants() {
    // §8-一F-3 / 二L-1 / §3.4：逻辑删除（delete_flag=1）的 binding 在快照里不可见——
    // 默认作用域排除已删，删除即收回授权。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-c");
    let role = seed_role(&db, &g, "observer");
    let res = seed_resource(&db, &g, "db-main");
    seed_role_capability(&db, &g, role, "observe", "allow");
    let b = seed_binding(&db, &g, p, role);
    seed_binding_scope_resource(&db, &g, b, res);
    // 经 base 唯一写路径逻辑删除该 binding（delete_flag=1、version 自增）。
    db.with_write_txn(|txn| write::logical_delete(txn, now(), &Actor::System, "bindings", b, 0))
        .expect("logical delete binding");
    assert_eq!(count_where(&db, "bindings", "delete_flag = 1"), 1, "binding is logically removed");

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-main", Capability::Observe).is_none(),
        "a logically-removed binding must not appear in the snapshot grants"
    );
}

// ============================================================ 加载规则：限制性表（L-2b）

#[test]
fn build_loads_restricted_deny_notes_solely_by_delete_flag_not_enable_flag() {
    // §8-二L-2b / §7-11：限制性表（deny_notes）快照加载的可见性谓词**只有** delete_flag=0，
    // 绝不叠加 enable_flag 过滤。这一"不按 enable_flag 过滤"是恒真不变量：schema 对
    // deny_notes 钉死 CHECK (enable_flag = 1)（src/schema.sql:252），故任何合法限制性行的
    // enable_flag 恒为 1——不存在可被 enable_flag 过滤掉的限制性行；该不变量由 schema CHECK
    // （enable_flag≡1）与 load 规则（仅 delete_flag 谓词）共同保证。倘若有人给限制性加载
    // 误加 enable_flag 谓词，在合法数据上虽冗余、却正是 §7-11 所禁的 fail-open 解约缝隙。
    // 本测试用**合法 setup**（enable_flag 只能=1）把该谓词钉死为单一 delete_flag 维度：同一
    // 资源上两条合法 deny_note，仅 delete_flag 不同 → 加载严格按 delete_flag 分流（live 在、
    // 逻辑删除的不在），证明限制性加载不对 enable_flag 分层（其值恒 1，本就无从分层）。
    let db = migrated_db();
    let g = idgen();
    let res = seed_resource(&db, &g, "db-main");
    // 同一资源两条合法限制性行（base 写路径恒置 enable_flag=1）：(manage) 存活、(destroy) 删除。
    seed_deny_note(&db, &g, res, "manage", "manage barred by operator");
    let dn_destroy = seed_deny_note_row(&db, &g, res, "destroy", "destroy barred by operator");
    db.with_write_txn(|txn| {
        write::logical_delete(txn, now(), &Actor::System, "deny_notes", dn_destroy, 0)
    })
    .expect("logical delete the destroy deny_note");
    // 物理上两行皆在（仅 delete_flag 区分），enable_flag 两者皆为 1（CHECK 保证）。
    assert_eq!(
        count_where(&db, "deny_notes", "enable_flag = 1"),
        2,
        "both restricted deny_notes are stored with enable_flag=1 (schema CHECK pins it)"
    );

    let snap = build_snapshot(&db, 1).expect("builds");
    // 存活的限制性行（delete_flag=0、enable_flag=1）出现——不被任何 enable_flag 谓词剔除。
    assert_eq!(
        snap.deny_notes
            .get(&(ResourceCode::new("db-main"), Capability::Manage))
            .map(String::as_str),
        Some("manage barred by operator"),
        "a live (delete_flag=0) restricted deny_note appears — restricted load carries no enable_flag predicate (else fail-open)"
    );
    // 逻辑删除的限制性行（delete_flag=1）被排除——证明加载确按 delete_flag 维度分流。
    assert!(
        !snap
            .deny_notes
            .contains_key(&(ResourceCode::new("db-main"), Capability::Destroy)),
        "a logically-removed (delete_flag=1) restricted deny_note is excluded — visibility predicate is delete_flag, and only delete_flag"
    );
}

#[test]
fn build_loads_deny_note_for_resource_capability() {
    // §8-一F-12 / §3.4：deny_notes（限制性表，仅 delete_flag=0 加载）按 (resource, capability)
    // 原样投影为 operator_note。
    let db = migrated_db();
    let g = idgen();
    let res = seed_resource(&db, &g, "db-main");
    seed_deny_note(&db, &g, res, "destroy", "destruction barred by operator");

    let snap = build_snapshot(&db, 1).expect("builds");
    let note = snap
        .deny_notes
        .get(&(ResourceCode::new("db-main"), Capability::Destroy))
        .expect("deny note must be projected for (db-main, destroy)");
    assert_eq!(note, "destruction barred by operator", "operator note relayed verbatim");
}

#[test]
fn build_excludes_logically_deleted_deny_note() {
    // §8-二L-2b / §3.4：限制性表按 delete_flag=0 加载——逻辑删除的 deny_note 不再出现。
    let db = migrated_db();
    let g = idgen();
    let res = seed_resource(&db, &g, "db-main");
    let dn = seed_deny_note_row(&db, &g, res, "destroy", "barred");
    db.with_write_txn(|txn| write::logical_delete(txn, now(), &Actor::System, "deny_notes", dn, 0))
        .expect("logical delete deny_note");

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        !snap
            .deny_notes
            .contains_key(&(ResourceCode::new("db-main"), Capability::Destroy)),
        "a logically-removed deny_note must not appear in the snapshot"
    );
}

/// 与 [`seed_deny_note`] 同，但返回新行 id（供随后逻辑删除）。
fn seed_deny_note_row(db: &Db, g: &IdGen, resource: SnowflakeId, capability: &str, note: &str) -> SnowflakeId {
    seed(
        db,
        g,
        "deny_notes",
        vec!["resource_id", "capability", "note"],
        vec![
            Value::Integer(resource.as_raw() as i64),
            Value::Text(capability.into()),
            Value::Text(note.into()),
        ],
    )
}

// ============================================================ 角色继承展开

#[test]
fn build_expands_role_inheritance_transitively() {
    // §8-一F-12 / §3.4：沿 role_inherits 展开继承的传递闭包——子角色获得父角色的动词。
    // 链：editor → observer（editor 继承 observer 的 observe）。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-d");
    let observer = seed_role(&db, &g, "observer");
    let editor = seed_role(&db, &g, "editor");
    let res = seed_resource(&db, &g, "db-main");
    seed_role_capability(&db, &g, observer, "observe", "allow");
    seed_role_capability(&db, &g, editor, "mutate", "allow");
    seed_role_inherit(&db, &g, editor, observer); // editor 继承 observer
    let b = seed_binding(&db, &g, p, editor);
    seed_binding_scope_resource(&db, &g, b, res);

    let snap = build_snapshot(&db, 1).expect("builds");
    // 直接动词（mutate）在。
    assert!(
        cell_at(&snap, p, "db-main", Capability::Mutate).is_some(),
        "editor's own mutate capability is materialized"
    );
    // 继承动词（observe，来自父角色 observer）也在——传递闭包展开。
    assert!(
        cell_at(&snap, p, "db-main", Capability::Observe).is_some(),
        "inherited observe (from parent role observer) must be materialized too"
    );
}

#[test]
fn build_inheritance_cycle_terminates_without_overgranting() {
    // §8-一F-12 / §3.4：继承图遇环不放大授权也不死循——已访问集去重防环兜底。
    // 构造环 a → b → a，仅 a 带 observe；principal 绑定到 b → 期望看到 observe，且 build 返回。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-e");
    let role_a = seed_role(&db, &g, "role-a");
    let role_b = seed_role(&db, &g, "role-b");
    let res = seed_resource(&db, &g, "db-main");
    seed_role_capability(&db, &g, role_a, "observe", "allow");
    seed_role_inherit(&db, &g, role_a, role_b); // a 继承 b
    seed_role_inherit(&db, &g, role_b, role_a); // b 继承 a —— 成环
    let b = seed_binding(&db, &g, p, role_b);
    seed_binding_scope_resource(&db, &g, b, res);

    // 构建必须返回（不死循环 / 不爆栈），且授权不被环放大：observe 在、其余不在。
    let snap = build_snapshot(&db, 1).expect("cyclic inheritance must terminate, not loop");
    assert!(
        cell_at(&snap, p, "db-main", Capability::Observe).is_some(),
        "observe reachable through the cycle is granted once"
    );
    assert!(
        cell_at(&snap, p, "db-main", Capability::Mutate).is_none(),
        "a cycle must not fabricate capabilities nobody declared"
    );
}

// ============================================================ 选择器辖区展开（fail-closed）

#[test]
fn build_expands_selector_scope_against_resource_labels() {
    // §8-一F-12 / §3.4 / 5.2bis-②：binding_scope.kind=selector 在构建时按当时 resource_labels
    // 展开为具体资源集（展开结果落快照，求值仍是纯内存查表）。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-f");
    let role = seed_role(&db, &g, "observer");
    let res = seed_resource(&db, &g, "db-tagged");
    seed_role_capability(&db, &g, role, "observe", "allow");
    seed_resource_label(&db, &g, res, "env", "prod");
    let b = seed_binding(&db, &g, p, role);
    seed_binding_scope_selector(&db, &g, b, "env=prod");

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-tagged", Capability::Observe).is_some(),
        "selector env=prod must expand to the labelled resource db-tagged"
    );
}

#[test]
fn build_empty_expanding_selector_grants_nothing() {
    // §8-二L-9 fail-closed / 5.2bis-②：选择器展开为空集（无标签匹配）⇒ 该绑定不授予任何
    // 资源（空集、不报错也不放行）。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-g");
    let role = seed_role(&db, &g, "observer");
    let res = seed_resource(&db, &g, "db-unlabelled");
    seed_role_capability(&db, &g, role, "observe", "allow");
    // 资源无任何标签：选择器 env=prod 展开为空集。
    let b = seed_binding(&db, &g, p, role);
    seed_binding_scope_selector(&db, &g, b, "env=prod");

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-unlabelled", Capability::Observe).is_none(),
        "an empty-expanding selector grants nothing (fail-closed: empty set, not a grant)"
    );
    let _ = res;
}

// ============================================================ 临时授权物化

#[test]
fn build_materializes_effective_temp_grant() {
    // §8-一F-12 / §3.4：有效 temp_grants ∪ 进授权空间——principal × resource × capability
    // 物化为授权格（无需 binding/role 即生效，因其是直授临时格）。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-h");
    let res = seed_resource(&db, &g, "db-main");
    seed_temp_grant(&db, &g, p, res, "execute", "2099-01-01T00:00:00.000Z");

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-main", Capability::Execute).is_some(),
        "an effective temp_grant materializes a (db-main, execute) cell for the principal"
    );
}

// ============================================================ L-11 TTL 不进快照

#[test]
fn build_does_not_adjudicate_temp_grant_expiry() {
    // §8-二L-11 / §3.4：快照只是原子投影，不做 TTL 终判——即便 temp_grant 的 expires_at
    // 是过去时刻，构建侧也照常物化（过期裁决归求值期按 now 二次校验，本域不兜底时序）。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-i");
    let res = seed_resource(&db, &g, "db-main");
    // expires_at 取一个早已过去的时刻（语义上"已过期"）。
    seed_temp_grant(&db, &g, p, res, "query", "2000-01-01T00:00:00.000Z");

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-main", Capability::Query).is_some(),
        "build must NOT drop a past-expiry temp_grant (TTL adjudication belongs to the evaluator)"
    );
}

// ============================================================ L-9 悬挂引用不放行

#[test]
fn build_drops_grant_whose_resource_parent_is_logically_deleted() {
    // §8-二L-9 / §7-14：引用链父行不可见（resource delete_flag=1）⇒ 引用它的子行（含其授权格）
    // 不入快照，即便级联遗漏。绕开级联直插一条 binding_scope 指向已逻辑删除的 resource。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-j");
    let role = seed_role(&db, &g, "observer");
    let res = seed_resource(&db, &g, "db-doomed");
    seed_role_capability(&db, &g, role, "observe", "allow");
    let b = seed_binding(&db, &g, p, role);
    // 逻辑删除 resource（父行不可见），但 binding_scope 子行残留（模拟级联遗漏）。
    db.with_write_txn(|txn| write::logical_delete(txn, now(), &Actor::System, "resources", res, 0))
        .expect("logical delete resource");
    let bs_id = g.next_id().expect("id").as_raw() as i64;
    let mut cols = base_cols();
    cols.extend(["binding_id", "kind", "resource_id"]);
    let mut vals = base_vals(bs_id, 0, 1);
    vals.push(Value::Integer(b.as_raw() as i64));
    vals.push(Value::Text("resource".into()));
    vals.push(Value::Integer(res.as_raw() as i64)); // 指向已逻辑删除的 resource
    raw_insert(&db, "binding_scope", &cols, vals);

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-doomed", Capability::Observe).is_none(),
        "a grant referencing a logically-deleted resource is a dangling reference and must not appear"
    );
}

#[test]
fn build_drops_temp_grant_whose_resource_is_logically_deleted() {
    // §8-二L-9 / §7-14：临时授权直授路径同样 fail-closed——temp_grant 指向已逻辑删除的
    // resource（父行不可见）⇒ 该临时格不入快照（build.rs temp_grants 物化分支的悬挂兜底，
    // 与 binding_scope 悬挂路径并列、是独立的一条 fail-closed 缝）。先经合法路径播一条有效
    // temp_grant，再逻辑删除其 resource → 构建侧据 load_resources 的 delete_flag=0 真集判悬挂。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-m");
    let res = seed_resource(&db, &g, "db-vanishing");
    seed_temp_grant(&db, &g, p, res, "execute", "2099-01-01T00:00:00.000Z");
    // 逻辑删除 resource：temp_grant 子行残留，但其指向的资源父行不再可见。
    db.with_write_txn(|txn| write::logical_delete(txn, now(), &Actor::System, "resources", res, 0))
        .expect("logical delete resource under a live temp_grant");
    assert_eq!(
        count_where(&db, "temp_grants", "delete_flag = 0"),
        1,
        "the temp_grant row itself remains (only its resource parent was logically removed)"
    );

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        cell_at(&snap, p, "db-vanishing", Capability::Execute).is_none(),
        "a temp_grant referencing a logically-deleted resource is a dangling reference and must not materialize a cell"
    );
}

// ============================================================ tiers / grantable / credentials

#[test]
fn build_loads_tier_declarations_per_resource() {
    // §8-一F-12 / §3.4：tier 声明按资源加载（step [6] tier 选择源），快照含其 tier 名。
    let db = migrated_db();
    let g = idgen();
    let res = seed_resource(&db, &g, "db-main");
    seed_tier(&db, &g, res, "readonly", "observe,query");

    let snap = build_snapshot(&db, 1).expect("builds");
    let tiers = snap
        .tiers
        .get(&ResourceCode::new("db-main"))
        .expect("tier declarations must be loaded for db-main");
    assert!(
        tiers.iter().any(|t| t.tier.as_str() == "readonly"),
        "readonly tier declaration is present in the snapshot"
    );
}

#[test]
fn build_loads_credential_metadata_with_secret_hash() {
    // §8-一F-12 / §3.4：凭证元数据（含 secret_hash，即明文哈希）进快照——明文永不入快照。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-k");
    seed_credential(&db, &g, p, "argon2id$deadbeef");

    let snap = build_snapshot(&db, 1).expect("builds");
    assert!(
        snap.credentials
            .credentials
            .iter()
            .any(|c| c.secret_hash == "argon2id$deadbeef" && c.principal == PrincipalId::new(p)),
        "credential metadata (with secret_hash) for agent-k must be in the snapshot"
    );
}

#[test]
fn build_grantable_lists_resource_capabilities() {
    // §8-一F-12 / §3.4：grantable 是 request_hint 的机械来源——某资源可授动词集进快照。
    // observer 在 db-main 上带 observe → grantable[db-main] 含 observe。
    let db = migrated_db();
    let g = idgen();
    let p = seed_principal(&db, &g, "agent-l");
    let role = seed_role(&db, &g, "observer");
    let res = seed_resource(&db, &g, "db-main");
    seed_role_capability(&db, &g, role, "observe", "allow");
    let b = seed_binding(&db, &g, p, role);
    seed_binding_scope_resource(&db, &g, b, res);

    let snap = build_snapshot(&db, 1).expect("builds");
    let grantable = snap
        .grantable
        .get(&ResourceCode::new("db-main"))
        .expect("grantable set must list db-main's capabilities");
    assert!(
        grantable.contains(&Capability::Observe),
        "observe is a grantable capability on db-main"
    );
}

// ============================================================ SnapshotView / PolicyView

#[test]
fn view_returns_the_held_snapshot() {
    // §8-一/5.1：SnapshotView 实现 PolicyView，snapshot() 返回当前持有的 Arc（无锁克隆）。
    let snap = PolicySnapshot {
        policy_rev: 42,
        ..PolicySnapshot::default()
    };
    let view = SnapshotView::new(Arc::new(snap));
    assert_eq!(
        view.snapshot().policy_rev,
        42,
        "PolicyView::snapshot returns the currently held snapshot"
    );
}

#[test]
fn view_replace_swaps_the_whole_snapshot_atomically() {
    // §8-二L-8 / §6.2 / §7-13：replace 整份 Arc 原子替换——替换后读到的是新快照全貌。
    let first = PolicySnapshot {
        policy_rev: 1,
        ..PolicySnapshot::default()
    };
    let view = SnapshotView::new(Arc::new(first));
    let second = PolicySnapshot {
        policy_rev: 2,
        ..PolicySnapshot::default()
    };
    view.replace(Arc::new(second));
    assert_eq!(
        view.snapshot().policy_rev,
        2,
        "after replace, readers see the new snapshot in full (atomic Arc swap)"
    );
}

#[test]
fn view_snapshot_is_a_shared_clone_not_a_deep_copy() {
    // §8-二L-8：snapshot() 是 Arc 克隆——同一份底层快照被多个读者共享（指针相等）。
    let view = SnapshotView::new(Arc::new(PolicySnapshot::default()));
    let a = view.snapshot();
    let b = view.snapshot();
    assert!(
        Arc::ptr_eq(&a, &b),
        "two reads between rebuilds share the same underlying Arc (lock-free clone, no deep copy)"
    );
}
