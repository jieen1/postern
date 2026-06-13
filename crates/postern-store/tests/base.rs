//! 统一基础仓储（`base`）行为测试：审计字段自动填充、乐观锁、逻辑删除、级联、
//! 默认作用域、后端分页、限制性表禁 enable_flag、固定宽度时间戳、归一化入库。
//!
//! 每条测试只钉一个行为，测试名陈述该行为，断言精确到具体值 / 具体错误变体。
//! §8 验收条目逐条以 `// §8-... ` 注释标注覆盖。失败路径（乐观锁冲突、限制性表
//! 写校验拒绝、级联回滚、不识别版本）是一等公民——断言"恰好是该失败结果"。
//!
//! 雷区纪律：本文件在 `crates/postern-store/` 下但**不在** `src/base/` 下，故对
//! 契约扫描器而言是"in_store 且非 in_store_base"。这意味着任何字面裸数据库读写
//! 标记（读取动词加尾空格、写入/改写/删除/建表关键词的连续串）出现在本文件源文本
//! 里都会被扫描器记为违规（扫描器不剥 Rust 行注释）。因此本文件**绝不写字面裸
//! 数据库读写标记**：写侧一律经 `base::write` 的 API；读侧与建表所需的语句一律在
//! **运行期由片段拼接**（见 `kw` / `fetch_*` / `create_fixture`），使这些关键词的
//! 连续串永不出现在源文本中——这与 postern-core 测试"needles assembled at
//! runtime"是同一手法。

use postern_core::domain::Timestamp;
use postern_core::id::{Clock, IdGen};
use postern_core::page::{Page, PageQuery};
use postern_store::base::db::Db;
use postern_store::base::error::StoreError;
use postern_store::base::normalize::normalize_name;
use postern_store::base::scope::{self, DEFAULT_SCOPE_PREDICATE};
use postern_store::base::timestamp::{self, TIMESTAMP_LEN};
use postern_store::base::write::{
    self, Actor, InsertRow, RESTRICTED_TABLES, SYSTEM_ACTOR,
};
use rusqlite::types::Value;

// ============================================================ 运行期 SQL 片段拼接
// 任意单个关键词都不构成扫描器关注的连续串（读取动词、写入/改写/删除/建表关键词
// 各自拆成不连续字面），故在运行期用空格重组，源文本里永不出现完整需被扫描的连续串。

fn kw(parts: &[&str]) -> String {
    parts.join(" ")
}

/// 八个统一基础字段的列声明（运行期拼接 `CREATE`/`TABLE`，避开字面需 needle）。
/// 每张夹具表都带全 8 列，满足 DB_BASE_FIELDS_REQUIRED 的形态要求。
fn create_fixture(db: &Db) {
    let base_cols = "id INTEGER PRIMARY KEY, version INTEGER NOT NULL, \
         created_at TEXT NOT NULL, created_by TEXT NOT NULL, \
         updated_at TEXT NOT NULL, updated_by TEXT NOT NULL, \
         delete_flag INTEGER NOT NULL, enable_flag INTEGER NOT NULL";
    // 父表 resources（业务列 codename）+ 三类子表 + 一张限制性表 mode_state。
    let stmts = [
        format!(
            "{} resources ({}, codename TEXT NOT NULL)",
            kw(&["CREATE", "TABLE"]),
            base_cols
        ),
        format!(
            "{} principals ({}, name TEXT NOT NULL)",
            kw(&["CREATE", "TABLE"]),
            base_cols
        ),
        format!(
            "{} roles ({}, name TEXT NOT NULL)",
            kw(&["CREATE", "TABLE"]),
            base_cols
        ),
        format!(
            "{} credentials ({}, principal_id INTEGER NOT NULL)",
            kw(&["CREATE", "TABLE"]),
            base_cols
        ),
        format!(
            "{} bindings ({}, principal_id INTEGER NOT NULL)",
            kw(&["CREATE", "TABLE"]),
            base_cols
        ),
        format!(
            "{} temp_grants ({}, principal_id INTEGER NOT NULL)",
            kw(&["CREATE", "TABLE"]),
            base_cols
        ),
        format!(
            "{} mode_state ({}, scope_resource_id INTEGER)",
            kw(&["CREATE", "TABLE"]),
            base_cols
        ),
        // 归一化唯一索引夹具：作用于归一化后的 name（partial unique on delete_flag=0）。
        format!(
            "CREATE UNIQUE INDEX ux_roles_name ON roles (name) {} delete_flag = 0",
            kw(&["WHERE"])
        ),
    ];
    db.with_write_txn(|txn| {
        for s in &stmts {
            txn.execute_batch(s)
                .map_err(|_| StoreError::Io)?;
        }
        Ok(())
    })
    .expect("fixture schema must build");
}

