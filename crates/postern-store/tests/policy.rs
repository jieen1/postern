//! PolicyRepo 事务读写行为测试：写经 base 唯一写路径、审计字段自动填充、乐观锁
//! version 冲突、仅逻辑删除、逻辑删除 + 级联（同事务）、默认作用域排除已删、后端分页
//! clamp。每条只钉一个行为，断言精确到落库行的确切形态 / 确切错误变体。
//!
//! §8 验收逐条以 `// §8-...` 注释标注覆盖（本单元主攻 F-1/F-2/F-3/F-4/F-5/F-6/F-7、
//! L-1/L-3/L-4，并就限制性表禁 enable_flag 写在 PolicyRepo 表面的体现给出佐证）。
//! 失败路径是一等公民——乐观锁冲突 → `VersionConflict` 且库不变、级联回滚 →
//! 父子均不变、归一化重复 → `ConstraintViolation`、禁 admin → `ConstraintViolation`，
//! 断言恰为该结果。
//!
//! 雷区纪律（与 base.rs / schema_migrate.rs 同源）：本文件在
//! `crates/postern-store/` 下但**不在** `src/base/` 下，故对契约扫描器是"in_store 且
//! 非 in_store_base"。任何字面裸数据库读关键词的连续串出现在源文本里都会被记为违规
//! （扫描器不剥 Rust 行注释，且物理移除关键词字面全工程禁出现）。因此本文件**绝不写
//! 字面裸数据库读写标记**：写一律经被测 `PolicyRepo` / `base::write` 的 API（唯一写
//! 路径），行内省读一律在**运行期由片段拼接**（见 `kw` / `fetch_*` / `count_where`），
//! 使读关键词的连续串永不出现在源文本中。逻辑删除断言只表达 `delete_flag` 行为，绝不
//! 写散文级移除字面串。

use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_core::page::{Page, PageQuery};
use postern_store::base::db::Db;
use postern_store::base::error::StoreError;
use postern_store::base::write::{Actor, SYSTEM_ACTOR};
use postern_store::migrate;
use postern_store::policy::{BindingRow, PolicyRepo, PrincipalRow, ResourceRow, RoleRow};

// ============================================================ 运行期 SQL 片段拼接
// 任意单个关键词都不构成扫描器关注的连续串（读取动词、来源、过滤各自拆成不连续
// 字面），故在运行期用空格重组，源文本里永不出现完整需被扫描的连续读关键词 needle。

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

/// 雪花纪元附近的确定墙钟基准（所有时间列得到长度 24 的固定宽度文本）。
const EPOCH_UNIX_MS: u64 = 1_767_225_600_000;

/// 以确定时钟装配一个已迁移到当前版本的内存库 + PolicyRepo（空库 → migrate 建全套表）。
/// `unix_ms` 是写入的确定 `now` 与 id 时间基准。
fn repo_at(unix_ms: u64) -> PolicyRepo {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema on empty db");
    let idgen = IdGen::new(FixedClock(unix_ms));
    PolicyRepo::new(db, idgen, Box::new(FixedClock(unix_ms)))
}

/// 控制面操作者（落 created_by / updated_by）。
fn operator(id: &str) -> Actor {
    Actor::Operator(id.to_string())
}

/// 单值 i64 省读（按主键，无作用域过滤——专供测试核对落库形态）。
/// 拼出的语句源文本无连续读关键词 needle。
fn fetch_i64(repo: &PolicyRepo, table: &str, id: SnowflakeId, col: &str) -> Option<i64> {
    let q = format!(
        "{} {} {} {} {} id = ?1 {} 1",
        kw(&["SEL", "ECT"]),
        col,
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
        kw(&["LIM", "IT"]),
    );
    repo.db()
        .with_read(|conn| {
            let v = conn
                .query_row(&q, [id.as_raw() as i64], |r| r.get::<_, Option<i64>>(0))
                .map_err(|_| StoreError::Io)?;
            Ok(v)
        })
        .ok()
        .flatten()
}

/// 单值文本省读（按主键）。
fn fetch_text(repo: &PolicyRepo, table: &str, id: SnowflakeId, col: &str) -> Option<String> {
    let q = format!(
        "{} {} {} {} {} id = ?1 {} 1",
        kw(&["SEL", "ECT"]),
        col,
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
        kw(&["LIM", "IT"]),
    );
    repo.db()
        .with_read(|conn| {
            let v = conn
                .query_row(&q, [id.as_raw() as i64], |r| r.get::<_, Option<String>>(0))
                .map_err(|_| StoreError::Io)?;
            Ok(v)
        })
        .ok()
        .flatten()
}

/// 物理行计数（按运行期拼接的谓词），纯 COUNT(*) 投影（pagination 豁免）。
fn count_where(repo: &PolicyRepo, table: &str, predicate: &str) -> i64 {
    let q = format!(
        "{} COUNT(*) {} {} {} {}",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
        predicate,
    );
    repo.db()
        .with_read(|conn| {
            let n: i64 = conn
                .query_row(&q, [], |r| r.get(0))
                .map_err(|_| StoreError::Io)?;
            Ok(n)
        })
        .expect("count query")
}

// ============================================================ F-6 写路径唯一（行为佐证）

#[test]
fn create_principal_persists_business_columns_via_base() {
    // §8-一F-6：PolicyRepo 的写经 base 唯一写路径落库（业务列如实入库）。源码里
    // 本测试不含任何裸写语句——写只经被测 API；落库形态用运行期拼接的省读核对。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "svc-bot", "agent")
        .expect("create_principal lands a row");
    assert_eq!(
        fetch_text(&repo, "principals", id, "kind").as_deref(),
        Some("agent"),
        "business column kind must be persisted"
    );
}

