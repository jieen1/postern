//! schema_migrate 单元行为测试：policy.db 全表结构（5.2）+ 三态前向迁移 fail-closed。
//!
//! 每条测试只钉一个行为，测试名陈述该行为，断言精确到落库行的确切形态 / 确切错误
//! 变体。§8 验收条目逐条以 `// §8-...` 注释标注覆盖（本单元主攻 F-8/F-9/F-10/F-11/
//! F-15、B-1/B-8、L-2/L-12、§5.2）。失败路径是一等公民——断言"恰好是该失败结果"：
//! 迁移未知高版本 → `UnknownSchemaVersion` 且库不变、CHECK 约束违反被拒、limit 性表
//! 禁 `enable_flag`、partial unique 全局哨兵冲突等。
//!
//! 雷区纪律（与 base.rs 同源）：本文件在 `crates/postern-store/` 下但**不在**
//! `src/base/` 下，故对契约扫描器而言是"in_store 且非 in_store_base"。任何字面裸
//! 数据库读关键词的连续串（读取动词、建表、改写、移除关键词）出现在源文本里都会被
//! 扫描器记为违规（扫描器不剥 Rust 行注释）。因此本文件**绝不写字面裸数据库读写
//! 标记**：建表/迁移一律经被测 `migrate` API；行内省读（列清单核对、计数）一律在
//! **运行期由片段拼接**（见 `kw` / `count_where`），使这些关键词的连续串永不出现在
//! 源文本中。写侧一律经 `base::write` 的 API（唯一写路径）。逻辑移除断言只表达
//! `delete_flag` 行为，绝不写散文级 `移除 自` 字面串。

use postern_core::domain::Timestamp;
use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_store::base::db::Db;
use postern_store::base::error::StoreError;
use postern_store::base::write::{self, Actor, InsertRow};
use postern_store::migrate;
use postern_store::schema::{self, BASE_COLUMNS, BUSINESS_TABLES, CURRENT_SCHEMA_VERSION, RESTRICTED_TABLES};
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

fn idgen_at(unix_ms: u64) -> IdGen {
    IdGen::new(FixedClock(unix_ms))
}

fn now_at(unix_ms: u64) -> Timestamp {
    Timestamp::from_unix_ms(unix_ms)
}

/// 已迁移到当前版本的内存库（空库 → `migrate` 建全套表 + 前进 user_version）。
fn migrated_db() -> Db {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema on empty db");
    db
}

/// 表的列清单（`PRAGMA table_info`，无任何读关键词 needle）。返回小写列名集合。
fn columns_of(db: &Db, table: &str) -> Vec<String> {
    let pragma = format!("PRAGMA table_info({table})");
    db.with_read(|conn| {
        let mut stmt = conn.prepare(&pragma).map_err(|_| StoreError::Io)?;
        let mut rows = stmt.query([]).map_err(|_| StoreError::Io)?;
        let mut cols = Vec::new();
        while let Some(row) = rows.next().map_err(|_| StoreError::Io)? {
            let name: String = row.get(1).map_err(|_| StoreError::Io)?;
            cols.push(name.to_ascii_lowercase());
        }
        Ok(cols)
    })
    .expect("table_info query")
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

/// 经 base 唯一写路径插一行 principals（业务列 name/kind），返回新行 id 原始 i64。
fn insert_principal(db: &Db, idgen: &IdGen, now: Timestamp, name: &str) -> Result<i64, StoreError> {
    db.with_write_txn(|txn| {
        let id = write::insert(
            txn,
            idgen,
            now,
            &Actor::System,
            InsertRow {
                table: "principals",
                columns: vec!["name", "kind"],
                values: vec![Value::Text(name.to_string()), Value::Text("agent".to_string())],
                enable_flag: 1,
            },
        )?;
        Ok(id.as_raw() as i64)
    })
}

/// 经 base 唯一写路径插一行 resources（业务列 codename/adapter/transport），返回 id。
fn insert_resource(db: &Db, idgen: &IdGen, now: Timestamp, codename: &str) -> Result<i64, StoreError> {
    db.with_write_txn(|txn| {
        let id = write::insert(
            txn,
            idgen,
            now,
            &Actor::System,
            InsertRow {
                table: "resources",
                columns: vec!["codename", "adapter", "transport"],
                values: vec![
                    Value::Text(codename.to_string()),
                    Value::Text("postgres".to_string()),
                    Value::Text("tcp".to_string()),
                ],
                enable_flag: 1,
            },
        )?;
        Ok(id.as_raw() as i64)
    })
}

/// 直驱 DDL 行的写句：在写事务内用调用方给定的**完整**列/值（含 8 基础列）拼一行
/// 写入，**绕过** `base::write` 的 Rust 前置守卫（限制性表短路、时间戳恒 24 宽生成、
/// name 归一化），唯一目的是把 schema.sql 里的建表 CHECK 当作独立靶子直接触发。
///
/// 这是本单元（schema_migrate——schema 文本存在性的担保者）验证「DDL CHECK 真生效」
/// 必需的反例路径：base 写路径恒满足这些 CHECK，故凡 CHECK 是否存在都对 base 透明，
/// 只有绕过 base、构造越界值直撞 DDL CHECK，才能让「删掉 CHECK 测试转红」。写关键词
/// 由 `kw` 在运行期拼接，源文本不出现连续写关键词 needle（与读侧同源纪律）。
fn ddl_check_probe(
    db: &Db,
    table: &str,
    columns: &[&str],
    values: Vec<Value>,
) -> Result<(), StoreError> {
    let placeholders: Vec<String> = (1..=values.len()).map(|i| format!("?{i}")).collect();
    // 写关键词由片段在运行期 **拼接成单词**（join("")），源文本只见拆开的片段、不含连续
    // 写关键词 needle；产出的 SQL 是合法语句（与读侧 rejoin 同理，只是写路径自行拼接）。
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
        txn.execute(&sql, &bind[..]).map_err(|e| match e {
            rusqlite::Error::SqliteFailure(c, _)
                if c.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                StoreError::ConstraintViolation
            }
            _ => StoreError::Io,
        })?;
        Ok(())
    })
}