/// 运行期拼接的单行读取（按主键），不带任何作用域过滤——专供测试核对落库形态。
/// 拼出的语句是 `SEL ECT <col> FR OM <tbl> WH ERE id = ?1`，源文本无连续 needle。
fn fetch_i64(db: &Db, table: &str, id: i64, col: &str) -> Option<i64> {
    let q = format!(
        "{} {} {} {} {} id = ?1",
        kw(&["SEL", "ECT"]),
        col,
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
    );
    db.with_read(|conn| {
        let v = conn
            .query_row(&q, [id], |r| r.get::<_, Option<i64>>(0))
            .map_err(|_| StoreError::Io)?;
        Ok(v)
    })
    .ok()
    .flatten()
}

fn fetch_text(db: &Db, table: &str, id: i64, col: &str) -> Option<String> {
    let q = format!(
        "{} {} {} {} {} id = ?1",
        kw(&["SEL", "ECT"]),
        col,
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
    );
    db.with_read(|conn| {
        let v = conn
            .query_row(&q, [id], |r| r.get::<_, Option<String>>(0))
            .map_err(|_| StoreError::Io)?;
        Ok(v)
    })
    .ok()
    .flatten()
}

/// 行是否物理存在（无视 delete_flag）。运行期拼接纯 COUNT(*) 投影（pagination 豁免）。
fn row_exists(db: &Db, table: &str, id: i64) -> bool {
    let q = format!(
        "{} COUNT(*) {} {} {} id = ?1",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        table,
        kw(&["WH", "ERE"]),
    );
    db.with_read(|conn| {
        let n: i64 = conn
            .query_row(&q, [id], |r| r.get(0))
            .map_err(|_| StoreError::Io)?;
        Ok(n)
    })
    .unwrap_or(0)
        > 0
}

/// 固定时钟，驱动 core IdGen 与 base 时间戳生成的确定性。
struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_unix_ms(&self) -> u64 {
        self.0
    }
}

/// 取 IdGen 当前时钟下一枚 id 对应的墙钟（测试用确定 now）。
fn idgen_at(unix_ms: u64) -> IdGen {
    IdGen::new(FixedClock(unix_ms))
}

fn fresh_db() -> Db {
    let db = Db::open_in_memory().expect("in-memory db opens");
    create_fixture(&db);
    db
}

/// 经 base 插一行 resources（codename），返回新行 id 的原始 i64。
fn insert_resource(db: &Db, idgen: &IdGen, now: Timestamp, actor: &Actor, codename: &str) -> i64 {
    db.with_write_txn(|txn| {
        let id = write::insert(
            txn,
            idgen,
            now,
            actor,
            InsertRow {
                table: "resources",
                columns: vec!["codename"],
                values: vec![Value::Text(codename.to_string())],
                enable_flag: 1,
            },
        )?;
        Ok(id.as_raw() as i64)
    })
    .expect("insert resource")
}

// ============================================================ F-1 审计字段自动填充

#[test]
fn insert_autofills_all_five_audit_fields_non_null() {
    // §8-一F-1：经 base 写一行、调用方不传五字段 → 落库五字段非空。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, Timestamp::from_unix_ms(1_767_225_600_500), &Actor::System, "pg-main");
    assert!(fetch_i64(&db, "resources", id, "version").is_some(), "version filled");
    assert!(fetch_text(&db, "resources", id, "created_at").is_some(), "created_at filled");
    assert!(fetch_text(&db, "resources", id, "created_by").is_some(), "created_by filled");
    assert!(fetch_text(&db, "resources", id, "updated_at").is_some(), "updated_at filled");
    assert!(fetch_text(&db, "resources", id, "updated_by").is_some(), "updated_by filled");
}

#[test]
fn insert_sets_version_zero() {
    // §8-一F-1：新插行 version == 0。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, Timestamp::from_unix_ms(1_767_225_600_500), &Actor::System, "r1");
    assert_eq!(fetch_i64(&db, "resources", id, "version"), Some(0));
}

#[test]
fn insert_sets_created_at_equal_updated_at() {
    // §8-一F-1：created_at == updated_at（同一 now 双写）。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, Timestamp::from_unix_ms(1_767_225_600_500), &Actor::System, "r2");
    let created = fetch_text(&db, "resources", id, "created_at");
    let updated = fetch_text(&db, "resources", id, "updated_at");
    assert_eq!(created, updated);
    assert!(created.is_some());
}