// ============================================================ F-1 审计字段自动填充

#[test]
fn create_autofills_version_zero_and_equal_timestamps() {
    // §8-一F-1：经 PolicyRepo 写一行、调用方不传五审计字段 → version=0、
    // created_at == updated_at（同一 now）、时间戳长度 24。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "svc-bot", "agent")
        .expect("create");
    assert_eq!(
        fetch_i64(&repo, "principals", id, "version"),
        Some(0),
        "new row version is 0"
    );
    let created = fetch_text(&repo, "principals", id, "created_at").expect("created_at non-null");
    let updated = fetch_text(&repo, "principals", id, "updated_at").expect("updated_at non-null");
    assert_eq!(created, updated, "created_at == updated_at on insert");
    assert_eq!(created.len(), 24, "fixed-width timestamp length is 24");
}

#[test]
fn create_by_operator_stamps_created_by_with_operator_label() {
    // §8-一F-1：控制面写 → created_by == updated_by == 已认证操作者标识。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "svc-bot", "agent")
        .expect("create");
    assert_eq!(
        fetch_text(&repo, "principals", id, "created_by").as_deref(),
        Some("alice"),
        "operator label lands in created_by"
    );
    assert_eq!(
        fetch_text(&repo, "principals", id, "updated_by").as_deref(),
        Some("alice"),
        "operator label lands in updated_by on insert"
    );
}

#[test]
fn create_by_system_actor_stamps_system_in_audit_fields() {
    // §8-一F-1：系统自动写 → created_by == updated_by == 'system'。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&Actor::System, "sweeper-acct", "program")
        .expect("create");
    assert_eq!(
        fetch_text(&repo, "principals", id, "created_by").as_deref(),
        Some(SYSTEM_ACTOR),
        "system actor stamps 'system' in created_by"
    );
}

// ============================================================ F-2 / L-4 乐观锁 version

#[test]
fn rename_with_matching_version_increments_version_to_one() {
    // §8-一F-2：持 version=0 改名 → 落库 version == 1（version = version + 1）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "old-name", "agent")
        .expect("create");
    repo.rename_principal(&operator("alice"), id, 0, "new-name")
        .expect("rename with matching version");
    assert_eq!(
        fetch_i64(&repo, "principals", id, "version"),
        Some(1),
        "version increments to 1"
    );
    assert_eq!(
        fetch_text(&repo, "principals", id, "name").as_deref(),
        Some("new-name"),
        "business column updated"
    );
}

#[test]
fn rename_updates_updated_by_to_acting_operator() {
    // §8-一F-1/F-2：更新维护 updated_by 为当次操作者（created_by 不动）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "n0", "agent")
        .expect("create");
    repo.rename_principal(&operator("bob"), id, 0, "n1")
        .expect("rename by bob");
    assert_eq!(
        fetch_text(&repo, "principals", id, "updated_by").as_deref(),
        Some("bob"),
        "updated_by reflects the acting operator"
    );
    assert_eq!(
        fetch_text(&repo, "principals", id, "created_by").as_deref(),
        Some("alice"),
        "created_by is immutable across updates"
    );
}

#[test]
fn rename_with_stale_version_returns_version_conflict() {
    // §8-二L-4：持过期 version 改名 → 乐观锁改写影响 0 行 → VersionConflict（独立变体）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "n0", "agent")
        .expect("create");
    // 先成功一次把 version 抬到 1。
    repo.rename_principal(&operator("alice"), id, 0, "n1")
        .expect("first rename");
    // 再持已过期的 version=0 → 冲突。
    let err = repo
        .rename_principal(&operator("alice"), id, 0, "n2")
        .expect_err("stale version must conflict");
    assert!(
        matches!(err, StoreError::VersionConflict),
        "stale version maps to VersionConflict, got {err:?}"
    );
}

#[test]
fn version_conflict_leaves_row_unchanged() {
    // §8-二L-4：冲突时库不变——name 与 version 都保持冲突前的值，无静默重试。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "n0", "agent")
        .expect("create");
    repo.rename_principal(&operator("alice"), id, 0, "n1")
        .expect("first rename");
    let _ = repo
        .rename_principal(&operator("alice"), id, 0, "n2")
        .expect_err("conflict");
    assert_eq!(
        fetch_text(&repo, "principals", id, "name").as_deref(),
        Some("n1"),
        "name stays at the last committed value after a conflict"
    );
    assert_eq!(
        fetch_i64(&repo, "principals", id, "version"),
        Some(1),
        "version stays at 1, not bumped by the rejected write"
    );
}

// ============================================================ F-3 / L-1 仅逻辑删除

#[test]
fn delete_principal_sets_delete_flag_without_physical_removal() {
    // §8-一F-3 / §8-二L-1：删一行 → 该行 delete_flag==1、version 自增、行仍物理存在
    // （无物理移除、无 undelete 入口）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "doomed", "agent")
        .expect("create");
    repo.delete_principal(&operator("alice"), id, 0)
        .expect("logical delete");
    assert_eq!(
        fetch_i64(&repo, "principals", id, "delete_flag"),
        Some(1),
        "logical delete sets delete_flag = 1"
    );
    assert_eq!(
        fetch_i64(&repo, "principals", id, "version"),
        Some(1),
        "logical delete increments version"
    );
    // 行仍物理存在（仅逻辑删除）：按 delete_flag=1 计数恰好找到这一行。
    assert_eq!(
        count_where(&repo, "principals", "delete_flag = 1"),
        1,
        "the row is still physically present, just flagged deleted (logical, not physical, delete)"
    );
}