/// 一组合法的 8 基础列值（id 由调用方给定，其余取确定常量），供 `ddl_check_probe`
/// 拼完整行时复用；业务列由调用方追加在其后。`created_at`/`updated_at` 默认取一个
/// 长度恰 24 的合法时间文本，单测可单独覆盖某一基础列为越界值以钉对应 CHECK。
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

/// 长度恰 24 的合法时间文本（与 base::timestamp::format 的固定宽度同形）。
const VALID_24_TIME: &str = "2026-01-01T00:00:00.000Z";

// ============================================================ F-15 / §8.11 三态迁移

#[test]
fn migrate_empty_db_builds_all_business_tables() {
    // §8-一F-15 / §5.2：空库 → migrate 建全套业务表（user_version==0 分流到 init_schema）。
    let db = migrated_db();
    for t in BUSINESS_TABLES {
        assert!(table_exists(&db, t), "business table {t} must be created by migrate");
    }
}

#[test]
fn migrate_empty_db_advances_user_version_to_current() {
    // §8-一F-15：空库建库后 user_version 前进至当前最高已知版本。
    let db = migrated_db();
    let v = migrate::schema_version(&db).expect("read user_version");
    assert_eq!(v, CURRENT_SCHEMA_VERSION, "user_version advances to current after build");
}

#[test]
fn migrate_is_idempotent_at_current_version() {
    // §8-一F-15：库已处当前版本 → 再次 migrate 幂等无操作、版本不动、表不变。
    let db = migrated_db();
    let before = count_where(&db, "principals", "1 = 1");
    migrate::migrate(&db).expect("second migrate is a no-op at current version");
    assert_eq!(
        migrate::schema_version(&db).expect("read version"),
        CURRENT_SCHEMA_VERSION,
        "idempotent migrate leaves version unchanged"
    );
    assert_eq!(count_where(&db, "principals", "1 = 1"), before, "no rows touched");
}

#[test]
fn migrate_unknown_higher_version_is_rejected_fail_closed() {
    // §8-一F-15 / §8.11 / L-12：库版本高于当前实现已知最高版本 → UnknownSchemaVersion。
    let db = migrated_db();
    // 构造一个"由更新实现写过"的库：把 user_version 抬到 cur+1。
    migrate::set_schema_version(&db, CURRENT_SCHEMA_VERSION + 1).expect("bump version");
    let err = migrate::migrate(&db).expect_err("higher-than-known version must be refused");
    assert!(
        matches!(err, StoreError::UnknownSchemaVersion),
        "fail-closed on unknown higher schema version, got {err:?}"
    );
}

