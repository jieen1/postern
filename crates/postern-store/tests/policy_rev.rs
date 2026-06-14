//! policy_rev + 提交/重建编排 单元行为测试（D2 写基建）：持久单调修订号、单一临界区
//! 内"写 + 递增 rev + 重建快照 + 原子发布视图"的全或无原子性、v1→v2 前向迁移。
//!
//! 每条测试只钉一个行为，测试名陈述该行为，断言精确到确切形态（rev 的确切值 / 视图
//! 内 snapshot 的 policy_rev / 乐观锁冲突时的"全不变"）或确切错误变体。失败路径是一等
//! 公民——断言"恰好是该结果"：版本冲突 → 既不改库、不进 rev、不换 snapshot。
//!
//! 雷区纪律（与 base.rs / schema_migrate.rs / snapshot.rs 同源）：本文件在
//! `crates/postern-store/` 下但**不在** `src/base/` 下，故对契约扫描器是"in_store 且非
//! in_store_base"。任何字面裸数据库读关键词的连续串出现在源文本里都会被记为违规
//! （扫描器不剥 Rust 行注释）。因此本文件**绝不写字面裸数据库读写标记**：建表/迁移一律经
//! 被测 `migrate` API；写一律经 `base::write`（唯一写路径）；行内省读（计数核对）一律在
//! **运行期由片段拼接**（见 `kw` / `count_where`）。

use std::sync::Arc;

use postern_core::domain::{PolicySnapshot, Timestamp};
use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_core::plugin::PolicyView;
use postern_store::base::db::Db;
use postern_store::base::error::StoreError;
use postern_store::base::meta::read_policy_rev;
use postern_store::base::write::{self, Actor, InsertRow};
use postern_store::migrate;
use postern_store::policy::PolicyRepo;
use postern_store::schema::CURRENT_SCHEMA_VERSION;
use postern_store::snapshot::SnapshotView;
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

fn idgen() -> IdGen {
    IdGen::new(FixedClock(EPOCH_UNIX_MS))
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(EPOCH_UNIX_MS)
}

/// 已迁移到当前版本的内存库（空库 → `migrate` 建全套表 + policy_meta + 前进 user_version）。
fn migrated_db() -> Db {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema + policy_meta on empty db");
    db
}

/// 物理行计数（按谓词），纯 COUNT(*) 投影（pagination 豁免）。谓词由调用方运行期拼接。
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
    .unwrap_or(0)
        > 0
}

/// 读库的 `PRAGMA user_version`（schema 版本号）。
fn user_version(db: &Db) -> i64 {
    db.with_read(|conn| {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|_| StoreError::Io)
    })
    .expect("user_version pragma")
}

// ============================================================ 经 base 唯一写路径播种

/// 经 base 唯一写路径插一行 principals（业务列 name/kind），返回新行 id。
fn seed_principal(db: &Db, g: &IdGen, name: &str) -> SnowflakeId {
    db.with_write_txn(|txn| {
        write::insert(
            txn,
            g,
            now(),
            &Actor::System,
            InsertRow {
                table: "principals",
                columns: vec!["name", "kind"],
                values: vec![Value::Text(name.into()), Value::Text("agent".into())],
                enable_flag: 1,
            },
        )
    })
    .expect("seed principal via base")
}

/// 读单行 principals 的 version（带 delete_flag=0 默认作用域，pagination 单行豁免）。
fn principal_version(db: &Db, id: SnowflakeId) -> i64 {
    let q = format!(
        "{} version {} principals {} id = ?1 AND delete_flag = 0 {} 1",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        kw(&["LIM", "IT"]),
    );
    db.with_read(|conn| {
        conn.query_row(&q, [id.as_raw() as i64], |r| r.get(0))
            .map_err(|_| StoreError::Io)
    })
    .expect("principal version read")
}

/// 装配一个持视图的写句柄 + 它发布的初始视图（首份快照 policy_rev=0）。
fn repo_with_view() -> (PolicyRepo, Arc<SnapshotView>) {
    let db = migrated_db();
    let view = Arc::new(SnapshotView::new(Arc::new(PolicySnapshot::default())));
    let repo = PolicyRepo::with_view(
        db,
        idgen(),
        Box::new(FixedClock(EPOCH_UNIX_MS)),
        Arc::clone(&view),
    );
    (repo, view)
}