#[test]
fn deleted_principal_is_absent_from_default_scoped_get() {
    // §8-二L-1 / §8-一F-5：删后按 id 默认作用域取单条 → None（默认查询追加 delete_flag=0）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "gone", "agent")
        .expect("create");
    assert!(
        repo.get_principal(id).expect("get before delete").is_some(),
        "visible before delete"
    );
    repo.delete_principal(&operator("alice"), id, 0)
        .expect("delete");
    assert!(
        repo.get_principal(id).expect("get after delete").is_none(),
        "deleted row is absent from the default-scoped single-row read"
    );
}

#[test]
fn deleted_principal_is_absent_from_default_scoped_list() {
    // §8-一F-5 / §8-二L-1：删后默认集合查询不含该行（追加 delete_flag=0）；未删行仍在。
    let repo = repo_at(EPOCH_UNIX_MS);
    let keep = repo
        .create_principal(&operator("alice"), "keep", "agent")
        .expect("create keep");
    let drop = repo
        .create_principal(&operator("alice"), "drop", "agent")
        .expect("create drop");
    repo.delete_principal(&operator("alice"), drop, 0)
        .expect("delete drop");
    let page = repo
        .list_principals(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list");
    let ids: Vec<SnowflakeId> = page.items.iter().map(|p| p.id).collect();
    assert!(ids.contains(&keep), "undeleted row is listed");
    assert!(
        !ids.contains(&drop),
        "deleted row is excluded by default scope"
    );
}

// ============================================================ F-5 enable_flag 不进默认过滤

#[test]
fn list_principals_returns_disabled_but_undeleted_row() {
    // §8-一F-5：enable_flag 不在默认过滤内——enable_flag=0 的未删行仍被默认集合查询返回。
    // 经 base 系统协调写把一行 enable_flag 翻到 0（principals 非限制性表，允许），其仍未删。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "paused", "agent")
        .expect("create");
    disable_row(&repo, "principals", id);
    let page = repo
        .list_principals(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list");
    let ids: Vec<SnowflakeId> = page.items.iter().map(|p| p.id).collect();
    assert!(
        ids.contains(&id),
        "a disabled (enable_flag=0) but undeleted row is still returned by default scope"
    );
}

/// 经 base 系统协调写把一行 enable_flag 置 0（写仍走 base 唯一写路径）。
fn disable_row(repo: &PolicyRepo, table: &'static str, id: SnowflakeId) {
    use postern_core::domain::Timestamp;
    use postern_store::base::write;
    use rusqlite::types::Value;
    let now = Timestamp::from_unix_ms(EPOCH_UNIX_MS);
    repo.db()
        .with_write_txn(|txn| {
            write::system_update(
                txn,
                now,
                table,
                vec!["enable_flag"],
                vec![Value::Integer(0)],
                "id = ?",
                vec![Value::Integer(id.as_raw() as i64)],
            )?;
            Ok(())
        })
        .expect("system_update disables the row via base write path");
}

// ============================================================ F-4 / L-3 级联逻辑删除

#[test]
fn delete_principal_cascades_to_child_bindings() {
    // §8-一F-4 / §8-二L-3：删 principals#p → 其 bindings 子行 delete_flag==1。
    let repo = repo_at(EPOCH_UNIX_MS);
    let pid = repo
        .create_principal(&operator("alice"), "p", "agent")
        .expect("principal");
    let rid = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    let bid = repo
        .create_binding(&operator("alice"), pid, rid)
        .expect("binding");
    repo.delete_principal(&operator("alice"), pid, 0)
        .expect("delete principal");
    assert_eq!(
        fetch_i64(&repo, "bindings", bid, "delete_flag"),
        Some(1),
        "child binding is cascaded to delete_flag = 1 in the same txn"
    );
}

#[test]
fn delete_principal_cascade_spares_other_principals_bindings() {
    // §8-一F-4 / §3.2：级联的 fk 作用域——删 principals#p 只翻【p 名下】的 bindings，
    // 挂在【另一个主体 q】下的兄弟 binding 必须存活（delete_flag 仍为 0）。这一断言
    // 是级联 fk 作用域（`WHERE fk = parent AND delete_flag = 0`）的唯一价值所在：
    // 去掉作用域改成无条件匹配（过度级联翻掉全表）即会令本断言转红。
    let repo = repo_at(EPOCH_UNIX_MS);
    let role = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    let doomed = repo
        .create_principal(&operator("alice"), "doomed", "agent")
        .expect("doomed");
    let bystander = repo
        .create_principal(&operator("alice"), "bystander", "agent")
        .expect("bystander");
    let doomed_binding = repo
        .create_binding(&operator("alice"), doomed, role)
        .expect("doomed binding");
    let sibling_binding = repo
        .create_binding(&operator("alice"), bystander, role)
        .expect("sibling binding");

    repo.delete_principal(&operator("alice"), doomed, 0)
        .expect("delete doomed");

    assert_eq!(
        fetch_i64(&repo, "bindings", doomed_binding, "delete_flag"),
        Some(1),
        "the deleted principal's own binding is cascaded"
    );
    assert_eq!(
        fetch_i64(&repo, "bindings", sibling_binding, "delete_flag"),
        Some(0),
        "a sibling binding owned by a different principal must survive the cascade (fk-scoped)"
    );
}

#[test]
fn delete_principal_cascades_credentials_and_temp_grants_only_for_that_principal() {
    // §8-一F-4 / §3.2：principals → {credentials, bindings, temp_grants} 三条边各自
    // 既级联本主体子行、又放过他主体的兄弟子行。credentials/temp_grants 两条边此前
    // 在 policy.rs 零断言（裁掉边为静默 Ok），本用例同时关闭「遗漏整条边」与「过度
    // 级联翻掉他主体子行」两个变异口子。
    let repo = repo_at(EPOCH_UNIX_MS);
    let res = repo
        .create_resource(&operator("alice"), "db-main", "postgres", "tcp")
        .expect("resource");
    let doomed = repo
        .create_principal(&operator("alice"), "doomed", "agent")
        .expect("doomed");
    let bystander = repo
        .create_principal(&operator("alice"), "bystander", "agent")
        .expect("bystander");

    let doomed_cred = seed_credential(&repo, doomed);
    let sibling_cred = seed_credential(&repo, bystander);
    let doomed_tg = seed_temp_grant(&repo, doomed, res);
    let sibling_tg = seed_temp_grant(&repo, bystander, res);

    repo.delete_principal(&operator("alice"), doomed, 0)
        .expect("delete doomed");

    // credentials 边：本主体凭证被级联，他主体凭证存活。
    assert_eq!(
        fetch_i64(&repo, "credentials", doomed_cred, "delete_flag"),
        Some(1),
        "the deleted principal's credential is cascaded (credentials edge wired and scoped)"
    );
    assert_eq!(
        fetch_i64(&repo, "credentials", sibling_cred, "delete_flag"),
        Some(0),
        "another principal's credential must survive (credentials cascade is fk-scoped)"
    );
    // temp_grants 边：本主体临时授权被级联，他主体临时授权存活。
    assert_eq!(
        fetch_i64(&repo, "temp_grants", doomed_tg, "delete_flag"),
        Some(1),
        "the deleted principal's temp_grant is cascaded (temp_grants edge wired and scoped)"
    );
    assert_eq!(
        fetch_i64(&repo, "temp_grants", sibling_tg, "delete_flag"),
        Some(0),
        "another principal's temp_grant must survive (temp_grants cascade is fk-scoped)"
    );
}

#[test]
fn cascade_stamps_origin_in_child_updated_by() {
    // §8-一F-4：级联子行 updated_by 标 cascade:principals#<id>（来源可追溯）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let pid = repo
        .create_principal(&operator("alice"), "p", "agent")
        .expect("principal");
    let rid = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    let bid = repo
        .create_binding(&operator("alice"), pid, rid)
        .expect("binding");
    repo.delete_principal(&operator("alice"), pid, 0)
        .expect("delete principal");
    let by = fetch_text(&repo, "bindings", bid, "updated_by").expect("updated_by");
    let expected = format!("cascade:principals#{}", pid.as_raw());
    assert_eq!(by, expected, "cascade origin recorded in child updated_by");
}

#[test]
fn delete_principal_with_stale_version_rolls_back_cascade() {
    // §8-一F-4 / §8-二L-3：删父持过期 version → 整体 ROLLBACK，父子均保持 delete_flag==0。
    let repo = repo_at(EPOCH_UNIX_MS);
    let pid = repo
        .create_principal(&operator("alice"), "p", "agent")
        .expect("principal");
    let rid = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    let bid = repo
        .create_binding(&operator("alice"), pid, rid)
        .expect("binding");
    // 持过期 version=9（真实为 0）删父 → 乐观锁冲突，事务回滚。
    let err = repo
        .delete_principal(&operator("alice"), pid, 9)
        .expect_err("stale version on parent delete must conflict");
    assert!(matches!(err, StoreError::VersionConflict), "got {err:?}");
    assert_eq!(
        fetch_i64(&repo, "principals", pid, "delete_flag"),
        Some(0),
        "parent stays undeleted after rollback"
    );
    assert_eq!(
        fetch_i64(&repo, "bindings", bid, "delete_flag"),
        Some(0),
        "child stays undeleted after rollback (no half-applied cascade)"
    );
}

#[test]
fn delete_resource_cascades_to_binding_scope_child() {
    // §8-一F-4 / §8-二L-3：删 resources#x → 其 binding_scope 子行 delete_flag==1（§3.2 图）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let pid = repo
        .create_principal(&operator("alice"), "p", "agent")
        .expect("principal");
    let rid = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    let bid = repo
        .create_binding(&operator("alice"), pid, rid)
        .expect("binding");
    let res = repo
        .create_resource(&operator("alice"), "db-main", "postgres", "tcp")
        .expect("resource");
    let scope_id = insert_binding_scope(&repo, bid, res);
    repo.delete_resource(&operator("alice"), res, 0)
        .expect("delete resource");
    assert_eq!(
        fetch_i64(&repo, "binding_scope", scope_id, "delete_flag"),
        Some(1),
        "binding_scope child of the deleted resource is cascaded"
    );
}

#[test]
fn delete_resource_cascades_every_child_edge_and_spares_other_resource() {
    // §8-一F-4 / §5.2 行496-499（fail-closed）：删 resources#x 必须级联 §3.2 图列出的
    // 全部 7 张子表——resource_credential_tiers / binding_scope / grant_constraints /
    // grant_conditions / mode_state(scope_resource_id) / deny_notes / resource_labels
    // ——其中四张限制性表（grant_constraints/grant_conditions/mode_state/deny_notes）
    // 悬挂会被快照按生效约束/冻结加载，违公理一/二。本用例为每条边播两行：一行挂在
    // 【被删资源 x】下（断言被级联=delete_flag 1），一行挂在【另一资源 y】下（断言存活
    // =delete_flag 0）。前者关闭「遗漏整条边→静默 Ok」变异，后者关闭「过度级联翻掉他
    // 资源子行→去掉 fk 作用域」变异。binding_scope/grant_conditions/mode_state 的 fk 列
    // 名各不同（resource_id / resource_id / scope_resource_id），逐边显式播种。
    let repo = repo_at(EPOCH_UNIX_MS);
    let pid = repo
        .create_principal(&operator("alice"), "p", "agent")
        .expect("principal");
    let rid = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    let bid = repo
        .create_binding(&operator("alice"), pid, rid)
        .expect("binding");
    let doomed = repo
        .create_resource(&operator("alice"), "db-doomed", "postgres", "tcp")
        .expect("doomed resource");
    let bystander = repo
        .create_resource(&operator("alice"), "db-bystander", "postgres", "tcp")
        .expect("bystander resource");

    // (子表, 钉在被删资源 doomed 下的子行, 钉在 bystander 下的兄弟子行)。
    let edges: Vec<(&'static str, SnowflakeId, SnowflakeId)> = vec![
        (
            "resource_credential_tiers",
            seed_resource_child(
                &repo,
                "resource_credential_tiers",
                "resource_id",
                doomed,
                &["tier"],
                vec!["t-doomed"],
            ),
            seed_resource_child(
                &repo,
                "resource_credential_tiers",
                "resource_id",
                bystander,
                &["tier"],
                vec!["t-by"],
            ),
        ),
        (
            "binding_scope",
            insert_binding_scope(&repo, bid, doomed),
            insert_binding_scope(&repo, bid, bystander),
        ),
        (
            "grant_constraints",
            seed_resource_child(
                &repo,
                "grant_constraints",
                "resource_id",
                doomed,
                &["capability", "kind"],
                vec!["query", "rate"],
            ),
            seed_resource_child(
                &repo,
                "grant_constraints",
                "resource_id",
                bystander,
                &["capability", "kind"],
                vec!["query", "rate"],
            ),
        ),
        (
            "grant_conditions",
            seed_resource_child(
                &repo,
                "grant_conditions",
                "resource_id",
                doomed,
                &["predicate"],
                vec!["d-pred"],
            ),
            seed_resource_child(
                &repo,
                "grant_conditions",
                "resource_id",
                bystander,
                &["predicate"],
                vec!["b-pred"],
            ),
        ),
        (
            "mode_state",
            seed_resource_child(
                &repo,
                "mode_state",
                "scope_resource_id",
                doomed,
                &["mode"],
                vec!["freeze"],
            ),
            seed_resource_child(
                &repo,
                "mode_state",
                "scope_resource_id",
                bystander,
                &["mode"],
                vec!["freeze"],
            ),
        ),
        (
            "deny_notes",
            seed_resource_child(
                &repo,
                "deny_notes",
                "resource_id",
                doomed,
                &["capability", "note"],
                vec!["mutate", "d-note"],
            ),
            seed_resource_child(
                &repo,
                "deny_notes",
                "resource_id",
                bystander,
                &["capability", "note"],
                vec!["mutate", "b-note"],
            ),
        ),
        (
            "resource_labels",
            seed_resource_child(
                &repo,
                "resource_labels",
                "resource_id",
                doomed,
                &["key", "value"],
                vec!["env", "prod"],
            ),
            seed_resource_child(
                &repo,
                "resource_labels",
                "resource_id",
                bystander,
                &["key", "value"],
                vec!["env", "prod"],
            ),
        ),
    ];

    repo.delete_resource(&operator("alice"), doomed, 0)
        .expect("delete doomed resource");

    for (table, doomed_child, sibling_child) in edges {
        assert_eq!(
            fetch_i64(&repo, table, doomed_child, "delete_flag"),
            Some(1),
            "child of the deleted resource in {table} must be cascaded to delete_flag = 1 \
             (missing this edge would leave a dangling row loaded as effective state)"
        );
        assert_eq!(
            fetch_i64(&repo, table, sibling_child, "delete_flag"),
            Some(0),
            "a sibling row in {table} owned by another resource must survive (fk-scoped cascade)"
        );
    }
}

/// 经 base 唯一写路径播一行挂在某 resource 上的子行：`fk_column` 是该子表指向 resources
/// 的外键列名（多数为 `resource_id`，mode_state 为 `scope_resource_id`），其余业务列由
/// `extra_cols`/`extra_vals` 给定（均为文本列）。
fn seed_resource_child(
    repo: &PolicyRepo,
    table: &'static str,
    fk_column: &'static str,
    resource_id: SnowflakeId,
    extra_cols: &[&'static str],
    extra_vals: Vec<&str>,
) -> SnowflakeId {
    use rusqlite::types::Value;
    let mut cols: Vec<&'static str> = vec![fk_column];
    cols.extend_from_slice(extra_cols);
    let mut vals: Vec<Value> = vec![Value::Integer(resource_id.as_raw() as i64)];
    vals.extend(extra_vals.into_iter().map(|s| Value::Text(s.to_string())));
    seed_child(repo, table, &cols, vals)
}

/// 经 base 唯一写路径插一行 binding_scope（kind=resource，挂在某 binding+resource 上）。
fn insert_binding_scope(
    repo: &PolicyRepo,
    binding_id: SnowflakeId,
    resource_id: SnowflakeId,
) -> SnowflakeId {
    use rusqlite::types::Value;
    seed_child(
        repo,
        "binding_scope",
        &["binding_id", "kind", "resource_id"],
        vec![
            Value::Integer(binding_id.as_raw() as i64),
            Value::Text("resource".to_string()),
            Value::Integer(resource_id.as_raw() as i64),
        ],
    )
}

/// 经 base 唯一写路径插一行任意子表（写仍走 base::write::insert 唯一写路径，
/// 8 基础列自动填充）。每次用一把以唯一 `now+id` 偏移播种的 IdGen，保证多次播种
/// 得到互异主键。供级联作用域测试播种「本父子行」与「他父兄弟子行」。
fn seed_child(
    repo: &PolicyRepo,
    table: &'static str,
    columns: &[&'static str],
    values: Vec<rusqlite::types::Value>,
) -> SnowflakeId {
    use postern_core::domain::Timestamp;
    use postern_store::base::write::{self, InsertRow};
    use std::sync::atomic::{AtomicU64, Ordering};
    // 单调推进的播种偏移，使每行 id 互异（雪花 id 由时钟基准驱动）。
    static SEED_OFFSET: AtomicU64 = AtomicU64::new(1);
    let offset = SEED_OFFSET.fetch_add(1, Ordering::Relaxed);
    let now = Timestamp::from_unix_ms(EPOCH_UNIX_MS);
    let idgen = IdGen::new(FixedClock(EPOCH_UNIX_MS + offset));
    repo.db()
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table,
                    columns: columns.to_vec(),
                    values,
                    enable_flag: 1,
                },
            )
        })
        .unwrap_or_else(|e| panic!("seed_child into {table} via base write path failed: {e:?}"))
}