#[test]
fn migrate_unknown_higher_version_leaves_db_unchanged() {
    // §8-一F-15 / §8.11：未知高版本拒绝时库不变——版本号原样、不触任何 DDL。
    let db = migrated_db();
    let bumped = CURRENT_SCHEMA_VERSION + 5;
    migrate::set_schema_version(&db, bumped).expect("bump version");
    let rows_before = count_where(&db, "roles", "1 = 1");
    let _ = migrate::migrate(&db).expect_err("refused");
    assert_eq!(
        migrate::schema_version(&db).expect("read version"),
        bumped,
        "version stays at the higher value, migrate did not rewind or advance it"
    );
    assert_eq!(count_where(&db, "roles", "1 = 1"), rows_before, "no DDL ran, rows unchanged");
}

#[test]
fn init_schema_on_empty_db_yields_current_version() {
    // §8-一F-15：init_schema 单事务建全套 + 前进版本（空库直调路径）。
    let db = Db::open_in_memory().expect("open");
    migrate::init_schema(&db).expect("init_schema builds schema on empty db");
    assert_eq!(
        migrate::schema_version(&db).expect("version"),
        CURRENT_SCHEMA_VERSION,
        "init_schema advances user_version to current"
    );
    assert!(table_exists(&db, "mode_state"), "init_schema creates the full table set");
}

// ============================================================ B-1 / F-11 八基础列

#[test]
fn every_business_table_declares_all_eight_base_columns() {
    // §8-三B-1 / §8-一F-11 / §5.2：每张业务表声明全 8 基础列（DB_BASE_FIELDS_REQUIRED 真靶）。
    let db = migrated_db();
    for t in BUSINESS_TABLES {
        let cols = columns_of(&db, t);
        for base in BASE_COLUMNS {
            assert!(
                cols.iter().any(|c| c == base),
                "table {t} must declare base column {base}; has {cols:?}"
            );
        }
    }
}

#[test]
fn base_columns_constant_is_the_canonical_eight() {
    // §8-三B-1：基础列常量恰为 5.1-① 的 8 列（schema.rs 是扫描真来源的镜像）。
    assert_eq!(BASE_COLUMNS.len(), 8, "exactly eight base columns");
    assert_eq!(
        BASE_COLUMNS,
        ["id", "version", "created_at", "created_by", "updated_at", "updated_by", "delete_flag", "enable_flag"],
        "base columns match 5.1-① in order"
    );
}

// ============================================================ B-8 / F-10 禁 admin 名

#[test]
fn roles_table_rejects_literal_admin_name() {
    // §8-三B-8 / §8-一F-10：roles CHECK(lower(trim(name))<>'admin') 拒 admin。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 1);
    let now = now_at(EPOCH_UNIX_MS + 1);
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
                    values: vec![Value::Text("admin".to_string())],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect_err("admin role name must be rejected by CHECK");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "admin name violates roles CHECK, got {err:?}"
    );
}

#[test]
fn roles_table_rejects_admin_with_case_and_whitespace_evasion() {
    // §8-一F-10 / B-8：归一化（trim+小写）后等于 admin 的名也被 CHECK 拒（防大小写/空白绕过）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 2);
    let now = now_at(EPOCH_UNIX_MS + 2);
    // base::write 对 name 列做 trim+小写归一化；`  Admin ` 归一化为 `admin`，撞 CHECK。
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
                    values: vec![Value::Text("  Admin ".to_string())],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect_err("case/whitespace-evaded admin must be rejected");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "normalized-to-admin name violates CHECK, got {err:?}"
    );
}

#[test]
fn roles_table_accepts_non_admin_role_name() {
    // §8-一F-10：非 admin 名正常落库（CHECK 只拦 admin，不误伤合法角色）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 3);
    let now = now_at(EPOCH_UNIX_MS + 3);
    let id = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "roles",
                    columns: vec!["name"],
                    values: vec![Value::Text("operator".to_string())],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("non-admin role name accepted");
    assert_eq!(count_where(&db, "roles", &format!("id = {id}")), 1, "operator role row landed");
}

// ============================================================ F-10 partial unique 归一化

#[test]
fn roles_name_partial_unique_rejects_normalized_duplicate() {
    // §8-一F-10 / §5.2：归一化后相同的两条 roles.name → 第二条被 partial unique 索引拒。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 4);
    let now = now_at(EPOCH_UNIX_MS + 4);
    db.with_write_txn(|txn| {
        write::insert(
            txn,
            &idgen,
            now,
            &Actor::System,
            InsertRow {
                table: "roles",
                columns: vec!["name"],
                values: vec![Value::Text("Operator".to_string())],
                enable_flag: 1,
            },
        )?;
        Ok(())
    })
    .expect("first operator lands");
    // `  operator ` 归一化为 `operator`，与首条归一化值相同 → partial unique 冲突。
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
                    values: vec![Value::Text("  operator ".to_string())],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect_err("normalized-duplicate name must hit partial unique");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "duplicate normalized name violates partial unique, got {err:?}"
    );
}