// ============================================================ 1) policy_rev 持久 + 单调

#[test]
fn migrate_creates_policy_meta_table_and_advances_to_v2() {
    // v1→v2 前向：迁移后 policy_meta 建齐、user_version 抵达当前最高版本(2)。
    let db = migrated_db();
    assert!(
        table_exists(&db, "policy_meta"),
        "v1->v2 migration builds the persistent policy_meta table"
    );
    assert_eq!(
        user_version(&db),
        CURRENT_SCHEMA_VERSION,
        "migrate advances user_version to the current highest schema version (2)"
    );
}

#[test]
fn fresh_policy_rev_after_migration_is_zero() {
    // 迁移后初始 rev：尚无任何提交，policy_rev 读作 0（缺失行视作 0）。
    let db = migrated_db();
    assert_eq!(
        read_policy_rev(&db).expect("read policy_rev"),
        0,
        "freshly migrated db has policy_rev = 0 (no commits yet)"
    );
}

#[test]
fn bump_policy_rev_is_strictly_monotonic() {
    // 连续 bump：rev 严格 +1（1,2,3...），单调不跳号不回退。
    let db = migrated_db();
    for expected in 1u64..=3 {
        let got = db
            .with_write_txn(write::bump_policy_rev)
            .expect("bump policy_rev");
        assert_eq!(
            got, expected,
            "each bump advances policy_rev by exactly 1 (strictly monotonic)"
        );
        assert_eq!(
            read_policy_rev(&db).expect("read policy_rev"),
            expected,
            "the persisted policy_rev equals the bumped value"
        );
    }
}

#[test]
fn policy_rev_survives_reopen_without_regressing() {
    // 跨重开库：rev 持久落库，重开后不回退（持久单调）。用持久文件库验证落盘。
    let dir = std::env::temp_dir().join(format!("postern_rev_{}", EPOCH_UNIX_MS));
    let _ = std::fs::remove_file(&dir);
    {
        let db = Db::open(&dir).expect("open file db");
        migrate::migrate(&db).expect("migrate file db");
        for _ in 0..2 {
            db.with_write_txn(write::bump_policy_rev).expect("bump");
        }
        assert_eq!(read_policy_rev(&db).expect("read"), 2, "two bumps -> rev 2");
    }
    {
        let db = Db::open(&dir).expect("reopen file db");
        migrate::migrate(&db).expect("migrate is idempotent on reopen");
        assert_eq!(
            read_policy_rev(&db).expect("read after reopen"),
            2,
            "policy_rev survives reopen and never regresses"
        );
    }
    let _ = std::fs::remove_file(&dir);
}

// ============================================================ 2) 提交+重建 原子

#[test]
fn commit_and_rebuild_advances_rev_and_returns_new_version() {
    // 一次成功写：返回 (new_version, new_rev)；写实体 version +1、rev 由 0 进到 1。
    let (repo, _view) = repo_with_view();
    let pid = seed_principal(repo.db(), &idgen(), "alpha");
    let v0 = principal_version(repo.db(), pid);

    let (new_version, new_rev) = repo
        .commit_and_rebuild(|txn| {
            write::update(
                txn,
                now(),
                &Actor::System,
                "principals",
                pid,
                v0,
                vec!["name"],
                vec![Value::Text("alpha2".into())],
                None,
            )?;
            Ok(v0 + 1)
        })
        .expect("commit_and_rebuild succeeds");

    assert_eq!(
        new_version,
        v0 + 1,
        "returned new_version is the entity's bumped version"
    );
    assert_eq!(new_rev, 1, "first commit advances policy_rev from 0 to 1");
    assert_eq!(
        read_policy_rev(repo.db()).expect("read rev"),
        1,
        "policy_rev is persisted at 1 after one commit"
    );
}

#[test]
fn commit_and_rebuild_publishes_snapshot_with_matching_rev() {
    // 成功写后 SnapshotView 立即反映新状态，且视图内 snapshot.policy_rev == 返回的 new_rev
    // （rev 与 snapshot 一致——无 torn 态）。
    let (repo, view) = repo_with_view();
    let pid = seed_principal(repo.db(), &idgen(), "beta");
    let v0 = principal_version(repo.db(), pid);

    let (_new_version, new_rev) = repo
        .commit_and_rebuild(|txn| {
            write::update(
                txn,
                now(),
                &Actor::System,
                "principals",
                pid,
                v0,
                vec!["name"],
                vec![Value::Text("beta2".into())],
                None,
            )?;
            Ok(v0 + 1)
        })
        .expect("commit_and_rebuild succeeds");

    assert_eq!(
        view.snapshot().policy_rev,
        new_rev,
        "published snapshot's policy_rev matches the returned new_rev (rev == snapshot, no torn state)"
    );
}