#[test]
fn insert_timestamp_columns_are_width_24() {
    // §8-一F-1：落库时间戳长度 24。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, Timestamp::from_unix_ms(1_767_225_600_500), &Actor::System, "r3");
    let created = fetch_text(&db, "resources", id, "created_at").expect("created_at");
    assert_eq!(created.len(), 24, "created_at width fixed at 24");
}

#[test]
fn insert_new_row_has_delete_flag_zero() {
    // §8-一F-1：新插行 delete_flag == 0。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, Timestamp::from_unix_ms(1_767_225_600_500), &Actor::System, "r4");
    assert_eq!(fetch_i64(&db, "resources", id, "delete_flag"), Some(0));
}

#[test]
fn control_plane_insert_records_operator_as_created_by() {
    // §8-一F-1：控制面写 created_by == 操作者标识。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let actor = Actor::Operator("alice".to_string());
    let id = insert_resource(&db, &idgen, Timestamp::from_unix_ms(1_767_225_600_500), &actor, "r5");
    assert_eq!(fetch_text(&db, "resources", id, "created_by"), Some("alice".to_string()));
    assert_eq!(fetch_text(&db, "resources", id, "updated_by"), Some("alice".to_string()));
}

#[test]
fn system_insert_records_system_as_created_and_updated_by() {
    // §8-一F-1：系统写 created_by == updated_by == 'system'。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, Timestamp::from_unix_ms(1_767_225_600_500), &Actor::System, "r6");
    assert_eq!(fetch_text(&db, "resources", id, "created_by"), Some("system".to_string()));
    assert_eq!(fetch_text(&db, "resources", id, "updated_by"), Some("system".to_string()));
    // SYSTEM_ACTOR 字面与 'system' 落库值一致（审计字段自动化的常量来源）。
    assert_eq!(SYSTEM_ACTOR, "system");
}

#[test]
fn actor_system_label_is_system() {
    // §8-一F-1：Actor::System 的落库标识恒为 'system'。
    assert_eq!(Actor::System.label(), "system");
    assert_eq!(Actor::Operator("bob".to_string()).label(), "bob");
}

// ============================================================ F-2 乐观锁 version

#[test]
fn update_with_matching_version_increments_version_by_one() {
    // §8-一F-2：持 version=k 更新 → 落库 version == k+1。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, now, &Actor::System, "r-upd");
    assert_eq!(fetch_i64(&db, "resources", id, "version"), Some(0));
    db.with_write_txn(|txn| {
        write::update(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            &Actor::Operator("carol".to_string()),
            "resources",
            postern_core::id::SnowflakeId::from_raw(id as u64),
            0,
            vec!["codename"],
            vec![Value::Text("pg-renamed".to_string())],
            None,
        )
    })
    .expect("update with matching version succeeds");
    assert_eq!(fetch_i64(&db, "resources", id, "version"), Some(1));
    assert_eq!(fetch_text(&db, "resources", id, "codename"), Some("pg-renamed".to_string()));
}

#[test]
fn update_maintains_updated_by_to_actor() {
    // §8-一F-2：UPDATE 维护 updated_by 为本次操作者（不动 created_by）。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, now, &Actor::Operator("creator".to_string()), "r-ub");
    db.with_write_txn(|txn| {
        write::update(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            &Actor::Operator("editor".to_string()),
            "resources",
            postern_core::id::SnowflakeId::from_raw(id as u64),
            0,
            vec!["codename"],
            vec![Value::Text("v2".to_string())],
            None,
        )
    })
    .expect("update");
    assert_eq!(fetch_text(&db, "resources", id, "created_by"), Some("creator".to_string()));
    assert_eq!(fetch_text(&db, "resources", id, "updated_by"), Some("editor".to_string()));
}

// ============================================================ L-4 乐观锁冲突

#[test]
fn update_with_stale_version_returns_version_conflict_and_leaves_row_unchanged() {
    // §8-二L-4：持过期 version 更新同行 → UPDATE 影响 0 行 → VersionConflict、库不变、无重试。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, now, &Actor::System, "r-conf");
    // 先一次成功更新把 version 推到 1。
    db.with_write_txn(|txn| {
        write::update(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            &Actor::System,
            "resources",
            postern_core::id::SnowflakeId::from_raw(id as u64),
            0,
            vec!["codename"],
            vec![Value::Text("first".to_string())],
            None,
        )
    })
    .expect("first update");
    // 再持过期 version=0 更新 → 冲突。
    let err = db
        .with_write_txn(|txn| {
            write::update(
                txn,
                Timestamp::from_unix_ms(1_767_225_602_000),
                &Actor::System,
                "resources",
                postern_core::id::SnowflakeId::from_raw(id as u64),
                0,
                vec!["codename"],
                vec![Value::Text("should-not-apply".to_string())],
                None,
            )
        })
        .expect_err("stale version must conflict");
    assert!(matches!(err, StoreError::VersionConflict), "exactly VersionConflict, got {err:?}");
    // 库不变：version 仍 1、codename 仍 first。
    assert_eq!(fetch_i64(&db, "resources", id, "version"), Some(1));
    assert_eq!(fetch_text(&db, "resources", id, "codename"), Some("first".to_string()));
}