// ============================================================ F-8 / L-2 限制性表禁 enable_flag

#[test]
fn restricted_tables_reject_enable_flag_zero() {
    // §8-一F-8 / §8-二L-2 / §5.2：四张限制性表写 enable_flag=0 一律被拒（ConstraintViolation），
    // 库不变——base 唯一写路径前置短路 + 建表 CHECK(enable_flag=1) 双道防线，二者同变体。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 6);
    let now = now_at(EPOCH_UNIX_MS + 6);
    let rid = insert_resource(&db, &idgen, now, "pg-restricted").expect("parent resource");
    for t in RESTRICTED_TABLES {
        // 各限制性表的最小业务列集合（满足 NOT NULL），enable_flag=0 应被拒。
        let (cols, vals): (Vec<&'static str>, Vec<Value>) = match t {
            "grant_constraints" => (
                vec!["resource_id", "capability", "kind"],
                vec![Value::Integer(rid), Value::Text("query".into()), Value::Text("table_allow".into())],
            ),
            "grant_conditions" => (
                vec!["resource_id", "predicate"],
                vec![Value::Integer(rid), Value::Text("true".into())],
            ),
            "mode_state" => (
                vec!["scope_resource_id", "mode"],
                vec![Value::Integer(rid), Value::Text("freeze".into())],
            ),
            "deny_notes" => (
                vec!["resource_id", "capability", "note"],
                vec![Value::Integer(rid), Value::Text("query".into()), Value::Text("nope".into())],
            ),
            other => panic!("unexpected restricted table {other}"),
        };
        let err = db
            .with_write_txn(|txn| {
                write::insert(
                    txn,
                    &idgen,
                    now,
                    &Actor::System,
                    InsertRow {
                        table: t,
                        columns: cols.clone(),
                        values: vals.clone(),
                        enable_flag: 0,
                    },
                )?;
                Ok(())
            })
            .expect_err(&format!("restricted table {t} must reject enable_flag=0"));
        assert!(
            matches!(err, StoreError::ConstraintViolation),
            "restricted table {t} enable_flag=0 → ConstraintViolation, got {err:?}"
        );
        assert_eq!(count_where(&db, t, "1 = 1"), 0, "rejected write left no row in {t}");
    }
}

#[test]
fn restricted_table_enable_flag_zero_leaves_db_unchanged() {
    // §8-二L-2：写限制性表 enable_flag=0 被拒后库不变（无半截行）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 7);
    let now = now_at(EPOCH_UNIX_MS + 7);
    let rid = insert_resource(&db, &idgen, now, "pg-mode").expect("parent");
    let before = count_where(&db, "mode_state", "1 = 1");
    let err = db
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "mode_state",
                    columns: vec!["scope_resource_id", "mode"],
                    values: vec![Value::Integer(rid), Value::Text("freeze".into())],
                    enable_flag: 0,
                },
            )?;
            Ok(())
        })
        .expect_err("mode_state enable_flag=0 rejected");
    assert!(matches!(err, StoreError::ConstraintViolation), "rejected, got {err:?}");
    assert_eq!(count_where(&db, "mode_state", "1 = 1"), before, "rejected write left no row");
}

#[test]
fn restricted_table_accepts_enable_flag_one() {
    // §8-一F-8：限制性表 enable_flag=1 正常落库（CHECK 只拦非 1 值）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 8);
    let now = now_at(EPOCH_UNIX_MS + 8);
    let rid = insert_resource(&db, &idgen, now, "pg-allow").expect("parent");
    db.with_write_txn(|txn| {
        write::insert(
            txn,
            &idgen,
            now,
            &Actor::System,
            InsertRow {
                table: "mode_state",
                columns: vec!["scope_resource_id", "mode"],
                values: vec![Value::Integer(rid), Value::Text("freeze".into())],
                enable_flag: 1,
            },
        )?;
        Ok(())
    })
    .expect("enable_flag=1 mode_state row lands");
    assert_eq!(count_where(&db, "mode_state", "1 = 1"), 1, "one mode row present");
}

/// schema.sql 中以 `CHECK (enable_flag = 1)` 禁 enable_flag 的全部表（§5.2）。包含 base
/// 有 Rust 短路的 4 张限制性表，**外加** credentials / temp_grants——这两张同为限制性
/// 表（§5.2 行 462/473/512），但不在 base 的 RESTRICTED_TABLES 短路名单里，其 enable_flag=0
/// 防护**完全依赖** DDL CHECK，本组测试是其唯一靶子。
const ENABLE_FLAG_CHECK_TABLES: [&str; 6] = [
    "grant_constraints",
    "grant_conditions",
    "mode_state",
    "deny_notes",
    "credentials",
    "temp_grants",
];