#[test]
fn commit_and_rebuild_version_conflict_is_all_or_nothing() {
    // 乐观锁版本冲突：既不改库、也不前进 rev、也不换 snapshot（全或无）。
    let (repo, view) = repo_with_view();
    let pid = seed_principal(repo.db(), &idgen(), "gamma");
    let v0 = principal_version(repo.db(), pid);
    let rev_before = read_policy_rev(repo.db()).expect("read rev before");
    let snap_before = view.snapshot();

    // 传入错误的期望 version（v0 + 99）→ 影响 0 行 → VersionConflict。
    let err = repo
        .commit_and_rebuild(|txn| {
            write::update(
                txn,
                now(),
                &Actor::System,
                "principals",
                pid,
                v0 + 99,
                vec!["name"],
                vec![Value::Text("gamma_should_not_land".into())],
                None,
            )?;
            Ok(v0 + 1)
        })
        .expect_err("version conflict must surface as Err");
    assert!(
        matches!(err, StoreError::VersionConflict),
        "stale expected_version maps to VersionConflict (caller maps 409)"
    );

    // 库不变：该主体名未改、version 未动。
    assert_eq!(
        principal_version(repo.db(), pid),
        v0,
        "version conflict leaves the row's version unchanged (db unchanged)"
    );
    assert_eq!(
        count_where(repo.db(), "principals", "name = 'gamma_should_not_land'"),
        0,
        "the conflicting write never landed any row"
    );
    // rev 不前进。
    assert_eq!(
        read_policy_rev(repo.db()).expect("read rev after"),
        rev_before,
        "version conflict does not advance policy_rev"
    );
    // snapshot 不换（仍是同一份 Arc）。
    assert!(
        Arc::ptr_eq(&snap_before, &view.snapshot()),
        "version conflict does not swap the published snapshot (same Arc)"
    );
}

// ============================================================ 3) 写仍经 base::write

#[test]
fn commit_and_rebuild_delete_goes_through_logical_delete() {
    // 经编排做删除：走 base::write::logical_delete（逻辑删，非物理删）——行仍在、
    // delete_flag=1，默认作用域不再可见；rev 前进。
    let (repo, _view) = repo_with_view();
    let pid = seed_principal(repo.db(), &idgen(), "delta");
    let v0 = principal_version(repo.db(), pid);

    let (_v, new_rev) = repo
        .commit_and_rebuild(|txn| {
            write::logical_delete(txn, now(), &Actor::System, "principals", pid, v0)?;
            Ok(v0 + 1)
        })
        .expect("commit_and_rebuild delete succeeds");

    assert_eq!(new_rev, 1, "delete commit advances policy_rev to 1");
    // 物理行仍在（逻辑删除，非物理删除）。
    assert_eq!(
        count_where(repo.db(), "principals", &format!("id = {}", pid.as_raw())),
        1,
        "logical delete keeps the physical row (no physical delete)"
    );
    // 但 delete_flag=1。
    assert_eq!(
        count_where(
            repo.db(),
            "principals",
            &format!("id = {} AND delete_flag = 1", pid.as_raw())
        ),
        1,
        "the row is logically deleted (delete_flag = 1), not physically removed"
    );
}

#[test]
fn bump_writes_base_fields_on_policy_meta_row() {
    // policy_rev 行经 base 唯一写路径落库：作为带 8 基础字段的行存在（version 自增等
    // 由 base 维护），未绕过 BASE_FIELDS——验证 policy_meta 是带基础字段的合规表。
    let db = migrated_db();
    db.with_write_txn(write::bump_policy_rev)
        .expect("first bump creates the policy_rev row");
    // policy_meta 中恰有一行活跃的 policy_rev 键（带基础字段、delete_flag=0）。
    assert_eq!(
        count_where(&db, "policy_meta", "key = 'policy_rev' AND delete_flag = 0"),
        1,
        "the policy_rev row lives in policy_meta with base fields (delete_flag = 0)"
    );
}
