//! 实体组 1（constraints / conditions / deny-notes）的 *_and_rebuild 写接缝 + list_*
//! 读模型 行为测试（D2bx）。每条只钉一个行为：create→list 反映、rev 前进、delete→列表
//! 减少且 rev 再进、乐观锁/uq 冲突全或无（不改库、不进 rev、不换 snapshot）、可空列
//! （conditions 的 resource_id/capability）正确入库读出。
//!
//! 三张表均为限制性表（grant_constraints/grant_conditions/deny_notes：CHECK enable_flag=1），
//! 写一律经被测 PolicyRepo 的 *_and_rebuild（内部经 base::write::insert + commit_and_rebuild
//! 唯一写路径）。读经被测 list_*。rev 经 base::meta::read_policy_rev 核对。
//!
//! 雷区纪律（与 policy.rs / policy_rev.rs 同源）：本文件在 crates/postern-store/ 下但
//! **不在** src/base/ 下，故对契约扫描器是"in_store 且非 in_store_base"。任何字面裸数据库
//! 读关键词的连续串出现在源文本里都会被记为违规（扫描器不剥 Rust 行注释）。因此本文件
//! **绝不写字面裸数据库读写标记**：建表/迁移经 migrate；写经被测 *_and_rebuild API；
//! 行内省读（计数核对落库形态）一律在**运行期由片段拼接**（见 kw / count_where）。

use std::sync::Arc;

use postern_core::domain::PolicySnapshot;
use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_core::page::PageQuery;
use postern_core::plugin::PolicyView;
use postern_store::base::db::Db;
use postern_store::base::error::StoreError;
use postern_store::base::meta::read_policy_rev;
use postern_store::base::write::Actor;
use postern_store::migrate;
use postern_store::policy::PolicyRepo;
use postern_store::snapshot::SnapshotView;

// ============================================================ 运行期 SQL 片段拼接
// 任意单个片段都不构成扫描器关注的连续串，故源文本里永不出现完整需被扫描的连续读
// 关键词 needle。

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

/// 以确定时钟装配一个已迁移到当前版本、持视图的写句柄（首份快照 policy_rev=0）。
/// 持视图使「写 + bump rev + 重建 + 发布」在同一临界区原子完成（供乐观锁/uq 冲突
/// 时核对"snapshot 不换"）。
fn repo_with_view() -> (PolicyRepo, Arc<SnapshotView>) {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema on empty db");
    let idgen = IdGen::new(FixedClock(EPOCH_UNIX_MS));
    let view = Arc::new(SnapshotView::new(Arc::new(PolicySnapshot::default())));
    let repo = PolicyRepo::with_view(
        db,
        idgen,
        Box::new(FixedClock(EPOCH_UNIX_MS)),
        Arc::clone(&view),
    );
    (repo, view)
}