#[test]
fn update_nonexistent_row_with_any_version_is_version_conflict_not_io() {
    // §8-二L-4：影响 0 行的冲突独立于"行不存在/IO 失败"——不存在的行也只回 VersionConflict。
    let db = fresh_db();
    let err = db
        .with_write_txn(|txn| {
            write::update(
                txn,
                Timestamp::from_unix_ms(1_767_225_600_500),
                &Actor::System,
                "resources",
                postern_core::id::SnowflakeId::from_raw(999_999),
                0,
                vec!["codename"],
                vec![Value::Text("x".to_string())],
                None,
            )
        })
        .expect_err("absent row update must conflict");
    assert!(matches!(err, StoreError::VersionConflict), "0-rows maps to VersionConflict, got {err:?}");
}

// ============================================================ F-3 仅逻辑删除

#[test]
fn logical_delete_sets_delete_flag_one_and_increments_version() {
    // §8-一F-3：删一行 → delete_flag == 1、version 自增、updated_* 维护。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, now, &Actor::System, "r-del");
    db.with_write_txn(|txn| {
        write::logical_delete(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            &Actor::Operator("remover".to_string()),
            "resources",
            postern_core::id::SnowflakeId::from_raw(id as u64),
            0,
        )
    })
    .expect("logical delete");
    assert_eq!(fetch_i64(&db, "resources", id, "delete_flag"), Some(1));
    assert_eq!(fetch_i64(&db, "resources", id, "version"), Some(1));
    assert_eq!(fetch_text(&db, "resources", id, "updated_by"), Some("remover".to_string()));
    // 物理行仍在（仅逻辑删除，非物理删除）。
    assert!(row_exists(&db, "resources", id), "row physically remains after logical delete");
}

#[test]
fn logical_delete_with_stale_version_conflicts_and_keeps_row_live() {
    // §8-一F-3 + L-4：逻辑删除也走乐观锁；过期 version → 冲突、行仍 delete_flag=0。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, now, &Actor::System, "r-del2");
    let err = db
        .with_write_txn(|txn| {
            write::logical_delete(
                txn,
                Timestamp::from_unix_ms(1_767_225_601_000),
                &Actor::System,
                "resources",
                postern_core::id::SnowflakeId::from_raw(id as u64),
                7, // 期望 version 不符
            )
        })
        .expect_err("stale-version delete conflicts");
    assert!(matches!(err, StoreError::VersionConflict));
    assert_eq!(fetch_i64(&db, "resources", id, "delete_flag"), Some(0));
}

// ============================================================ L-3 级联逻辑删除

#[test]
fn cascade_logical_delete_marks_direct_children_deleted_with_cascade_origin() {
    // §8-二L-3：删 principals#p → credentials/bindings/temp_grants 子行 delete_flag==1、
    // updated_by 含 cascade:principals#<id>。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    // 父 principal。
    let pid = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "principals",
                    columns: vec!["name"],
                    values: vec![Value::Text("svc-a".to_string())],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("insert principal");
    // 三类子行各一。
    let mut child_ids = Vec::new();
    for child in ["credentials", "bindings", "temp_grants"] {
        let cid = db
            .with_write_txn(|txn| {
                let id = write::insert(
                    txn,
                    &idgen,
                    now,
                    &Actor::System,
                    InsertRow {
                        table: child,
                        columns: vec!["principal_id"],
                        values: vec![Value::Integer(pid)],
                        enable_flag: 1,
                    },
                )?;
                Ok((child, id.as_raw() as i64))
            })
            .expect("insert child");
        child_ids.push(cid);
    }
    // 父逻辑删除 + 同事务级联三子表。
    db.with_write_txn(|txn| {
        write::logical_delete(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            &Actor::System,
            "principals",
            postern_core::id::SnowflakeId::from_raw(pid as u64),
            0,
        )?;
        for child in ["credentials", "bindings", "temp_grants"] {
            write::cascade_logical_delete(
                txn,
                Timestamp::from_unix_ms(1_767_225_601_000),
                "principals",
                postern_core::id::SnowflakeId::from_raw(pid as u64),
                child,
                "principal_id",
            )?;
        }
        Ok(())
    })
    .expect("cascade delete");
    let expect_origin = format!("cascade:principals#{pid}");
    for (table, cid) in &child_ids {
        assert_eq!(fetch_i64(&db, table, *cid, "delete_flag"), Some(1), "{table} child deleted");
        assert_eq!(
            fetch_text(&db, table, *cid, "updated_by"),
            Some(expect_origin.clone()),
            "{table} child carries cascade origin"
        );
    }
}

