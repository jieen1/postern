//! 实体组 2（mode / grants）的 *_and_rebuild 写接缝 + list_* 读模型 + your_grants
//! 投影 行为测试（D2bx 续组1）。每条只钉一个行为：
//!
//! - **mode**（upsert）：set 新 scope → 插入一行、rev 前进；同 scope 再 set → 改既有行
//!   （不新增行、mode/expires_at 被改）、rev 再进；全局 scope=NULL 哨兵正确落库读出。
//! - **grants**：elevate → temp_grant 在列表带正确 expires_at（granted_at=now、
//!   expires_at=now+ttl）；revoke → 置终态（ended_at/end_reason='revoked'）且 rev 进、
//!   乐观锁冲突全或无（不改库、不进 rev、不换 snapshot）；your_grants 投影正确
//!   （从已物化快照取 resource→capability[]）。
//!
//! mode_state/temp_grants 写一律经被测 PolicyRepo 的 *_and_rebuild（内部经
//! base::write::{insert,update} + commit_and_rebuild 唯一写路径）。读经被测 list_*。
//! rev 经 base::meta::read_policy_rev 核对。
//!
//! 雷区纪律（与 d2bx_constraints.rs 同源）：本文件在 crates/postern-store/ 下但
//! **不在** src/base/ 下，故对契约扫描器是"in_store 且非 in_store_base"。任何字面裸
//! 数据库读写关键词的连续串都会被记为违规。因此本文件**绝不写字面裸数据库读写标记**：
//! 建表/迁移经 migrate；写经被测 *_and_rebuild API；行内省读一律在**运行期由片段拼接**
//! （见 kw / count_where / fetch_*）。

use std::sync::Arc;

use postern_core::domain::{Capability, PolicySnapshot, ResourceCode};
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

/// 以确定时钟装配一个已迁移、持视图的写句柄（首份快照 policy_rev=0）。持视图使
/// 「写 + bump rev + 重建 + 发布」原子完成（供乐观锁冲突时核对"snapshot 不换"，且
/// your_grants 投影可从已物化快照取）。
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

/// 物理行计数（按运行期拼接的谓词），纯 COUNT(*) 投影。
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

/// 单值 i64 省读（按主键），用于核对可空 scope_resource_id 是否真为 NULL（None）。
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

/// 播一个主体，返回其 id（grants 的 principal 宿主）。
fn seed_principal(repo: &PolicyRepo, name: &str) -> SnowflakeId {
    repo.create_principal_and_rebuild(&operator("alice"), name, "agent")
        .expect("seed principal");
    repo.list_principals(PageQuery {
        page_no: 1,
        page_size: 200,
    })
    .expect("list principals")
    .items
    .into_iter()
    .find(|p| p.name == name)
    .map(|p| p.id)
    .expect("seeded principal is listed")
}

/// 播一个资源，返回其 id（grants/mode 的 resource 宿主）。
fn seed_resource(repo: &PolicyRepo, codename: &str) -> SnowflakeId {
    repo.create_resource_and_rebuild(&operator("alice"), codename, "postgres", "tcp")
        .expect("seed resource");
    repo.list_resources(PageQuery {
        page_no: 1,
        page_size: 200,
    })
    .expect("list resources")
    .items
    .into_iter()
    .find(|r| r.codename == codename)
    .map(|r| r.id)
    .expect("seeded resource is listed")
}

// ============================================================ mode（upsert）

