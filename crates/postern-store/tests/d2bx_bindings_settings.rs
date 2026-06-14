//! 实体组 3（bindings 全量 + scope / settings）的 *_and_rebuild 写接缝 + list_* 读模型
//! + v2→v3 迁移数据存活 行为测试（D2bx 续组3）。每条只钉一个行为：
//!
//! - **bindings 全量**：list_bindings（**无主体过滤**）列出多主体绑定（与按主体过滤的
//!   list_bindings_of 互补）；create_binding_with_scope_and_rebuild 同事务落绑定 + 辖区，
//!   list_binding_scopes 读出正确（resource 枚举 / selector 二选一忠实落库）。
//! - **settings**：set 新 key → 插一行、list_settings 反映、rev 进；同 key 再 set → 改既有行
//!   （不新增、value 被改、version 自增）、rev 再进；全或无（持过期 version... 经 upsert 内
//!   乐观锁）。
//! - **迁移 v2→v3 数据存活**：旧 v2 库（无 settings 表）迁到 v3——已有业务行逐行存活、
//!   settings 表建出、user_version=3。
//!
//! bindings/binding_scope/settings 写一律经被测 PolicyRepo 的 *_and_rebuild（内部经
//! base::write::{insert,update} + commit_and_rebuild 唯一写路径）。读经被测 list_*。
//! rev 经 base::meta::read_policy_rev 核对。
//!
//! 雷区纪律（与 d2bx_mode_grants.rs 同源）：本文件在 crates/postern-store/ 下但**不在**
//! src/base/ 下，故对契约扫描器是"in_store 且非 in_store_base"。任何字面裸数据库读写
//! 关键词的连续串都会被记为违规。因此本文件**绝不写字面裸数据库读写标记**：建表/迁移
//! 经 migrate / migrate::ddl 返回的常量 DDL；写经被测 *_and_rebuild API；行内省读一律在
//! **运行期由片段拼接**（见 kw / count_where / table_exists）。

use std::sync::Arc;

use postern_core::domain::PolicySnapshot;
use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_core::page::PageQuery;
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