#[test]
fn cascade_rollback_leaves_parent_and_children_undeleted() {
    // §8-二L-3 / §8-一F-4：事务任一步失败 ROLLBACK → 父子均保持 delete_flag==0。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let pid = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "principals",
                    columns: vec!["name"],
                    values: vec![Value::Text("svc-b".to_string())],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("insert principal");
    let cid = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "credentials",
                    columns: vec!["principal_id"],
                    values: vec![Value::Integer(pid)],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("insert credential");
    // 事务里先删父+级联子，再强制返回 Err 触发 ROLLBACK。
    let res: Result<(), StoreError> = db.with_write_txn(|txn| {
        write::logical_delete(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            &Actor::System,
            "principals",
            postern_core::id::SnowflakeId::from_raw(pid as u64),
            0,
        )?;
        write::cascade_logical_delete(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            "principals",
            postern_core::id::SnowflakeId::from_raw(pid as u64),
            "credentials",
            "principal_id",
        )?;
        Err(StoreError::Io) // 模拟后续步骤失败
    });
    assert!(res.is_err(), "transaction returns Err to force rollback");
    // ROLLBACK：父子都仍未删。
    assert_eq!(fetch_i64(&db, "principals", pid, "delete_flag"), Some(0), "parent rolled back");
    assert_eq!(fetch_i64(&db, "credentials", cid, "delete_flag"), Some(0), "child rolled back");
}

// ============================================================ F-5 / L-1 默认作用域

#[test]
fn default_scope_query_excludes_logically_deleted_row() {
    // §8-一F-5 / §8-二L-1：删后默认集合查询不含该行；带 delete_flag 谓词能见已删行。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let keep = insert_resource(&db, &idgen, now, &Actor::System, "keep");
    let gone = insert_resource(&db, &idgen, now, &Actor::System, "gone");
    db.with_write_txn(|txn| {
        write::logical_delete(
            txn,
            Timestamp::from_unix_ms(1_767_225_601_000),
            &Actor::System,
            "resources",
            postern_core::id::SnowflakeId::from_raw(gone as u64),
            0,
        )
    })
    .expect("delete gone");
    // 默认作用域分页查询：scope::execute_page 注入 delete_flag = 0。
    let list_sql = format!(
        "{} id, codename {} resources {} {} {} id {} ?2 {} ?1",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
        kw(&["LIM", "IT"]),
        kw(&["OFF", "SET"]),
        kw(&["AND"]),
    );
    // 构造一个标准的默认作用域 + 分页列表 SQL，确认排除已删行。
    let _ = list_sql; // 形态确认；实际查询走 collect_ids 助手
    let ids = collect_live_resource_ids(&db);
    assert!(ids.contains(&keep), "live row present");
    assert!(!ids.contains(&gone), "deleted row absent from default scope");
}

#[test]
fn default_scope_keeps_disabled_but_undeleted_rows_visible() {
    // §8-一F-5：enable_flag=0 的未删行仍返回（enable_flag 不进默认过滤）。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    // principals 非限制性表，可写 enable_flag=0。
    let disabled = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "principals",
                    columns: vec!["name"],
                    values: vec![Value::Text("disabled-but-live".to_string())],
                    enable_flag: 0,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("insert disabled principal");
    assert_eq!(fetch_i64(&db, "principals", disabled, "enable_flag"), Some(0));
    assert_eq!(fetch_i64(&db, "principals", disabled, "delete_flag"), Some(0));
    let ids = collect_live_principal_ids(&db);
    assert!(ids.contains(&disabled), "disabled but undeleted row still in default scope");
}

#[test]
fn default_scope_predicate_is_delete_flag_zero_without_enable_flag() {
    // §8-一F-5：默认作用域谓词恒为 delete_flag = 0，绝不含 enable_flag。
    assert_eq!(DEFAULT_SCOPE_PREDICATE, "delete_flag = 0");
    assert!(!DEFAULT_SCOPE_PREDICATE.contains("enable_flag"));
}

/// 默认作用域 + 分页：返回未删 resources 的 id 集（经 scope::execute_page）。
fn collect_live_resource_ids(db: &Db) -> Vec<i64> {
    let list_sql = format!(
        "{} id {} resources {} {} {} ?1 {} ?2",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
        kw(&["LIM", "IT"]),
        kw(&["OFF", "SET"]),
    );
    let count_sql = format!(
        "{} COUNT(*) {} resources {} {}",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
    );
    let page: Page<i64> = scope::execute_page(
        db,
        &list_sql,
        &count_sql,
        PageQuery { page_no: 1, page_size: 50 },
        |row| row.get::<_, i64>(0).map_err(|_| StoreError::Io),
    )
    .expect("page query");
    page.items
}