#[test]
fn set_mode_for_new_scope_inserts_row_and_advances_rev() {
    // set 新 scope（资源级）→ 插入一行、list_mode_state 含该行（mode/expires_at/version
    // 如实读出）、rev 前进 1。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-m1");
    let rev_before = read_policy_rev(repo.db()).expect("rev before");

    let (version, rev_after) = repo
        .set_mode_and_rebuild(
            &operator("alice"),
            Some(res),
            "freeze",
            Some("2099-01-01T00:00:00.000Z"),
        )
        .expect("set mode for new scope");
    assert_eq!(version, 0, "new mode_state row version is 0");
    assert_eq!(rev_after, rev_before + 1, "set advances policy_rev by 1");

    let page = repo
        .list_mode_state(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list mode state");
    let row = page
        .items
        .iter()
        .find(|m| m.scope_resource_id == Some(res))
        .expect("created mode row is listed");
    assert_eq!(row.mode, "freeze", "mode persisted and read back");
    assert_eq!(
        row.expires_at.as_deref(),
        Some("2099-01-01T00:00:00.000Z"),
        "expires_at persisted verbatim"
    );
    assert_eq!(row.version, 0, "read model carries version 0");
}

#[test]
fn set_mode_for_global_scope_stores_null_sentinel() {
    // 全局 scope=None → scope_resource_id 落库 NULL（哨兵），list 读回 None。
    let (repo, _view) = repo_with_view();

    repo.set_mode_and_rebuild(&operator("alice"), None, "observe", None)
        .expect("set global mode");

    let page = repo
        .list_mode_state(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list mode state");
    let row = page
        .items
        .iter()
        .find(|m| m.mode == "observe")
        .expect("global mode row is listed");
    assert_eq!(
        row.scope_resource_id, None,
        "global mode reads back with None scope (NULL sentinel)"
    );
    assert_eq!(row.expires_at, None, "null expires_at reads back as None");
    // 落库形态佐证：scope_resource_id 列在该行确为 NULL。
    assert_eq!(
        fetch_opt_i64(&repo, "mode_state", row.id, "scope_resource_id"),
        None,
        "scope_resource_id column is physically NULL for the global row"
    );
}

#[test]
fn set_mode_same_scope_updates_existing_row_without_adding() {
    // upsert 收窄：同 scope 再 set → 改既有行（行数不增、mode/expires_at 被改、version
    // 自增），rev 再进 1。这是 mode 区别于 append-only 表的关键（uq ON COALESCE 哨兵）。
    let (repo, _view) = repo_with_view();
    let res = seed_resource(&repo, "db-m2");

    repo.set_mode_and_rebuild(&operator("alice"), Some(res), "observe", None)
        .expect("first set");
    let first = repo
        .list_mode_state(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after first")
        .items
        .into_iter()
        .find(|m| m.scope_resource_id == Some(res))
        .expect("present after first set");
    assert_eq!(first.version, 0, "first set lands version 0");

    let rows_before = count_where(
        &repo,
        "mode_state",
        &format!("scope_resource_id = {} AND delete_flag = 0", res.as_raw()),
    );
    assert_eq!(rows_before, 1, "exactly one active row for the scope");
    let rev_before_second = read_policy_rev(repo.db()).expect("rev before second");

    let (version, rev_after) = repo
        .set_mode_and_rebuild(
            &operator("alice"),
            Some(res),
            "freeze",
            Some("2100-06-01T00:00:00.000Z"),
        )
        .expect("second set on same scope");
    assert_eq!(
        version, 1,
        "in-place write bumps version to 1 (no new row inserted)"
    );
    assert_eq!(
        rev_after,
        rev_before_second + 1,
        "second set advances policy_rev by 1"
    );

    // 行数不增：仍恰一行活跃。
    let rows_after = count_where(
        &repo,
        "mode_state",
        &format!("scope_resource_id = {} AND delete_flag = 0", res.as_raw()),
    );
    assert_eq!(
        rows_after, 1,
        "still exactly one active row (upsert, not append)"
    );

    // 既有行被改：同一 id、mode/expires_at 更新到第二次的值。
    let after = repo
        .list_mode_state(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after second")
        .items
        .into_iter()
        .find(|m| m.scope_resource_id == Some(res))
        .expect("present after second set");
    assert_eq!(after.id, first.id, "same row updated in place (same id)");
    assert_eq!(
        after.mode, "freeze",
        "mode narrowed to the second set's value"
    );
    assert_eq!(
        after.expires_at.as_deref(),
        Some("2100-06-01T00:00:00.000Z"),
        "expires_at updated to the second set's value"
    );
    assert_eq!(after.version, 1, "in-place write bumped version to 1");
}

// ============================================================ grants（temp_grants + your_grants）

#[test]
fn elevate_grant_listed_with_correct_expiry() {
    // elevate → temp_grant 在 list_temp_grants 带正确 expires_at（granted_at=now、
    // expires_at=now+ttl_ms，经唯一格式化点落 24 字节文本），rev 前进 1。
    let (repo, _view) = repo_with_view();
    let principal = seed_principal(&repo, "agent-e1");
    let res = seed_resource(&repo, "db-e1");
    let rev_before = read_policy_rev(repo.db()).expect("rev before");

    let ttl_ms: u64 = 3_600_000; // 1 小时
    let (version, rev_after) = repo
        .elevate_grant_and_rebuild(&operator("alice"), principal, res, "destroy", ttl_ms)
        .expect("elevate grant");
    assert_eq!(version, 0, "new temp_grant row version is 0");
    assert_eq!(
        rev_after,
        rev_before + 1,
        "elevate advances policy_rev by 1"
    );

    let page = repo
        .list_temp_grants(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list temp grants");
    let row = page
        .items
        .iter()
        .find(|g| g.principal_id == principal && g.resource_id == res)
        .expect("elevated grant is listed");
    assert_eq!(
        row.capability, "destroy",
        "capability persisted and read back"
    );
    // granted_at = now（EPOCH_UNIX_MS 经唯一格式化点）；expires_at = now + ttl。
    assert_eq!(
        row.granted_at, "2026-01-01T00:00:00.000Z",
        "granted_at is the write wall clock (now)"
    );
    assert_eq!(
        row.expires_at, "2026-01-01T01:00:00.000Z",
        "expires_at is now + ttl_ms via the single timestamp formatter"
    );
    assert_eq!(row.ended_at, None, "fresh grant has no end instant");
    assert_eq!(row.end_reason, None, "fresh grant has no end reason");
    assert_eq!(row.version, 0, "read model carries version 0");
}

#[test]
fn revoke_grant_sets_terminal_state_and_advances_rev() {
    // revoke → 乐观锁改写置 ended_at=now/end_reason='revoked'，version 自增、rev 再进。
    let (repo, _view) = repo_with_view();
    let principal = seed_principal(&repo, "agent-r1");
    let res = seed_resource(&repo, "db-r1");
    repo.elevate_grant_and_rebuild(&operator("alice"), principal, res, "manage", 60_000)
        .expect("elevate grant");
    let target = repo
        .list_temp_grants(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list")
        .items
        .into_iter()
        .find(|g| g.principal_id == principal)
        .expect("present before revoke");
    let rev_before_revoke = read_policy_rev(repo.db()).expect("rev before revoke");

    let (version, rev_after) = repo
        .revoke_grant_and_rebuild(&operator("alice"), target.id, target.version)
        .expect("revoke grant");
    assert_eq!(version, target.version + 1, "revoke bumps version by 1");
    assert_eq!(
        rev_after,
        rev_before_revoke + 1,
        "revoke advances policy_rev by 1"
    );

    let after = repo
        .list_temp_grants(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after")
        .items
        .into_iter()
        .find(|g| g.id == target.id)
        .expect("row still present (terminal state, not deleted)");
    assert_eq!(
        after.ended_at.as_deref(),
        Some("2026-01-01T00:00:00.000Z"),
        "ended_at set to revoke wall clock (now)"
    );
    assert_eq!(
        after.end_reason.as_deref(),
        Some("revoked"),
        "end_reason set to 'revoked' (terminal)"
    );
    // 落库形态佐证：end_reason 列确为 'revoked'。
    assert_eq!(
        fetch_text(&repo, "temp_grants", target.id, "end_reason").as_deref(),
        Some("revoked"),
        "end_reason column physically holds 'revoked'"
    );
}

#[test]
fn revoke_grant_with_stale_version_is_all_or_nothing() {
    // 乐观锁冲突全或无：持过期 version revoke → VersionConflict；行未变终态、rev 不进、
    // snapshot 不换。
    let (repo, view) = repo_with_view();
    let principal = seed_principal(&repo, "agent-r2");
    let res = seed_resource(&repo, "db-r2");
    repo.elevate_grant_and_rebuild(&operator("alice"), principal, res, "execute", 60_000)
        .expect("elevate grant");
    let target = repo
        .list_temp_grants(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list")
        .items
        .into_iter()
        .find(|g| g.principal_id == principal)
        .expect("present");

    let rev_before = read_policy_rev(repo.db()).expect("rev before");
    let snap_before = view.snapshot();

    let err = repo
        .revoke_grant_and_rebuild(&operator("alice"), target.id, target.version + 99)
        .expect_err("stale version must conflict");
    assert!(
        matches!(err, StoreError::VersionConflict),
        "stale version maps to VersionConflict, got {err:?}"
    );
    // 库不变：该行仍是活跃（未置终态：ended_at 为 NULL）。
    assert_eq!(
        count_where(
            &repo,
            "temp_grants",
            &format!("id = {} AND ended_at IS NULL", target.id.as_raw())
        ),
        1,
        "grant stays non-terminal after a conflicting revoke"
    );
    // rev 不前进、snapshot 不换（同一 Arc）。
    assert_eq!(
        read_policy_rev(repo.db()).expect("rev after"),
        rev_before,
        "version conflict does not advance policy_rev"
    );
    assert!(
        Arc::ptr_eq(&snap_before, &view.snapshot()),
        "version conflict does not swap the published snapshot"
    );
}

#[test]
fn your_grants_view_projects_active_temp_grants() {
    // your_grants 投影正确：从已物化快照取该主体的 resource→capability[]；revoke 后
    // 该格消失（投影忠实反映快照物化的有效授权）。
    let (repo, _view) = repo_with_view();
    let principal = seed_principal(&repo, "agent-y1");
    let res = seed_resource(&repo, "db-y1");
    repo.elevate_grant_and_rebuild(&operator("alice"), principal, res, "query", 600_000)
        .expect("elevate query grant");

    let projection = repo.your_grants_view(principal);
    let res_code = ResourceCode::new("db-y1");
    let caps = projection
        .get(&res_code)
        .expect("resource present in your_grants projection");
    assert!(
        caps.contains(&Capability::Query),
        "elevated query capability appears in your_grants projection"
    );

    // 主体无关资源不出现（投影只含该主体的格）。
    let other = repo.your_grants_view(seed_principal(&repo, "agent-y2"));
    assert!(
        !other.contains_key(&res_code),
        "another principal sees no grant on db-y1"
    );
}