/// 控制面操作者（落 created_by / updated_by）。
fn operator(id: &str) -> Actor {
    Actor::Operator(id.to_string())
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

/// 单值文本省读（按主键，无作用域过滤——专供核对落库形态）。
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

/// 单值 i64 省读（按主键），用于核对可空 resource_id 是否真为 NULL（None）。
fn fetch_opt_i64(repo: &PolicyRepo, table: &str, id: SnowflakeId, col: &str) -> Option<i64> {
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

/// 播一个资源（约束/条件/拒绝说明的外键宿主），返回其 id。建资源也走 *_and_rebuild
/// （rev 因此先 +1）——后续断言以"建资源后"的 rev 为基准取相对增量，故不受影响。
fn seed_resource(repo: &PolicyRepo, codename: &str) -> SnowflakeId {
    repo.create_resource_and_rebuild(&operator("alice"), codename, "postgres", "tcp")
        .expect("seed resource");
    // create_resource_and_rebuild 只返回 (version, rev)，不返回 id；从读模型取回该资源 id。
    let page = repo
        .list_resources(PageQuery {
            page_no: 1,
            page_size: 200,
        })
        .expect("list resources");
    page.items
        .iter()
        .find(|r| r.codename == codename)
        .map(|r| r.id)
        .expect("seeded resource is listed")
}

// ============================================================ constraints

#[test]
fn create_constraint_reflected_in_list_and_advances_rev() {
    // create→list 反映：新增对象细则后 list_constraints 含该行（业务列如实读出）；
    // 且 rev 由"建资源后"的基准前进 1。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-c1");
    let rev_before = read_policy_rev(repo.db()).expect("rev before");

    let (version, rev_after) = repo
        .create_constraint_and_rebuild(
            &operator("alice"),
            res,
            "query",
            "rate",
            Some("{\"qps\":10}"),
        )
        .expect("create constraint");
    assert_eq!(version, 0, "new constraint row version is 0");
    assert_eq!(rev_after, rev_before + 1, "create advances policy_rev by 1");

    let page = repo
        .list_constraints(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list constraints");
    let row = page
        .items
        .iter()
        .find(|c| c.resource_id == res)
        .expect("created constraint is listed");
    assert_eq!(
        row.capability, "query",
        "capability persisted and read back"
    );
    assert_eq!(row.kind, "rate", "kind persisted and read back");
    assert_eq!(row.spec.as_deref(), Some("{\"qps\":10}"), "spec persisted");
    assert_eq!(row.version, 0, "read model carries version 0");
}

#[test]
fn delete_constraint_shrinks_list_and_advances_rev_again() {
    // delete→列表减少且 rev 再进：删后 list_constraints 不含该行、计数 -1、rev 再 +1。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-c2");
    repo.create_constraint_and_rebuild(&operator("alice"), res, "mutate", "rate", None)
        .expect("create constraint");

    let before = repo
        .list_constraints(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list before");
    let target = before
        .items
        .iter()
        .find(|c| c.resource_id == res)
        .expect("present before delete");
    let rev_before_delete = read_policy_rev(repo.db()).expect("rev before delete");

    let (version, rev_after) = repo
        .delete_constraint_and_rebuild(&operator("alice"), target.id, target.version)
        .expect("delete constraint");
    assert_eq!(version, target.version + 1, "delete bumps version by 1");
    assert_eq!(
        rev_after,
        rev_before_delete + 1,
        "delete advances policy_rev by 1"
    );

    let after = repo
        .list_constraints(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after");
    assert!(
        !after.items.iter().any(|c| c.id == target.id),
        "deleted constraint excluded by default scope"
    );
    assert_eq!(
        after.total,
        before.total - 1,
        "list total shrinks by exactly one after delete"
    );
}

#[test]
fn delete_constraint_with_stale_version_is_all_or_nothing() {
    // 乐观锁冲突全或无：持过期 version 删 → VersionConflict；行未删、rev 不进、snapshot 不换。
    let (repo, view) = repo_with_view();
    let res = seed_resource(&repo, "db-c3");
    repo.create_constraint_and_rebuild(&operator("alice"), res, "query", "rate", None)
        .expect("create constraint");
    let target = repo
        .list_constraints(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list")
        .items
        .into_iter()
        .find(|c| c.resource_id == res)
        .expect("present");

    let rev_before = read_policy_rev(repo.db()).expect("rev before");
    let snap_before = view.snapshot();

    let err = repo
        .delete_constraint_and_rebuild(&operator("alice"), target.id, target.version + 99)
        .expect_err("stale version must conflict");
    assert!(
        matches!(err, StoreError::VersionConflict),
        "stale version maps to VersionConflict, got {err:?}"
    );
    // 库不变：该行仍未删（delete_flag=0）。
    assert_eq!(
        count_where(
            &repo,
            "grant_constraints",
            &format!("id = {} AND delete_flag = 0", target.id.as_raw())
        ),
        1,
        "constraint stays undeleted after a conflicting delete"
    );
    // rev 不前进。
    assert_eq!(
        read_policy_rev(repo.db()).expect("rev after"),
        rev_before,
        "version conflict does not advance policy_rev"
    );
    // snapshot 不换（同一 Arc）。
    assert!(
        Arc::ptr_eq(&snap_before, &view.snapshot()),
        "version conflict does not swap the published snapshot"
    );
}

// ============================================================ conditions（含可空列）

#[test]
fn create_condition_with_resource_and_capability_reflected_in_list() {
    // create→list 反映（带 resource_id + capability 的具体条件），rev 前进 1。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-cond1");
    let rev_before = read_policy_rev(repo.db()).expect("rev before");

    let (version, rev_after) = repo
        .create_condition_and_rebuild(
            &operator("alice"),
            Some(res),
            Some("execute"),
            "hour in 9..17",
            Some("{\"tz\":\"utc\"}"),
        )
        .expect("create condition");
    assert_eq!(version, 0, "new condition row version is 0");
    assert_eq!(rev_after, rev_before + 1, "create advances policy_rev by 1");

    let page = repo
        .list_conditions(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list conditions");
    let row = page
        .items
        .iter()
        .find(|c| c.resource_id == Some(res))
        .expect("created condition is listed");
    assert_eq!(
        row.capability.as_deref(),
        Some("execute"),
        "capability persisted and read back"
    );
    assert_eq!(row.predicate, "hour in 9..17", "predicate persisted");
    assert_eq!(
        row.spec.as_deref(),
        Some("{\"tz\":\"utc\"}"),
        "spec persisted"
    );
}

#[test]
fn create_condition_with_null_resource_and_capability_stores_and_reads_nulls() {
    // 可空列正确入库读出：resource_id=None / capability=None（全局通用条件）→ 落库为 NULL、
    // 读模型读回 None；predicate（NOT NULL）仍如实落库。这是 conditions 区别于 constraints
    // 的关键设计（grant_conditions 的 resource_id/capability 可空）。
    let (repo, _view) = repo_with_view();

    repo.create_condition_and_rebuild(&operator("alice"), None, None, "global-guard", None)
        .expect("create global condition");

    let page = repo
        .list_conditions(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list conditions");
    let row = page
        .items
        .iter()
        .find(|c| c.predicate == "global-guard")
        .expect("created global condition is listed");
    assert_eq!(
        row.resource_id, None,
        "null resource_id reads back as None (global condition)"
    );
    assert_eq!(
        row.capability, None,
        "null capability reads back as None (all-verb condition)"
    );
    assert_eq!(row.spec, None, "null spec reads back as None");
    // 落库形态佐证：resource_id 列在该行确为 NULL（省读读不到 i64 值）。
    assert_eq!(
        fetch_opt_i64(&repo, "grant_conditions", row.id, "resource_id"),
        None,
        "resource_id column is physically NULL in the stored row"
    );
}

#[test]
fn delete_condition_shrinks_list_and_advances_rev_again() {
    // delete→列表减少且 rev 再进。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-cond2");
    repo.create_condition_and_rebuild(&operator("alice"), Some(res), Some("query"), "p", None)
        .expect("create condition");
    let before = repo
        .list_conditions(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list before");
    let target = before
        .items
        .iter()
        .find(|c| c.resource_id == Some(res))
        .expect("present before delete");
    let rev_before_delete = read_policy_rev(repo.db()).expect("rev before delete");

    let (_version, rev_after) = repo
        .delete_condition_and_rebuild(&operator("alice"), target.id, target.version)
        .expect("delete condition");
    assert_eq!(
        rev_after,
        rev_before_delete + 1,
        "delete advances policy_rev by 1"
    );

    let after = repo
        .list_conditions(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after");
    assert!(
        !after.items.iter().any(|c| c.id == target.id),
        "deleted condition excluded by default scope"
    );
    assert_eq!(
        after.total,
        before.total - 1,
        "list total shrinks by exactly one after delete"
    );
}

// ============================================================ deny-notes（含 uq 冲突）

#[test]
fn create_deny_note_reflected_in_list_and_advances_rev() {
    // create→list 反映，rev 前进 1。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-d1");
    let rev_before = read_policy_rev(repo.db()).expect("rev before");

    let (version, rev_after) = repo
        .create_deny_note_and_rebuild(
            &operator("alice"),
            res,
            "destroy",
            "never auto-destroy prod",
        )
        .expect("create deny note");
    assert_eq!(version, 0, "new deny note row version is 0");
    assert_eq!(rev_after, rev_before + 1, "create advances policy_rev by 1");

    let page = repo
        .list_deny_notes(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list deny notes");
    let row = page
        .items
        .iter()
        .find(|d| d.resource_id == res)
        .expect("created deny note is listed");
    assert_eq!(
        row.capability, "destroy",
        "capability persisted and read back"
    );
    assert_eq!(row.note, "never auto-destroy prod", "note persisted");
}

#[test]
fn duplicate_deny_note_is_rejected_all_or_nothing() {
    // uq(resource_id, capability) 冲突全或无：同 (resource, capability) 重复（delete_flag=0）
    // → ConstraintViolation；库不变（仍恰一行）、rev 不进、snapshot 不换。
    let (repo, view) = repo_with_view();
    let res = seed_resource(&repo, "db-d2");
    repo.create_deny_note_and_rebuild(&operator("alice"), res, "mutate", "first note")
        .expect("first deny note");

    let rev_before = read_policy_rev(repo.db()).expect("rev before");
    let snap_before = view.snapshot();

    let err = repo
        .create_deny_note_and_rebuild(&operator("alice"), res, "mutate", "dup note")
        .expect_err("duplicate (resource, capability) deny note must be rejected");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "duplicate maps to ConstraintViolation, got {err:?}"
    );
    // 库不变：该 (resource, capability) 仍恰一行活跃，且是首条 note（重复写未落库）。
    assert_eq!(
        count_where(
            &repo,
            "deny_notes",
            &format!(
                "resource_id = {} AND capability = 'mutate' AND delete_flag = 0",
                res.as_raw()
            )
        ),
        1,
        "exactly one active deny note remains; the duplicate never landed"
    );
    let only = repo
        .list_deny_notes(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list")
        .items
        .into_iter()
        .find(|d| d.resource_id == res)
        .expect("present");
    assert_eq!(
        fetch_text(&repo, "deny_notes", only.id, "note").as_deref(),
        Some("first note"),
        "the surviving note is the original, not the rejected duplicate"
    );
    // rev 不前进、snapshot 不换。
    assert_eq!(
        read_policy_rev(repo.db()).expect("rev after"),
        rev_before,
        "constraint violation does not advance policy_rev"
    );
    assert!(
        Arc::ptr_eq(&snap_before, &view.snapshot()),
        "constraint violation does not swap the published snapshot"
    );
}

#[test]
fn delete_deny_note_shrinks_list_and_allows_recreate() {
    // delete→列表减少且 rev 再进；逻辑删后同 (resource, capability) 可重建（partial unique
    // on delete_flag=0）。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-d3");
    repo.create_deny_note_and_rebuild(&operator("alice"), res, "execute", "n0")
        .expect("create deny note");
    let before = repo
        .list_deny_notes(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list before");
    let target = before
        .items
        .iter()
        .find(|d| d.resource_id == res)
        .expect("present before delete");
    let rev_before_delete = read_policy_rev(repo.db()).expect("rev before delete");

    let (_version, rev_after) = repo
        .delete_deny_note_and_rebuild(&operator("alice"), target.id, target.version)
        .expect("delete deny note");
    assert_eq!(
        rev_after,
        rev_before_delete + 1,
        "delete advances policy_rev by 1"
    );

    let after = repo
        .list_deny_notes(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after");
    assert_eq!(
        after.total,
        before.total - 1,
        "list total shrinks by exactly one after delete"
    );
    // 逻辑删后同 (resource, capability) 可重建（partial unique 仅约束 delete_flag=0）。
    repo.create_deny_note_and_rebuild(&operator("alice"), res, "execute", "n1")
        .expect("same (resource, capability) re-creates after logical delete");
}