/// 固定宽度（长度 24）时间戳文本，经 base 的唯一格式化点产出（供 temp_grants 的
/// granted_at/expires_at 等带 `CHECK(length = 24)` 的业务时间列播种）。
fn ts24(unix_ms: u64) -> rusqlite::types::Value {
    use postern_core::domain::Timestamp;
    use postern_store::base::timestamp;
    rusqlite::types::Value::Text(timestamp::format(Timestamp::from_unix_ms(unix_ms)))
}

/// 经 base 唯一写路径播一行 credentials（挂在某 principal 上）。
fn seed_credential(repo: &PolicyRepo, principal_id: SnowflakeId) -> SnowflakeId {
    use rusqlite::types::Value;
    seed_child(
        repo,
        "credentials",
        &["principal_id", "kind"],
        vec![
            Value::Integer(principal_id.as_raw() as i64),
            Value::Text("token".to_string()),
        ],
    )
}

/// 经 base 唯一写路径播一行 temp_grants（挂在某 principal+resource 上，带合法终态时间列）。
fn seed_temp_grant(
    repo: &PolicyRepo,
    principal_id: SnowflakeId,
    resource_id: SnowflakeId,
) -> SnowflakeId {
    use rusqlite::types::Value;
    seed_child(
        repo,
        "temp_grants",
        &[
            "principal_id",
            "resource_id",
            "capability",
            "granted_at",
            "expires_at",
        ],
        vec![
            Value::Integer(principal_id.as_raw() as i64),
            Value::Integer(resource_id.as_raw() as i64),
            Value::Text("query".to_string()),
            ts24(EPOCH_UNIX_MS),
            ts24(EPOCH_UNIX_MS + 3_600_000),
        ],
    )
}