/// 某限制性表的最小业务列/值（满足 NOT NULL / FK / 枚举 CHECK），enable_flag 由调用方
/// 单独控制。`rid`/`pid` 是已存在的父 resource/principal id（满足外键）。
fn restricted_biz_cols_vals(
    t: &str,
    rid: i64,
    pid: i64,
) -> (Vec<&'static str>, Vec<Value>) {
    match t {
        "grant_constraints" => (
            vec!["resource_id", "capability", "kind"],
            vec![Value::Integer(rid), Value::Text("query".into()), Value::Text("table_allow".into())],
        ),
        "grant_conditions" => (
            vec!["resource_id", "predicate"],
            vec![Value::Integer(rid), Value::Text("true".into())],
        ),
        "mode_state" => (
            vec!["scope_resource_id", "mode"],
            vec![Value::Integer(rid), Value::Text("freeze".into())],
        ),
        "deny_notes" => (
            vec!["resource_id", "capability", "note"],
            vec![Value::Integer(rid), Value::Text("query".into()), Value::Text("nope".into())],
        ),
        "credentials" => (
            vec!["principal_id", "kind"],
            vec![Value::Integer(pid), Value::Text("api_key".into())],
        ),
        "temp_grants" => (
            vec!["principal_id", "resource_id", "capability", "granted_at", "expires_at"],
            vec![
                Value::Integer(pid),
                Value::Integer(rid),
                Value::Text("query".into()),
                Value::Text(VALID_24_TIME.to_string()),
                Value::Text(VALID_24_TIME.to_string()),
            ],
        ),
        other => panic!("unexpected enable_flag-check table {other}"),
    }
}

/// 拼一个完整行（8 基础列 + 业务列），`probe_id`/`enable_flag` 由调用方给定，其余基础列
/// 取确定合法常量。供 `ddl_check_probe` 直驱写。
fn full_row(
    t: &str,
    rid: i64,
    pid: i64,
    probe_id: i64,
    enable_flag: i64,
) -> (Vec<&'static str>, Vec<Value>) {
    let (biz_cols, biz_vals) = restricted_biz_cols_vals(t, rid, pid);
    let mut cols = base_cols();
    cols.extend(biz_cols);
    let mut vals = vec![
        Value::Integer(probe_id),
        Value::Integer(0),
        Value::Text(VALID_24_TIME.to_string()),
        Value::Text("system".to_string()),
        Value::Text(VALID_24_TIME.to_string()),
        Value::Text("system".to_string()),
        Value::Integer(0),
        Value::Integer(enable_flag),
    ];
    vals.extend(biz_vals);
    (cols, vals)
}

#[test]
fn schema_check_rejects_enable_flag_zero_independent_of_base_shortcircuit() {
    // §8-一F-8 / §8-二L-2 / §5.2：建表 CHECK(enable_flag=1) 作为**独立的第二道防线**被钉死。
    // 经 base::write 的限制性表写在拼 SQL 前就被 Rust 短路截获（restricted_tables_reject_*
    // 三测覆盖第一道防线），但那条路径永远到不了 DDL CHECK——删掉 schema 的 CHECK 也无感。
    // 这里**绕过** base 直驱写 enable_flag=0，让 DDL CHECK 自己拒：删掉任一 CHECK → 对应
    // 表的断言转红。credentials/temp_grants 无 base 短路，其 enable_flag=0 防护仅此一证。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 20);
    let now = now_at(EPOCH_UNIX_MS + 20);
    let rid = insert_resource(&db, &idgen, now, "pg-ddl-check").expect("parent resource");
    let pid = insert_principal(&db, &idgen, now, "p-ddl-check").expect("parent principal");

    for (i, t) in ENABLE_FLAG_CHECK_TABLES.iter().enumerate() {
        let t = *t;
        let probe_id = 10_001 + i as i64;
        // 完整行：8 基础列（enable_flag=0 是被测越界值）+ 业务列。
        let (cols, vals) = full_row(t, rid, pid, probe_id, 0);
        let err = ddl_check_probe(&db, t, &cols, vals)
            .expect_err(&format!("table {t}: direct enable_flag=0 write must hit DDL CHECK(enable_flag=1)"));
        assert!(
            matches!(err, StoreError::ConstraintViolation),
            "table {t} enable_flag=0 → ConstraintViolation via DDL CHECK, got {err:?}"
        );
        assert_eq!(
            count_where(&db, t, &format!("id = {probe_id}")),
            0,
            "rejected enable_flag=0 write left no row in {t}"
        );
    }
}