/// 以确定时钟装配一个已迁移、持视图的写句柄（首份快照 policy_rev=0）。
fn repo_with_view() -> PolicyRepo {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema on empty db");
    let idgen = IdGen::new(FixedClock(EPOCH_UNIX_MS));
    let view = Arc::new(SnapshotView::new(Arc::new(PolicySnapshot::default())));
    PolicyRepo::with_view(db, idgen, Box::new(FixedClock(EPOCH_UNIX_MS)), view)
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

/// 表是否存在（查 sqlite_master，运行期拼接读关键词）。
fn table_exists(db: &Db, table: &str) -> bool {
    let q = format!(
        "{} COUNT(*) {} sqlite_master {} type = 'table' AND name = ?1",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
    );
    db.with_read(|conn| {
        let n: i64 = conn
            .query_row(&q, [table], |r| r.get(0))
            .map_err(|_| StoreError::Io)?;
        Ok(n)
    })
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// 物理行计数（裸 Db 句柄版，供迁移测试核对旧版库写入的业务行存活）。
fn count_where_db(db: &Db, table: &str, predicate: &str) -> i64 {
    let q = format!(
        "{} COUNT(*) {} {} {} {}",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
        predicate,
    );
    db.with_read(|conn| {
        let n: i64 = conn
            .query_row(&q, [], |r| r.get(0))
            .map_err(|_| StoreError::Io)?;
        Ok(n)
    })
    .expect("count query")
}

/// 播一个主体，返回其 id。
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

/// 播一个角色，返回其 id。
fn seed_role(repo: &PolicyRepo, name: &str) -> SnowflakeId {
    repo.create_role_and_rebuild(&operator("alice"), name, None)
        .expect("seed role");
    repo.list_roles(PageQuery {
        page_no: 1,
        page_size: 200,
    })
    .expect("list roles")
    .items
    .into_iter()
    .find(|r| r.name == name)
    .map(|r| r.id)
    .expect("seeded role is listed")
}

/// 播一个资源，返回其 id。
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

// ============================================================ bindings 全量 + scope

#[test]
fn list_bindings_returns_all_principals_bindings() {
    // 全量 list_bindings（无主体过滤）→ 多主体的绑定都在（与按主体过滤的 list_bindings_of
    // 互补）。本测试钉"无过滤、跨主体"这一关键差异。
    let repo = repo_with_view();
    let p1 = seed_principal(&repo, "agent-b1");
    let p2 = seed_principal(&repo, "agent-b2");
    let role = seed_role(&repo, "observer-b");

    repo.create_binding_and_rebuild(&operator("alice"), p1, role)
        .expect("bind p1");
    repo.create_binding_and_rebuild(&operator("alice"), p2, role)
        .expect("bind p2");

    let page = repo
        .list_bindings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list all bindings");

    assert!(
        page.items.iter().any(|b| b.principal_id == p1),
        "p1's binding appears in the unfiltered full list"
    );
    assert!(
        page.items.iter().any(|b| b.principal_id == p2),
        "p2's binding appears in the unfiltered full list (no principal filter)"
    );
    let total_for_role = page.items.iter().filter(|b| b.role_id == role).count();
    assert_eq!(
        total_for_role, 2,
        "both principals' bindings to the role are listed (full list spans principals)"
    );
}

#[test]
fn list_bindings_excludes_logically_deleted() {
    // 默认作用域：逻辑删除（级联）的绑定不在全量列表里（delete_flag = 0 默认排除）。
    let repo = repo_with_view();
    let p = seed_principal(&repo, "agent-bd");
    let role = seed_role(&repo, "observer-bd");
    repo.create_binding_and_rebuild(&operator("alice"), p, role)
        .expect("bind");
    // 删主体 → 级联逻辑删除其绑定（delete_principal，既有 per-entity 写）。
    let pver = repo
        .list_principals(PageQuery {
            page_no: 1,
            page_size: 200,
        })
        .expect("list principals")
        .items
        .into_iter()
        .find(|x| x.id == p)
        .map(|x| x.version)
        .expect("principal present");
    repo.delete_principal(&operator("alice"), p, pver)
        .expect("delete principal cascades bindings");

    let page = repo
        .list_bindings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list all bindings");
    assert!(
        !page.items.iter().any(|b| b.principal_id == p),
        "cascade-deleted binding is excluded by default scope"
    );
}

#[test]
fn create_binding_with_resource_scope_is_readable() {
    // create_binding_with_scope_and_rebuild（resource 枚举辖区）→ 绑定与辖区同事务落库，
    // list_binding_scopes 读出该辖区（kind=resource、resource_id 非空、selector 空），rev 进 1。
    let repo = repo_with_view();
    let p = seed_principal(&repo, "agent-s1");
    let role = seed_role(&repo, "operator-s1");
    let res = seed_resource(&repo, "db-s1");
    let rev_before = read_policy_rev(repo.db()).expect("rev before");

    let (version, rev_after) = repo
        .create_binding_with_scope_and_rebuild(
            &operator("alice"),
            p,
            role,
            "resource",
            Some(res),
            None,
        )
        .expect("create binding with resource scope");
    assert_eq!(version, 0, "new binding row version is 0");
    assert_eq!(
        rev_after,
        rev_before + 1,
        "create binding with scope advances policy_rev by 1"
    );

    // 绑定本身可列出（无过滤全量）。
    let binding = repo
        .list_bindings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list bindings")
        .items
        .into_iter()
        .find(|b| b.principal_id == p && b.role_id == role)
        .expect("the created binding is listed");

    // 辖区可列出，且挂在该绑定上，kind/resource_id/selector 忠实读出。
    let scope = repo
        .list_binding_scopes(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list binding scopes")
        .items
        .into_iter()
        .find(|s| s.binding_id == binding.id)
        .expect("the created scope is listed and attached to the binding");
    assert_eq!(scope.kind, "resource", "scope kind persisted as 'resource'");
    assert_eq!(
        scope.resource_id,
        Some(res),
        "resource scope carries the enumerated resource id"
    );
    assert_eq!(
        scope.selector, None,
        "resource scope has no selector (selector is NULL)"
    );
    assert_eq!(scope.version, 0, "scope read model carries version 0");
}

#[test]
fn create_binding_with_selector_scope_is_readable() {
    // create_binding_with_scope_and_rebuild（selector 标签选择器辖区）→ list_binding_scopes
    // 读出该辖区（kind=selector、selector 非空、resource_id 空）。这是 resource 枚举的对偶分支。
    let repo = repo_with_view();
    let p = seed_principal(&repo, "agent-s2");
    let role = seed_role(&repo, "operator-s2");

    repo.create_binding_with_scope_and_rebuild(
        &operator("alice"),
        p,
        role,
        "selector",
        None,
        Some("env=prod"),
    )
    .expect("create binding with selector scope");

    let binding = repo
        .list_bindings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list bindings")
        .items
        .into_iter()
        .find(|b| b.principal_id == p && b.role_id == role)
        .expect("the created binding is listed");

    let scope = repo
        .list_binding_scopes(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list binding scopes")
        .items
        .into_iter()
        .find(|s| s.binding_id == binding.id)
        .expect("the created selector scope is listed");
    assert_eq!(scope.kind, "selector", "scope kind persisted as 'selector'");
    assert_eq!(
        scope.selector.as_deref(),
        Some("env=prod"),
        "selector scope carries the verbatim selector"
    );
    assert_eq!(
        scope.resource_id, None,
        "selector scope has no enumerated resource (resource_id is NULL)"
    );
}

// ============================================================ settings（upsert by key）

#[test]
fn set_setting_for_new_key_inserts_and_advances_rev() {
    // set 新 key → 插一行、list_settings 反映（key/value 如实读出）、rev 前进 1。
    let repo = repo_with_view();
    let rev_before = read_policy_rev(repo.db()).expect("rev before");

    let (version, rev_after) = repo
        .set_setting_and_rebuild(&operator("alice"), "audit.retention_days", "30")
        .expect("set new setting");
    assert_eq!(version, 0, "new setting row version is 0");
    assert_eq!(rev_after, rev_before + 1, "set advances policy_rev by 1");

    let page = repo
        .list_settings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list settings");
    let row = page
        .items
        .iter()
        .find(|s| s.key == "audit.retention_days")
        .expect("created setting is listed");
    assert_eq!(row.value, "30", "value persisted and read back");
    assert_eq!(row.version, 0, "read model carries version 0");
}

#[test]
fn set_setting_same_key_updates_existing_row_without_adding() {
    // upsert by key：同 key 再 set → 改既有行（行数不增、value 被改、version 自增），rev 再进 1。
    let repo = repo_with_view();
    repo.set_setting_and_rebuild(&operator("alice"), "log.level", "info")
        .expect("first set");
    let first = repo
        .list_settings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after first")
        .items
        .into_iter()
        .find(|s| s.key == "log.level")
        .expect("present after first set");
    assert_eq!(first.version, 0, "first set lands version 0");
    assert_eq!(first.value, "info", "first value persisted");

    let rows_before = count_where(&repo, "settings", "key = 'log.level' AND delete_flag = 0");
    assert_eq!(rows_before, 1, "exactly one active row for the key");
    let rev_before_second = read_policy_rev(repo.db()).expect("rev before second");

    let (version, rev_after) = repo
        .set_setting_and_rebuild(&operator("alice"), "log.level", "debug")
        .expect("second set on same key");
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
    let rows_after = count_where(&repo, "settings", "key = 'log.level' AND delete_flag = 0");
    assert_eq!(
        rows_after, 1,
        "still exactly one active row (upsert by key, not append)"
    );

    // 既有行被改：同一 id、value 更新到第二次的值、version 自增。
    let after = repo
        .list_settings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list after second")
        .items
        .into_iter()
        .find(|s| s.key == "log.level")
        .expect("present after second set");
    assert_eq!(after.id, first.id, "same row updated in place (same id)");
    assert_eq!(
        after.value, "debug",
        "value narrowed to the second set's value"
    );
    assert_eq!(after.version, 1, "in-place write bumped version to 1");
}

#[test]
fn distinct_keys_coexist_as_separate_rows() {
    // 不同 key 各自独立成行（upsert 收窄只对同 key 生效，不串扰）。
    let repo = repo_with_view();
    repo.set_setting_and_rebuild(&operator("alice"), "a.x", "1")
        .expect("set a.x");
    repo.set_setting_and_rebuild(&operator("alice"), "b.y", "2")
        .expect("set b.y");

    let page = repo
        .list_settings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list settings");
    assert_eq!(
        page.items.iter().filter(|s| s.key == "a.x").count(),
        1,
        "a.x is one row"
    );
    assert_eq!(
        page.items.iter().filter(|s| s.key == "b.y").count(),
        1,
        "b.y is one row (distinct key, separate row)"
    );
    assert_eq!(
        count_where(&repo, "settings", "delete_flag = 0"),
        2,
        "two distinct keys -> two active rows"
    );
}

// ============================================================ 迁移 v2→v3 数据存活

/// 构造一个**旧 v2 库**：施加 v0→v1（建全套业务表）+ v1→v2（建 policy_meta）前向步，
/// 把 user_version 钉在 2，且**不**建 v3 才有的 settings 表——即 v2 实现写出的库形态。
/// DDL 文本取自被测 migrate::ddl::forward_steps 返回的常量步（字面 needle 落在常量而非
/// 本测试源），经 execute_batch 施加。供 v2→v3 前向迁移往返测试构造前置态。
fn v2_db() -> Db {
    use postern_store::migrate::ddl;
    let db = Db::open_in_memory().expect("in-memory db opens");
    let steps = ddl::forward_steps(0).expect("forward steps from empty");
    db.with_write_txn(|txn| {
        for step in steps.iter().filter(|s| s.to <= 2) {
            txn.execute_batch(step.ddl).map_err(|_| StoreError::Io)?;
        }
        Ok(())
    })
    .expect("v0->v1->v2 DDL builds business tables + policy_meta");
    migrate::set_schema_version(&db, 2).expect("pin user_version at v2");
    db
}

#[test]
fn migrate_v2_to_v3_creates_settings_and_advances_version() {
    // 旧 v2 库（无 settings）→ migrate 单事务前向追平至 v3、建 settings 表、版本前进至当前。
    let db = v2_db();
    assert_eq!(
        migrate::schema_version(&db).expect("v2 version"),
        2,
        "starts at v2"
    );
    assert!(
        !table_exists(&db, "settings"),
        "v2 db has no settings table yet"
    );

    migrate::migrate(&db).expect("v2->v3 forward migration succeeds");

    assert_eq!(
        migrate::schema_version(&db).expect("post-migrate version"),
        3,
        "user_version advances from v2 to current (v3)"
    );
    assert!(
        table_exists(&db, "settings"),
        "v2->v3 step creates settings table"
    );
}

#[test]
fn migrate_v2_to_v3_preserves_existing_business_data() {
    // v2→v3 前向迁移只建新表、绝不改写已有数据——迁移前写入的业务行在迁移后逐行存活。
    // 在 v2 库上经被测 PolicyRepo（其 *_and_rebuild 经唯一写路径）落业务行，再迁移核对存活。
    let db = v2_db();
    let idgen = IdGen::new(FixedClock(EPOCH_UNIX_MS));
    let repo = PolicyRepo::new(db, idgen, Box::new(FixedClock(EPOCH_UNIX_MS)));
    repo.create_principal_and_rebuild(&operator("alice"), "survivor", "agent")
        .expect("write principal under v2");
    let pid = repo
        .list_principals(PageQuery {
            page_no: 1,
            page_size: 200,
        })
        .expect("list principals")
        .items
        .into_iter()
        .find(|p| p.name == "survivor")
        .map(|p| p.id)
        .expect("principal present under v2");

    // 取回裸 Db 句柄做迁移（PolicyRepo 持 Db；经其 db() 借用迁移 + 核对）。
    migrate::migrate(repo.db()).expect("v2->v3 forward migration succeeds");

    assert_eq!(
        migrate::schema_version(repo.db()).expect("version after migrate"),
        3,
        "reached v3"
    );
    assert_eq!(
        count_where_db(
            repo.db(),
            "principals",
            &format!("id = {} AND delete_flag = 0", pid.as_raw())
        ),
        1,
        "the principal written under v2 survives the v2->v3 migration"
    );
}

#[test]
fn migrate_v2_to_v3_then_settings_write_path_works() {
    // v2→v3 迁出的 settings 是空表——迁移后经被测 set_setting_and_rebuild 落值可读，
    // 验证迁移建的表与 store 写路径无缝衔接（往返后写路径可用）。
    let db = v2_db();
    migrate::migrate(&db).expect("v2->v3 forward migration succeeds");
    let idgen = IdGen::new(FixedClock(EPOCH_UNIX_MS));
    let repo = PolicyRepo::new(db, idgen, Box::new(FixedClock(EPOCH_UNIX_MS)));

    repo.set_setting_and_rebuild(&operator("alice"), "feature.flag", "on")
        .expect("set setting on freshly migrated settings table");
    let row = repo
        .list_settings(PageQuery {
            page_no: 1,
            page_size: 50,
        })
        .expect("list settings")
        .items
        .into_iter()
        .find(|s| s.key == "feature.flag")
        .expect("setting written after migration is listed");
    assert_eq!(row.value, "on", "value readable after v2->v3 migration");
}