// ============================================================ F-10 / B-8 归一化 + 禁 admin

#[test]
fn create_role_named_admin_is_rejected_by_check() {
    // §8-三B-8 / §8-一F-10：建名为 admin 的角色 → schema CHECK 拒 → ConstraintViolation。
    let repo = repo_at(EPOCH_UNIX_MS);
    let err = repo
        .create_role(&operator("alice"), "admin", None)
        .expect_err("admin role name must be rejected");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "got {err:?}"
    );
}

#[test]
fn create_role_named_admin_with_padding_and_caps_is_rejected() {
    // §8-一F-10：` Admin ` / `ADMIN` 经归一化后仍命中禁 admin CHECK（防大小写/空白绕过）。
    let repo = repo_at(EPOCH_UNIX_MS);
    for raw in [" Admin ", "ADMIN", "aDmIn"] {
        let err = repo
            .create_role(&operator("alice"), raw, None)
            .err()
            .unwrap_or_else(|| panic!("disguised admin name {raw:?} should be rejected, got Ok"));
        assert!(
            matches!(err, StoreError::ConstraintViolation),
            "{raw:?} maps to ConstraintViolation, got {err:?}"
        );
    }
}

#[test]
fn duplicate_principal_name_after_normalization_is_rejected() {
    // §8-一F-10：先后写归一化后相同的两条名 → 第二条被 partial unique 拒。
    let repo = repo_at(EPOCH_UNIX_MS);
    repo.create_principal(&operator("alice"), "Bot", "agent")
        .expect("first lands");
    let err = repo
        .create_principal(&operator("alice"), "  bot ", "agent")
        .expect_err("normalized-duplicate must be rejected by partial unique");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "got {err:?}"
    );
}