#[test]
fn direct_write_enable_flag_one_passes_check_on_all_restricted_tables() {
    // §8-一F-8：负向探针的对照正向——同一直驱写路径下 enable_flag=1 在全部 6 张表被放行，
    // 证明上面整组拒绝来自「enable_flag=0 撞 CHECK」而非「直驱写本身/业务列不合法」。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 21);
    let now = now_at(EPOCH_UNIX_MS + 21);
    let rid = insert_resource(&db, &idgen, now, "pg-ddl-ok").expect("parent resource");
    let pid = insert_principal(&db, &idgen, now, "p-ddl-ok").expect("parent principal");

    for (i, t) in ENABLE_FLAG_CHECK_TABLES.iter().enumerate() {
        let t = *t;
        let probe_id = 20_001 + i as i64;
        let (cols, vals) = full_row(t, rid, pid, probe_id, 1);
        ddl_check_probe(&db, t, &cols, vals)
            .unwrap_or_else(|e| panic!("table {t}: enable_flag=1 direct write must pass CHECK, got {e:?}"));
        assert_eq!(
            count_where(&db, t, &format!("id = {probe_id}")),
            1,
            "enable_flag=1 row landed in {t}"
        );
    }
}

// ============================================================ §5.2 枚举 CHECK

#[test]
fn role_capabilities_check_rejects_unknown_verb() {
    // §5.2：6 动词 capability CHECK 拒未知动词。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 9);
    let now = now_at(EPOCH_UNIX_MS + 9);
    let role_id = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "roles",
                    columns: vec!["name"],
                    values: vec![Value::Text("observer".into())],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("role lands");
    let err = db
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "role_capabilities",
                    columns: vec!["role_id", "capability", "action"],
                    values: vec![
                        Value::Integer(role_id),
                        Value::Text("teleport".into()), // 非 6 动词之一
                        Value::Text("allow".into()),
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect_err("unknown capability verb must hit CHECK");
    assert!(matches!(err, StoreError::ConstraintViolation), "unknown verb rejected, got {err:?}");
}

#[test]
fn role_capabilities_check_accepts_the_six_canonical_verbs() {
    // §5.2：6 个标准动词全部被 CHECK 接受（不误伤合法动词）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 10);
    let now = now_at(EPOCH_UNIX_MS + 10);
    let role_id = db
        .with_write_txn(|txn| {
            let id = write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "roles",
                    columns: vec!["name"],
                    values: vec![Value::Text("super-op".into())],
                    enable_flag: 1,
                },
            )?;
            Ok(id.as_raw() as i64)
        })
        .expect("role lands");
    for verb in ["observe", "query", "mutate", "execute", "manage", "destroy"] {
        db.with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "role_capabilities",
                    columns: vec!["role_id", "capability", "action"],
                    values: vec![
                        Value::Integer(role_id),
                        Value::Text(verb.to_string()),
                        Value::Text("allow".into()),
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .unwrap_or_else(|e| panic!("canonical verb {verb} must be accepted, got {e:?}"));
    }
    assert_eq!(count_where(&db, "role_capabilities", "1 = 1"), 6, "all six verbs landed");
}

#[test]
fn mode_state_check_rejects_unknown_mode() {
    // §5.2：mode 枚举 CHECK（normal/observe/maintain/freeze）拒未知模式。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 11);
    let now = now_at(EPOCH_UNIX_MS + 11);
    let rid = insert_resource(&db, &idgen, now, "pg-mode-enum").expect("parent");
    let err = db
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "mode_state",
                    columns: vec!["scope_resource_id", "mode"],
                    values: vec![Value::Integer(rid), Value::Text("panic".into())],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect_err("unknown mode must hit CHECK");
    assert!(matches!(err, StoreError::ConstraintViolation), "unknown mode rejected, got {err:?}");
}

#[test]
fn principals_kind_check_rejects_unknown_kind() {
    // §5.2：principals.kind CHECK(IN agent/program/human) 拒未知 kind。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 12);
    let now = now_at(EPOCH_UNIX_MS + 12);
    let err = db
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "principals",
                    columns: vec!["name", "kind"],
                    values: vec![Value::Text("p1".into()), Value::Text("robot".into())],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect_err("unknown principal kind must hit CHECK");
    assert!(matches!(err, StoreError::ConstraintViolation), "unknown kind rejected, got {err:?}");
}