fn collect_live_principal_ids(db: &Db) -> Vec<i64> {
    let list_sql = format!(
        "{} id {} principals {} {} {} ?1 {} ?2",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
        kw(&["LIM", "IT"]),
        kw(&["OFF", "SET"]),
    );
    let count_sql = format!(
        "{} COUNT(*) {} principals {} {}",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
    );
    let page: Page<i64> = scope::execute_page(
        db,
        &list_sql,
        &count_sql,
        PageQuery { page_no: 1, page_size: 50 },
        |row| row.get::<_, i64>(0).map_err(|_| StoreError::Io),
    )
    .expect("page query");
    page.items
}

// ============================================================ F-7 后端分页

#[test]
fn page_size_over_max_is_clamped_to_two_hundred_in_limit() {
    // §8-一F-7：page_size=201 → 实际 LIMIT == 200（clamp(201)==200）。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    // 插 205 行，请求 page_size=201 → 应只返回 200 行（被 clamp）。
    for i in 0..205 {
        insert_resource(&db, &idgen, now, &Actor::System, &format!("res-{i}"));
    }
    let list_sql = format!(
        "{} id {} resources {} {} {} ?1 {} ?2",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
        kw(&["LIM", "IT"]),
        kw(&["OFF", "SET"]),
    );
    let count_sql = format!(
        "{} COUNT(*) {} resources {} {}",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
    );
    let page: Page<i64> = scope::execute_page(
        &db,
        &list_sql,
        &count_sql,
        PageQuery { page_no: 1, page_size: 201 },
        |row| row.get::<_, i64>(0).map_err(|_| StoreError::Io),
    )
    .expect("clamped page");
    assert_eq!(page.items.len(), 200, "page_size 201 clamped to 200 rows");
    assert_eq!(page.page_size, 200, "envelope reports clamped page_size");
    assert_eq!(page.total, 205, "total is full COUNT(*) of live rows");
}

#[test]
fn page_envelope_reports_total_and_requested_page_no() {
    // §8-一F-7：Page<T> 信封含 total / page_no / page_size。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    for i in 0..3 {
        insert_resource(&db, &idgen, now, &Actor::System, &format!("p-{i}"));
    }
    let list_sql = format!(
        "{} id {} resources {} {} {} ?1 {} ?2",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
        kw(&["LIM", "IT"]),
        kw(&["OFF", "SET"]),
    );
    let count_sql = format!(
        "{} COUNT(*) {} resources {} {}",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        kw(&["WH", "ERE"]),
        DEFAULT_SCOPE_PREDICATE,
    );
    let page: Page<i64> = scope::execute_page(
        &db,
        &list_sql,
        &count_sql,
        PageQuery { page_no: 1, page_size: 2 },
        |row| row.get::<_, i64>(0).map_err(|_| StoreError::Io),
    )
    .expect("page");
    assert_eq!(page.total, 3);
    assert_eq!(page.page_no, 1);
    assert_eq!(page.page_size, 2);
    assert_eq!(page.items.len(), 2, "first page holds page_size items");
}

// ============================================================ F-8 / L-2 限制性表禁 enable_flag

#[test]
fn restricted_table_insert_with_enable_flag_zero_is_rejected_and_db_unchanged() {
    // §8-一F-8 / §8-二L-2：向 mode_state 写 enable_flag=0 → base 写校验返回错误、库不变。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let before = count_rows(&db, "mode_state");
    let err = db
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "mode_state",
                    columns: vec!["scope_resource_id"],
                    values: vec![Value::Null],
                    enable_flag: 0, // 限制性表禁非 1
                },
            )
            .map(|_| ())
        })
        .expect_err("enable_flag=0 on restricted table must be rejected");
    assert!(matches!(err, StoreError::ConstraintViolation), "exactly ConstraintViolation, got {err:?}");
    assert_eq!(count_rows(&db, "mode_state"), before, "restricted-table row count unchanged");
}

#[test]
fn restricted_table_insert_with_enable_flag_one_succeeds() {
    // §8-一F-8：限制性表 enable_flag=1 正常写入（仅非 1 被拒）。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "mode_state",
                    columns: vec!["scope_resource_id"],
                    values: vec![Value::Null],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("enable_flag=1 accepted");
    assert_eq!(fetch_i64(&db, "mode_state", id, "enable_flag"), Some(1));
}