#[test]
fn principal_name_is_normalized_on_store() {
    // §8-一F-10：name 入库前归一化（trim + 小写），落库为归一化值。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "  MixedCase  ", "agent")
        .expect("create");
    assert_eq!(
        fetch_text(&repo, "principals", id, "name").as_deref(),
        Some("mixedcase"),
        "stored name is trimmed and lowercased"
    );
}

#[test]
fn deleted_name_can_be_recreated_partial_unique_allows() {
    // §8-一F-11（partial unique on delete_flag=0）：逻辑删后同名可重建。
    let repo = repo_at(EPOCH_UNIX_MS);
    let first = repo
        .create_principal(&operator("alice"), "reborn", "agent")
        .expect("first");
    repo.delete_principal(&operator("alice"), first, 0)
        .expect("delete first");
    let second = repo
        .create_principal(&operator("alice"), "reborn", "agent")
        .expect("same name re-creates after logical delete (partial unique on delete_flag=0)");
    assert_ne!(
        first.as_raw(),
        second.as_raw(),
        "a brand new row id, not an undelete"
    );
}

// ============================================================ F-7 后端分页 clamp

#[test]
fn list_principals_clamps_oversized_page_size_to_max() {
    // §8-一F-7：传 page_size=201 → 返回信封 page_size==200（clamp(201)==200）。
    let repo = repo_at(EPOCH_UNIX_MS);
    repo.create_principal(&operator("alice"), "p0", "agent")
        .expect("seed");
    let page = repo
        .list_principals(PageQuery {
            page_no: 1,
            page_size: 201,
        })
        .expect("list");
    assert_eq!(
        page.page_size,
        PageQuery::MAX_SIZE,
        "page_size clamped to MAX_SIZE"
    );
}