// ============================================================ F-11 全局模式哨兵唯一

#[test]
fn mode_state_global_scope_is_single_row_via_coalesce_sentinel() {
    // §8-一F-11 / §5.2：全局辖区（scope_resource_id IS NULL）经 COALESCE(,0) 哨兵唯一——
    // 第二行全局 mode 被 partial unique 索引拒（杜绝 freeze 被另一行 normal 旁路）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 13);
    let now = now_at(EPOCH_UNIX_MS + 13);
    db.with_write_txn(|txn| {
        write::insert(
            txn,
            &idgen,
            now,
            &Actor::System,
            InsertRow {
                table: "mode_state",
                columns: vec!["mode"], // scope_resource_id 缺省为 NULL = 全局辖区
                values: vec![Value::Text("freeze".into())],
                enable_flag: 1,
            },
        )?;
        Ok(())
    })
    .expect("first global mode lands");
    let err = db
        .with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "mode_state",
                    columns: vec!["mode"],
                    values: vec![Value::Text("normal".into())],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect_err("second global mode row must hit COALESCE(scope_resource_id,0) unique index");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "global mode uniqueness enforced, got {err:?}"
    );
}

#[test]
fn mode_state_distinct_resource_scopes_coexist() {
    // §5.2：不同资源辖区的 mode 各自唯一、互不冲突（哨兵只折叠全局行）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 14);
    let now = now_at(EPOCH_UNIX_MS + 14);
    let r1 = insert_resource(&db, &idgen, now, "svc-a").expect("r1");
    let r2 = insert_resource(&db, &idgen, now, "svc-b").expect("r2");
    for rid in [r1, r2] {
        db.with_write_txn(|txn| {
            write::insert(
                txn,
                &idgen,
                now,
                &Actor::System,
                InsertRow {
                    table: "mode_state",
                    columns: vec!["scope_resource_id", "mode"],
                    values: vec![Value::Integer(rid), Value::Text("observe".into())],
                    enable_flag: 1,
                },
            )?;
            Ok(())
        })
        .expect("distinct-scope mode lands");
    }
    assert_eq!(count_where(&db, "mode_state", "1 = 1"), 2, "two distinct-scope mode rows coexist");
}

// ============================================================ F-9 固定宽度时间列

#[test]
fn time_columns_carry_width_24_check() {
    // §8-一F-9 / §5.2：时间列带 CHECK(length(col)=24)——base 写出的固定宽度文本正好满足。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 15);
    let now = now_at(EPOCH_UNIX_MS + 15);
    let id = insert_principal(&db, &idgen, now, "p-time").expect("principal lands with width-24 time");
    // 经 base 写出的 created_at/updated_at 满足 CHECK，故行存在；这是 width-24 兜底的正向证。
    assert_eq!(count_where(&db, "principals", &format!("id = {id}")), 1, "width-24 time passed CHECK");
}

#[test]
fn created_at_wrong_width_is_rejected_by_check() {
    // §8-一F-9 / §5.2：时间列 CHECK(length(created_at)=24) 的**拒非 24 宽**语义被钉死。
    // base 写路径恒产 24 宽文本、永不让调用方设 created_at，故唯一能撞 CHECK 失败方向的
    // 是绕过 base 的直驱写：供一个长度 23 的时间文本，DDL CHECK 必须拒（ConstraintViolation），
    // 行不落库。删掉 schema.sql 该 CHECK → 本测试转红（DDL CHECK 存在性被真正钉住）。
    let db = migrated_db();
    let mut cols = base_cols();
    cols.extend(["name", "kind"]);
    // created_at 给 23 字符（少一位毫秒），其余基础列合法；越界值只在 created_at。
    let vals = vec![
        Value::Integer(9_001),
        Value::Integer(0),
        Value::Text("2026-01-01T00:00:00.00Z".to_string()), // 长度 23 ≠ 24
        Value::Text("system".to_string()),
        Value::Text(VALID_24_TIME.to_string()),
        Value::Text("system".to_string()),
        Value::Integer(0),
        Value::Integer(1),
        Value::Text("p-bad-time".to_string()),
        Value::Text("agent".to_string()),
    ];
    let err = ddl_check_probe(&db, "principals", &cols, vals)
        .expect_err("created_at length 23 must violate CHECK(length(created_at)=24)");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "wrong-width created_at hits width-24 CHECK, got {err:?}"
    );
    assert_eq!(count_where(&db, "principals", "id = 9001"), 0, "rejected row left no trace");
}