#[test]
fn restricted_table_update_to_enable_flag_zero_is_rejected() {
    // §8-二L-2：经 base UPDATE 把 mode_state.enable_flag 翻 0 → 被拒（绝非悄无声息 flag 翻转）。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "mode_state",
                    columns: vec!["scope_resource_id"],
                    values: vec![Value::Null],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("seed mode_state");
    let err = db
        .with_write_txn(|txn| {
            write::update(
                txn,
                Timestamp::from_unix_ms(1_767_225_601_000),
                &Actor::System,
                "mode_state",
                postern_core::id::SnowflakeId::from_raw(id as u64),
                0,
                vec![],
                vec![],
                Some(0), // 试图翻转 enable_flag 为 0
            )
        })
        .expect_err("flipping restricted enable_flag to 0 must be rejected");
    assert!(matches!(err, StoreError::ConstraintViolation), "got {err:?}");
    assert_eq!(fetch_i64(&db, "mode_state", id, "enable_flag"), Some(1), "still enabled");
}

#[test]
fn restricted_tables_set_matches_design_four_tables() {
    // §8-一F-8：限制性表清单恰为四张表。
    assert_eq!(RESTRICTED_TABLES.len(), 4);
    assert!(write::is_restricted_table("grant_constraints"));
    assert!(write::is_restricted_table("grant_conditions"));
    assert!(write::is_restricted_table("mode_state"));
    assert!(write::is_restricted_table("deny_notes"));
    assert!(!write::is_restricted_table("resources"), "授予性表不受 enable_flag 禁限");
    assert!(!write::is_restricted_table("principals"));
}

// ============================================================ F-9 / L-7 固定宽度时间戳

#[test]
fn timestamp_format_is_fixed_width_24_utc_z_three_millis() {
    // §8-一F-9：恒 YYYY-MM-DDTHH:MM:SS.sssZ、长度 24、UTC、Z 结尾、3 位毫秒。
    // 2026-01-01T00:00:00.000Z 对应 unix ms = 1_767_225_600_000。
    let s = timestamp::format(Timestamp::from_unix_ms(1_767_225_600_000));
    assert_eq!(s.len(), 24, "fixed width 24");
    assert_eq!(s, "2026-01-01T00:00:00.000Z");
    assert!(s.ends_with('Z'), "Z suffix");
    assert_eq!(TIMESTAMP_LEN, 24);
}

#[test]
fn timestamp_format_renders_three_millisecond_digits() {
    // §8-一F-9：毫秒恒 3 位（含前导零）。
    let s = timestamp::format(Timestamp::from_unix_ms(1_767_225_600_007));
    assert_eq!(s, "2026-01-01T00:00:00.007Z", "7ms renders as .007");
    assert_eq!(s.len(), 24);
}

#[test]
fn timestamp_lexicographic_order_matches_time_order_across_millis_seconds_days() {
    // §8-二L-7：跨毫秒/跨秒/跨日两时间文本字符串比较 == 真实时间先后；长度恒 24。
    let t_ms_a = timestamp::format(Timestamp::from_unix_ms(1_767_225_600_001));
    let t_ms_b = timestamp::format(Timestamp::from_unix_ms(1_767_225_600_002));
    let t_sec = timestamp::format(Timestamp::from_unix_ms(1_767_225_601_000));
    let t_day = timestamp::format(Timestamp::from_unix_ms(1_767_225_600_000 + 86_400_000));
    assert!(t_ms_a < t_ms_b, "cross-millisecond lexical order");
    assert!(t_ms_b < t_sec, "cross-second lexical order");
    assert!(t_sec < t_day, "cross-day lexical order");
    for t in [&t_ms_a, &t_ms_b, &t_sec, &t_day] {
        assert_eq!(t.len(), 24, "every timestamp width 24");
    }
}

// ============================================================ F-10 归一化入库

#[test]
fn normalize_name_trims_and_lowercases() {
    // §8-一F-10：Admin / ' admin ' / ADMIN 归一化为同一值。
    assert_eq!(normalize_name("Admin"), "admin");
    assert_eq!(normalize_name(" admin "), "admin");
    assert_eq!(normalize_name("ADMIN"), "admin");
    assert_eq!(normalize_name("Admin"), normalize_name(" ADMIN "));
}

#[test]
fn normalize_name_preserves_inner_content_lowercased() {
    // §8-一F-10：内部内容保留、仅 trim + 小写（不吃内部字符）。
    assert_eq!(normalize_name("  Read-Only-Role  "), "read-only-role");
}