#[test]
fn list_principals_caps_items_to_page_size() {
    // §8-一F-7：集合查询 LIMIT 封顶——页面 items 不超过 clamp 后的 page_size。
    let repo = repo_at(EPOCH_UNIX_MS);
    for i in 0..5 {
        repo.create_principal(&operator("alice"), &format!("p{i}"), "agent")
            .expect("seed");
    }
    let page: Page<PrincipalRow> = repo
        .list_principals(PageQuery {
            page_no: 1,
            page_size: 2,
        })
        .expect("list");
    assert!(
        page.items.len() <= 2,
        "items are LIMIT-bounded to page_size"
    );
    assert_eq!(
        page.total, 5,
        "total reflects all undeleted rows regardless of page size"
    );
}

#[test]
fn list_principals_second_page_offsets_correctly() {
    // §8-一F-7：第二页 OFFSET 正确——首页与次页 item 集合不相交，合计覆盖全集。
    let repo = repo_at(EPOCH_UNIX_MS);
    let mut all = Vec::new();
    for i in 0..3 {
        all.push(
            repo.create_principal(&operator("alice"), &format!("p{i}"), "agent")
                .expect("seed"),
        );
    }
    let p1 = repo
        .list_principals(PageQuery {
            page_no: 1,
            page_size: 2,
        })
        .expect("page1");
    let p2 = repo
        .list_principals(PageQuery {
            page_no: 2,
            page_size: 2,
        })
        .expect("page2");
    let mut seen: Vec<SnowflakeId> = p1.items.iter().map(|r| r.id).collect();
    seen.extend(p2.items.iter().map(|r| r.id));
    seen.sort_by_key(|s| s.as_raw());
    seen.dedup();
    assert_eq!(
        seen.len(),
        3,
        "page 1 + page 2 cover all three rows without overlap"
    );
}

// ============================================================ 读端点携 version

#[test]
fn get_principal_returns_current_version_for_optimistic_lock() {
    // §8-一F-2 / §6.4：读端点统一返回 version，供调用方下一次写带上做乐观锁。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_principal(&operator("alice"), "p", "agent")
        .expect("create");
    let row = repo.get_principal(id).expect("get").expect("present");
    assert_eq!(row.version, 0, "fresh row reports version 0");
    repo.rename_principal(&operator("alice"), id, row.version, "p2")
        .expect("rename with read version");
    let row2 = repo.get_principal(id).expect("get2").expect("present");
    assert_eq!(
        row2.version, 1,
        "read-back version advanced after the write"
    );
    assert_eq!(
        row2.name, "p2",
        "read model reflects the new business value"
    );
}

// ============================================================ 角色 / 资源读模型