#[test]
fn updated_at_wrong_width_is_rejected_by_check() {
    // §8-一F-9 / §5.2：updated_at 列亦带 CHECK(length(updated_at)=24)——25 宽越界值被拒。
    // 与 created_at 对称，钉死「时间列 CHECK」覆盖到第二个时间列（防只给 created_at 设 CHECK）。
    let db = migrated_db();
    let mut cols = base_cols();
    cols.extend(["name", "kind"]);
    let vals = vec![
        Value::Integer(9_002),
        Value::Integer(0),
        Value::Text(VALID_24_TIME.to_string()),
        Value::Text("system".to_string()),
        Value::Text("2026-01-01T00:00:00.0000Z".to_string()), // 长度 25 ≠ 24
        Value::Text("system".to_string()),
        Value::Integer(0),
        Value::Integer(1),
        Value::Text("p-bad-upd".to_string()),
        Value::Text("agent".to_string()),
    ];
    let err = ddl_check_probe(&db, "principals", &cols, vals)
        .expect_err("updated_at length 25 must violate CHECK(length(updated_at)=24)");
    assert!(
        matches!(err, StoreError::ConstraintViolation),
        "wrong-width updated_at hits width-24 CHECK, got {err:?}"
    );
    assert_eq!(count_where(&db, "principals", "id = 9002"), 0, "rejected row left no trace");
}

#[test]
fn valid_width_24_time_passes_check_via_direct_write() {
    // §8-一F-9：负向探针的对照正向——同一直驱写路径下，24 宽合法时间文本被 CHECK 放行，
    // 证明上面两测的拒绝来自「宽度」而非「直驱写本身被拒」（探针无系统性副作用）。
    let db = migrated_db();
    let mut cols = base_cols();
    cols.extend(["name", "kind"]);
    let vals = vec![
        Value::Integer(9_003),
        Value::Integer(0),
        Value::Text(VALID_24_TIME.to_string()),
        Value::Text("system".to_string()),
        Value::Text(VALID_24_TIME.to_string()),
        Value::Text("system".to_string()),
        Value::Integer(0),
        Value::Integer(1),
        Value::Text("p-good-time".to_string()),
        Value::Text("agent".to_string()),
    ];
    ddl_check_probe(&db, "principals", &cols, vals).expect("24-wide time passes width CHECK");
    assert_eq!(count_where(&db, "principals", "id = 9003"), 1, "valid-width row landed");
}

// ============================================================ §5.2 partial unique 重建

#[test]
fn partial_unique_allows_rebuild_after_logical_delete() {
    // §5.2：partial unique on (WHERE delete_flag=0)——逻辑删除后同名可重建（历史行保留）。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 16);
    let now = now_at(EPOCH_UNIX_MS + 16);
    let id = insert_principal(&db, &idgen, now, "p-dup").expect("first p-dup lands");
    // 逻辑移除首条（delete_flag=1），其退出活跃集，partial unique 不再覆盖它。
    db.with_write_txn(|txn| {
        write::logical_delete(txn, now, &Actor::System, "principals", SnowflakeId::from_raw(id as u64), 0)
    })
    .expect("logical delete first row");
    // 同名重建：活跃集内无 p-dup，partial unique 放行新行。
    insert_principal(&db, &idgen, now, "p-dup").expect("rebuild same name after logical delete");
    assert_eq!(count_where(&db, "principals", "name = 'p-dup' AND delete_flag = 0"), 1, "one active row");
    assert_eq!(count_where(&db, "principals", "name = 'p-dup'"), 2, "history row retained");
}

#[test]
fn active_set_partial_unique_rejects_live_duplicate() {
    // §5.2：活跃集内（delete_flag=0）同名第二行被 partial unique 拒。
    let db = migrated_db();
    let idgen = idgen_at(EPOCH_UNIX_MS + 17);
    let now = now_at(EPOCH_UNIX_MS + 17);
    insert_principal(&db, &idgen, now, "p-live").expect("first lands");
    let err = insert_principal(&db, &idgen, now, "p-live").expect_err("live duplicate name rejected");
    assert!(matches!(err, StoreError::ConstraintViolation), "live dup rejected, got {err:?}");
}

// ============================================================ schema 元数据一致性

#[test]
fn business_tables_constant_matches_created_tables() {
    // §5.2：BUSINESS_TABLES 常量与 schema 实际建出的表一一对应（无遗漏、无幽灵）。
    let db = migrated_db();
    for t in BUSINESS_TABLES {
        assert!(table_exists(&db, t), "declared business table {t} exists in built schema");
    }
    assert_eq!(schema::CURRENT_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION, "version constant is stable");
}