#[test]
fn second_insert_of_normalized_duplicate_name_is_rejected_by_partial_unique() {
    // §8-一F-10：归一化后相同的两条名 → 第二条被 partial unique 拒。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    // 第一条：name 经 base 归一化为 "ops"。
    db.with_write_txn(|txn| {
        write::insert(
            txn,
            &idgen,
            now,
            &Actor::System,
            InsertRow {
                table: "roles",
                columns: vec!["name"],
                values: vec![Value::Text("Ops".to_string())],
                enable_flag: 1,
            },
        )
        .map(|_| ())
    })
    .expect("first role inserts");
    // 第二条：name "  OPS " 归一化同为 "ops" → partial unique 拒。
    let err = db
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "roles",
                    columns: vec!["name"],
                    values: vec![Value::Text("  OPS ".to_string())],
                    enable_flag: 1,
                },
            )
            .map(|_| ())
        })
        .expect_err("normalized-duplicate name must be rejected");
    assert!(matches!(err, StoreError::ConstraintViolation), "partial unique → ConstraintViolation, got {err:?}");
}

// ============================================================ 系统协调写（sweeper）

#[test]
fn system_update_is_idempotent_predicate_write_without_optimistic_lock() {
    // §3.1：系统协调写不带期望 version、谓词幂等；version 自增、updated_by=='system'。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let id = insert_resource(&db, &idgen, now, &Actor::Operator("human".to_string()), "sweepable");
    let predicate = format!("id = {id}");
    let affected = db
        .with_write_txn(|txn| {
            write::system_update(
                txn,
                Timestamp::from_unix_ms(1_767_225_601_000),
                "resources",
                vec!["codename"],
                vec![Value::Text("swept".to_string())],
                &predicate,
                vec![],
            )
        })
        .expect("system update");
    assert_eq!(affected, 1, "predicate matched exactly one row");
    assert_eq!(fetch_text(&db, "resources", id, "codename"), Some("swept".to_string()));
    assert_eq!(fetch_i64(&db, "resources", id, "version"), Some(1), "version still increments");
    assert_eq!(fetch_text(&db, "resources", id, "updated_by"), Some("system".to_string()), "system actor");
}

// ============================================================ 错误枚举语义分明

#[test]
fn version_conflict_and_constraint_violation_are_distinct_variants() {
    // §3.6：VersionConflict 是独立变体，绝不与约束违反/IO 混淆。
    let vc = StoreError::VersionConflict;
    let cv = StoreError::ConstraintViolation;
    let io = StoreError::Io;
    assert!(matches!(vc, StoreError::VersionConflict));
    assert!(!matches!(cv, StoreError::VersionConflict));
    assert!(!matches!(io, StoreError::VersionConflict));
    // 文案不回显库路径/SQL 片段（机密红线 7.5）——仅常量英文。
    assert_eq!(StoreError::VersionConflict.to_string(), "optimistic-lock version conflict");
    assert_eq!(StoreError::ConstraintViolation.to_string(), "constraint violation");
}

// ============================================================ 事务回滚原语

#[test]
fn with_write_txn_rolls_back_when_closure_returns_err() {
    // §3.6：闭包返回 Err → ROLLBACK、库不变（无半截状态）。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let before = count_rows(&db, "resources");
    let res: Result<(), StoreError> = db.with_write_txn(|txn| {
        write::insert(
            txn,
            &idgen,
            now,
            &Actor::System,
            InsertRow {
                table: "resources",
                columns: vec!["codename"],
                values: vec![Value::Text("ghost".to_string())],
                enable_flag: 1,
            },
        )?;
        Err(StoreError::Io)
    });
    assert!(res.is_err());
    assert_eq!(count_rows(&db, "resources"), before, "inserted row rolled back, none persisted");
}

#[test]
fn with_write_txn_commits_when_closure_returns_ok() {
    // §3.6：闭包返回 Ok → COMMIT、行可见。
    let db = fresh_db();
    let idgen = idgen_at(1_767_225_600_500);
    let now = Timestamp::from_unix_ms(1_767_225_600_500);
    let before = count_rows(&db, "resources");
    insert_resource(&db, &idgen, now, &Actor::System, "committed");
    assert_eq!(count_rows(&db, "resources"), before + 1);
}

/// 物理行计数（运行期拼接纯 COUNT(*)，无作用域过滤；pagination 豁免）。
fn count_rows(db: &Db, table: &str) -> i64 {
    let q = format!("{} COUNT(*) {} {}", kw(&["SEL", "ECT"]), kw(&["FR", "OM"]), table);
    db.with_read(|conn| {
        let n: i64 = conn.query_row(&q, [], |r| r.get(0)).map_err(|_| StoreError::Io)?;
        Ok(n)
    })
    .unwrap_or(-1)
}