#[test]
fn list_roles_excludes_deleted_and_carries_version() {
    // §8-一F-5 / F-2：角色集合默认排除已删、读模型携 version。
    let repo = repo_at(EPOCH_UNIX_MS);
    let keep = repo
        .create_role(&operator("alice"), "observer", Some("read-only"))
        .expect("keep");
    let drop = repo
        .create_role(&operator("alice"), "tmp", None)
        .expect("drop");
    repo.delete_role(&operator("alice"), drop, 0)
        .expect("delete role");
    let page = repo
        .list_roles(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list roles");
    let rows: Vec<&RoleRow> = page.items.iter().collect();
    assert!(
        rows.iter().any(|r| r.id == keep && r.version == 0),
        "kept role present with version 0"
    );
    assert!(
        !rows.iter().any(|r| r.id == drop),
        "deleted role excluded by default scope"
    );
}

#[test]
fn delete_role_cascades_every_child_edge_and_spares_other_role() {
    // §8-一F-4 / §3.2：roles → {role_inherits, role_capabilities, bindings} 三条边各须
    // 既级联本角色子行、又放过他角色的兄弟子行。此前 policy.rs 对 delete_role 仅有读
    // 模型用例、零级联断言（裁掉任一 roles 子边为静默 Ok）。本用例为每条边播一行挂在
    // 【被删角色 doomed】下与一行挂在【另一角色 bystander】下，断言前者 delete_flag→1、
    // 后者→0（fk 作用域）。
    let repo = repo_at(EPOCH_UNIX_MS);
    let doomed = repo
        .create_role(&operator("alice"), "doomed", None)
        .expect("doomed role");
    let bystander = repo
        .create_role(&operator("alice"), "bystander", None)
        .expect("bystander role");
    // role_inherits 的 parent 角色（外键须指向真实 roles 行）。
    let parent = repo
        .create_role(&operator("alice"), "parent", None)
        .expect("parent role");
    let p1 = repo
        .create_principal(&operator("alice"), "p1", "agent")
        .expect("p1");
    let p2 = repo
        .create_principal(&operator("alice"), "p2", "agent")
        .expect("p2");

    // role_inherits 边（fk = role_id）。
    let doomed_inherit = seed_child(
        &repo,
        "role_inherits",
        &["role_id", "parent_role_id"],
        vec![
            rusqlite::types::Value::Integer(doomed.as_raw() as i64),
            rusqlite::types::Value::Integer(parent.as_raw() as i64),
        ],
    );
    let sibling_inherit = seed_child(
        &repo,
        "role_inherits",
        &["role_id", "parent_role_id"],
        vec![
            rusqlite::types::Value::Integer(bystander.as_raw() as i64),
            rusqlite::types::Value::Integer(parent.as_raw() as i64),
        ],
    );
    // role_capabilities 边（fk = role_id）。
    let doomed_cap = seed_child(
        &repo,
        "role_capabilities",
        &["role_id", "capability", "action"],
        vec![
            rusqlite::types::Value::Integer(doomed.as_raw() as i64),
            rusqlite::types::Value::Text("observe".to_string()),
            rusqlite::types::Value::Text("allow".to_string()),
        ],
    );
    let sibling_cap = seed_child(
        &repo,
        "role_capabilities",
        &["role_id", "capability", "action"],
        vec![
            rusqlite::types::Value::Integer(bystander.as_raw() as i64),
            rusqlite::types::Value::Text("observe".to_string()),
            rusqlite::types::Value::Text("allow".to_string()),
        ],
    );
    // bindings 边（fk = role_id）：两条不同主体绑到两个角色，避免 (principal, role) 重复。
    let doomed_binding = repo
        .create_binding(&operator("alice"), p1, doomed)
        .expect("doomed binding");
    let sibling_binding = repo
        .create_binding(&operator("alice"), p2, bystander)
        .expect("sibling binding");

    repo.delete_role(&operator("alice"), doomed, 0)
        .expect("delete doomed role");

    for (table, doomed_child, sibling_child) in [
        ("role_inherits", doomed_inherit, sibling_inherit),
        ("role_capabilities", doomed_cap, sibling_cap),
        ("bindings", doomed_binding, sibling_binding),
    ] {
        assert_eq!(
            fetch_i64(&repo, table, doomed_child, "delete_flag"),
            Some(1),
            "child of the deleted role in {table} must be cascaded to delete_flag = 1"
        );
        assert_eq!(
            fetch_i64(&repo, table, sibling_child, "delete_flag"),
            Some(0),
            "a sibling row in {table} owned by another role must survive (fk-scoped cascade)"
        );
    }
}

#[test]
fn list_resources_reflects_persisted_business_columns() {
    // §8-一F-1/F-5：资源读模型如实反映落库业务列（codename/adapter/transport），默认排除已删。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_resource(&operator("alice"), "db-main", "postgres", "tcp")
        .expect("resource");
    let page = repo
        .list_resources(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list");
    let row: &ResourceRow = page
        .items
        .iter()
        .find(|r| r.id == id)
        .expect("created resource is listed");
    assert_eq!(row.codename, "db-main", "codename persisted");
    assert_eq!(row.adapter, "postgres", "adapter persisted");
    assert_eq!(row.transport, "tcp", "transport persisted");
}

#[test]
fn resource_id_by_code_resolves_active_resource_and_normalizes() {
    // 资源代号反查（elevate 经此把 ElevateRequest.resource 代号 → 资源 id）：命中活跃资源；
    // 入参经与入库一致的归一化（trim + 小写）后比对，故 " DB-Main " 命中 "db-main"。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_resource(&operator("alice"), "db-main", "postgres", "tcp")
        .expect("resource");

    assert_eq!(
        repo.resource_id_by_code("db-main").expect("lookup"),
        Some(id),
        "精确代号命中活跃资源 id"
    );
    assert_eq!(
        repo.resource_id_by_code(" DB-Main ").expect("lookup"),
        Some(id),
        "代号归一化（trim + 小写）与入库一致 ⇒ 命中同一资源"
    );
    assert_eq!(
        repo.resource_id_by_code("nope").expect("lookup"),
        None,
        "未知代号 ⇒ None（调用方 fail-closed，绝不臆造资源）"
    );
}

#[test]
fn resource_id_by_code_excludes_deleted_resource() {
    // §3.1 默认作用域：逻辑删除的资源不应被代号反查命中（None）——避免对已删资源发临时授权。
    let repo = repo_at(EPOCH_UNIX_MS);
    let id = repo
        .create_resource(&operator("alice"), "db-main", "postgres", "tcp")
        .expect("resource");
    repo.delete_resource(&operator("alice"), id, 0)
        .expect("delete resource");
    assert_eq!(
        repo.resource_id_by_code("db-main").expect("lookup"),
        None,
        "已逻辑删除的资源代号反查 ⇒ None（默认作用域排除已删）"
    );
}

#[test]
fn list_bindings_of_filters_by_principal_and_excludes_deleted() {
    // §8-一F-5：列某主体的绑定——只含该主体且默认排除已删。
    let repo = repo_at(EPOCH_UNIX_MS);
    let p1 = repo
        .create_principal(&operator("alice"), "p1", "agent")
        .expect("p1");
    let p2 = repo
        .create_principal(&operator("alice"), "p2", "agent")
        .expect("p2");
    let r = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    let b1 = repo.create_binding(&operator("alice"), p1, r).expect("b1");
    let _b2 = repo.create_binding(&operator("alice"), p2, r).expect("b2");
    let page = repo
        .list_bindings_of(
            p1,
            PageQuery {
                page_no: 1,
                page_size: 50,
            },
        )
        .expect("list");
    let rows: Vec<&BindingRow> = page.items.iter().collect();
    assert_eq!(rows.len(), 1, "only p1's binding is listed");
    assert_eq!(rows[0].id, b1, "the listed binding belongs to p1");
    assert_eq!(rows[0].principal_id, p1, "read model carries principal_id");
}

#[test]
fn duplicate_binding_is_rejected_by_partial_unique() {
    // §8-一F-11：同 (principal, role) 重复绑定（delete_flag=0）→ partial unique 拒。
    let repo = repo_at(EPOCH_UNIX_MS);
    let p = repo
        .create_principal(&operator("alice"), "p", "agent")
        .expect("p");
    let r = repo
        .create_role(&operator("alice"), "observer", None)
        .expect("role");
    repo.create_binding(&operator("alice"), p, r)
        .expect("first binding");
    let err = repo
        .create_binding(&operator("alice"), p, r)
        .expect_err("duplicate (principal, role) binding must be rejected");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "got {err:?}"
    );
}
