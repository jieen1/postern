//! 唯一写路径：审计字段自动填充、乐观锁、逻辑删除、级联（唯一允许的写/改写位置）。
//!
//! 本文件是全工作区**唯一**允许出现数据库写/改写语句的位置（契约
//! `DB_WRITE_PATH_CENTRALIZED`）。能力（§3.1）：
//!
//! - **INSERT**：自动填充 `id`（core 雪花 [`IdGen`]）/ `version=0` /
//!   `created_at = updated_at = now` / `created_by = updated_by`（控制面=操作者
//!   标识、系统写=`system`）/ `delete_flag=0` / `enable_flag`。写 API 签名**绝不**
//!   暴露 `version / created_* / updated_*` 五个审计字段参数（§7-2）。
//! - **UPDATE**：恒 `SET version = version + 1 ... WHERE id = ? AND version = ?`
//!   （乐观锁不自读自比；期望 `version` 由调用方传入），影响 0 行 →
//!   [`StoreError::VersionConflict`]，绝不静默重试（§7-3）。
//! - **逻辑删除**：恒 `SET delete_flag = 1`（连带 `version` 自增与 `updated_*`
//!   维护）；无物理删除、无 undelete 入口（§7-4）。
//! - **级联逻辑删除**：同事务把直接子行 `delete_flag` 置 1、`updated_by` 标
//!   `cascade:<table>#<id>`（§3.2 级联图）。
//! - **系统协调写**：不带期望 `version` 的谓词幂等更新，供 sweeper（不参与乐观锁）。
//! - **限制性表**（`grant_constraints` / `grant_conditions` / `mode_state` /
//!   `deny_notes`）拒绝写入非 1 的 `enable_flag`（fail-closed，§7-10）。

use crate::base::error::StoreError;
use crate::base::normalize::normalize_name;
use crate::base::timestamp;
use postern_core::domain::Timestamp;
use postern_core::id::{IdGen, SnowflakeId};
use rusqlite::types::Value;
use rusqlite::Transaction;

/// 写入操作者：`created_by` / `updated_by` 的取值来源（审计字段自动化）。
/// 调用方只交"是谁在写"，绝不交五个审计字段本身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// 控制面写入：已认证操作者标识（落 `created_by` / `updated_by`）。
    Operator(String),
    /// 系统自动写入（sweeper 回收 / import 协调）：落 `created_by = updated_by = 'system'`。
    System,
}

/// 系统写入者的固定标识字面：`created_by` / `updated_by` 取此值。
pub const SYSTEM_ACTOR: &str = "system";

impl Actor {
    /// 该操作者落库的 `created_by` / `updated_by` 文本：[`Actor::System`] → `"system"`。
    pub fn label(&self) -> &str {
        match self {
            Actor::Operator(id) => id,
            Actor::System => SYSTEM_ACTOR,
        }
    }
}

/// 一行 INSERT 的业务字段（不含 8 基础字段中的任何审计字段——那五个由 `base`
/// 自动填充）。`columns` 与 `values` 一一对应，仅含业务列与 `enable_flag`。
pub struct InsertRow {
    /// 目标表名。
    pub table: &'static str,
    /// 业务列名（绝不含 `id/version/created_at/created_by/updated_at/updated_by/delete_flag`）。
    pub columns: Vec<&'static str>,
    /// 与 `columns` 等长的业务列值。
    pub values: Vec<Value>,
    /// `enable_flag` 入库值；限制性表只接受 `1`，其余值 fail-closed 拒写。
    pub enable_flag: i64,
}

/// 限制性表清单（禁非 1 的 `enable_flag`，§3.1/§7-10）。
pub const RESTRICTED_TABLES: [&str; 4] = [
    "grant_constraints",
    "grant_conditions",
    "mode_state",
    "deny_notes",
];

/// 表是否为限制性表（写 `enable_flag≠1` 即被拒）。
pub fn is_restricted_table(table: &str) -> bool {
    RESTRICTED_TABLES.contains(&table)
}

/// 入库归一化所及的业务列：`name` / `codename`（principals/roles/resources 的
/// 唯一标识列）。其值经 [`normalize_name`] 后落库，使归一化唯一索引生效。
fn normalize_for(column: &str, value: Value) -> Value {
    if matches!(column, "name" | "codename") {
        if let Value::Text(s) = value {
            return Value::Text(normalize_name(&s));
        }
    }
    value
}

/// 把 rusqlite 写执行的影响行数包成 fail-closed 结果，驱动原始错误就地归类：
/// SQLite 约束违反（partial unique / CHECK / 限制性表等）→ [`StoreError::ConstraintViolation`]，
/// 其余（IO / 打开失败 / 杂项）→ [`StoreError::Io`]。绝不把原始驱动错误串透出 crate 边界。
fn execute(
    txn: &Transaction<'_>,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<usize, StoreError> {
    txn.execute(sql, params).map_err(classify)
}

/// 把 rusqlite 原始错误就地映射为本域语义变体（不外泄底层错误串、库路径、SQL 片段）。
fn classify(err: rusqlite::Error) -> StoreError {
    match err {
        rusqlite::Error::SqliteFailure(e, _)
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            StoreError::ConstraintViolation
        }
        _ => StoreError::Io,
    }
}

/// INSERT：在事务内插一行，自动填充 8 基础字段中的审计/管控列
/// （`id = IdGen` / `version = 0` / `created_at = updated_at = now` /
/// `created_by = updated_by = actor.label()` / `delete_flag = 0` /
/// `enable_flag`）。
///
/// 限制性表写 `enable_flag != 1` → [`StoreError::ConstraintViolation`]，库不变。
/// partial unique 等约束违反同样映射为 `ConstraintViolation`。返回新行 `id`。
pub fn insert(
    txn: &Transaction<'_>,
    idgen: &IdGen,
    now: Timestamp,
    actor: &Actor,
    row: InsertRow,
) -> Result<SnowflakeId, StoreError> {
    // 限制性表 fail-closed：enable_flag 非 1 直接拒，不触库。
    if is_restricted_table(row.table) && row.enable_flag != 1 {
        return Err(StoreError::ConstraintViolation);
    }

    let id = idgen.next_id().map_err(|_| StoreError::IdGen)?;
    let ts = timestamp::format(now);
    let by = actor.label().to_string();

    // 列序：8 基础列在前（id/version/created_at/created_by/updated_at/updated_by/
    // delete_flag/enable_flag），业务列在后。
    let mut columns: Vec<&str> = vec![
        "id",
        "version",
        "created_at",
        "created_by",
        "updated_at",
        "updated_by",
        "delete_flag",
        "enable_flag",
    ];
    columns.extend(row.columns.iter().copied());

    let mut params: Vec<Value> = vec![
        Value::Integer(id.as_raw() as i64),
        Value::Integer(0),
        Value::Text(ts.clone()),
        Value::Text(by.clone()),
        Value::Text(ts),
        Value::Text(by),
        Value::Integer(0),
        Value::Integer(row.enable_flag),
    ];
    for (col, val) in row.columns.iter().zip(row.values) {
        params.push(normalize_for(col, val));
    }

    let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        row.table,
        columns.join(", "),
        placeholders.join(", ")
    );

    let bind: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    execute(txn, &sql, &bind)?;
    Ok(id)
}

/// 乐观锁 UPDATE：恒 `SET <business...>, version = version + 1,
/// updated_at = now, updated_by = actor ... WHERE id = ? AND version = ?`。
///
/// 期望 `expected_version` 由调用方传入（**不**自读自比）。影响 0 行 →
/// [`StoreError::VersionConflict`]（绝不与"行不存在 / IO 失败"混淆、绝不静默
/// 重试）。限制性表写 `enable_flag != 1` → [`StoreError::ConstraintViolation`]。
///
/// 参数数量由唯一写路径的乐观锁 UPDATE 契约（业务列/值、期望 version、可选
/// enable_flag 各为独立入参）决定，是设计承诺的签名，故就地豁免参数计数 lint。
#[allow(clippy::too_many_arguments)]
pub fn update(
    txn: &Transaction<'_>,
    now: Timestamp,
    actor: &Actor,
    table: &'static str,
    id: SnowflakeId,
    expected_version: i64,
    columns: Vec<&'static str>,
    values: Vec<Value>,
    enable_flag: Option<i64>,
) -> Result<(), StoreError> {
    // 限制性表 fail-closed：试图把 enable_flag 置非 1 直接拒，不触库。
    if is_restricted_table(table) {
        if let Some(flag) = enable_flag {
            if flag != 1 {
                return Err(StoreError::ConstraintViolation);
            }
        }
    }

    let ts = timestamp::format(now);
    let by = actor.label().to_string();

    // SET 子句：业务列 + 可选 enable_flag + 审计列（version 自增、updated_*）。
    let mut set_clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut idx: usize = 1;

    for (col, val) in columns.iter().zip(values) {
        set_clauses.push(format!("{col} = ?{idx}"));
        params.push(normalize_for(col, val));
        idx += 1;
    }
    if let Some(flag) = enable_flag {
        set_clauses.push(format!("enable_flag = ?{idx}"));
        params.push(Value::Integer(flag));
        idx += 1;
    }
    set_clauses.push("version = version + 1".to_string());
    set_clauses.push(format!("updated_at = ?{idx}"));
    params.push(Value::Text(ts));
    idx += 1;
    set_clauses.push(format!("updated_by = ?{idx}"));
    params.push(Value::Text(by));
    idx += 1;

    // WHERE id = ? AND version = ?（乐观锁）。
    let id_idx = idx;
    params.push(Value::Integer(id.as_raw() as i64));
    idx += 1;
    let ver_idx = idx;
    params.push(Value::Integer(expected_version));

    let sql = format!(
        "UPDATE {} SET {} WHERE id = ?{} AND version = ?{}",
        table,
        set_clauses.join(", "),
        id_idx,
        ver_idx
    );

    let bind: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let affected = execute(txn, &sql, &bind)?;
    if affected == 0 {
        return Err(StoreError::VersionConflict);
    }
    Ok(())
}

/// 逻辑删除：恒 `SET delete_flag = 1, version = version + 1, updated_at = now,
/// updated_by = actor WHERE id = ? AND version = ?`。
///
/// 终态、无 undelete；影响 0 行（期望 version 不符）→
/// [`StoreError::VersionConflict`]。本函数**不**自动级联——级联走
/// [`cascade_logical_delete`]，由调用方在同一事务内显式编排父子顺序。
pub fn logical_delete(
    txn: &Transaction<'_>,
    now: Timestamp,
    actor: &Actor,
    table: &'static str,
    id: SnowflakeId,
    expected_version: i64,
) -> Result<(), StoreError> {
    let ts = timestamp::format(now);
    let by = actor.label().to_string();
    let sql = format!(
        "UPDATE {table} SET delete_flag = 1, version = version + 1, \
         updated_at = ?1, updated_by = ?2 WHERE id = ?3 AND version = ?4"
    );
    let affected = execute(
        txn,
        &sql,
        rusqlite::params![ts, by, id.as_raw() as i64, expected_version],
    )?;
    if affected == 0 {
        return Err(StoreError::VersionConflict);
    }
    Ok(())
}

/// 级联逻辑删除：在**同一事务**内把 `child_table` 中 `<fk_column> = parent_id`
/// 且尚未删（`delete_flag = 0`）的直接子行置 `delete_flag = 1`、`version` 自增、
/// `updated_at = now`、`updated_by = 'cascade:<parent_table>#<parent_id>'`。
///
/// 系统协调形态（谓词幂等、不参与乐观锁），故不带期望 version。返回受影响子行数。
/// 父事务任一步失败时，整体 ROLLBACK（父子行均不变）由 [`Db::with_write_txn`]
/// 保证。
pub fn cascade_logical_delete(
    txn: &Transaction<'_>,
    now: Timestamp,
    parent_table: &str,
    parent_id: SnowflakeId,
    child_table: &'static str,
    fk_column: &'static str,
) -> Result<usize, StoreError> {
    let ts = timestamp::format(now);
    let origin = format!("cascade:{parent_table}#{}", parent_id.as_raw());
    let sql = format!(
        "UPDATE {child_table} SET delete_flag = 1, version = version + 1, \
         updated_at = ?1, updated_by = ?2 \
         WHERE {fk_column} = ?3 AND delete_flag = 0"
    );
    let affected = execute(
        txn,
        &sql,
        rusqlite::params![ts, origin, parent_id.as_raw() as i64],
    )?;
    Ok(affected)
}

/// 系统协调写（sweeper）：谓词幂等更新，**不**参与乐观锁（无"读后写"竞态）。
/// 恒 `SET <business...>, version = version + 1, updated_at = now,
/// updated_by = 'system' WHERE <predicate>`；不带 `WHERE version = ?`。
/// 返回受影响行数。
pub fn system_update(
    txn: &Transaction<'_>,
    now: Timestamp,
    table: &'static str,
    set_columns: Vec<&'static str>,
    set_values: Vec<Value>,
    where_predicate: &str,
    where_values: Vec<Value>,
) -> Result<usize, StoreError> {
    let ts = timestamp::format(now);

    let mut set_clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut idx: usize = 1;

    for (col, val) in set_columns.iter().zip(set_values) {
        set_clauses.push(format!("{col} = ?{idx}"));
        params.push(normalize_for(col, val));
        idx += 1;
    }
    set_clauses.push("version = version + 1".to_string());
    set_clauses.push(format!("updated_at = ?{idx}"));
    params.push(Value::Text(ts));
    idx += 1;
    set_clauses.push(format!("updated_by = ?{idx}"));
    params.push(Value::Text(SYSTEM_ACTOR.to_string()));
    idx += 1;

    // 调用方谓词中的占位符从当前 idx 续编（其值经 where_values 按序绑定）。
    let predicate = renumber_predicate(where_predicate, &mut idx);
    params.extend(where_values);

    let sql = format!(
        "UPDATE {} SET {} WHERE {}",
        table,
        set_clauses.join(", "),
        predicate
    );

    let bind: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    execute(txn, &sql, &bind)
}

/// 原子递增持久 `policy_rev`（单调策略修订号）：在调用方给定的写事务内，把
/// `policy_meta` 中 `key = 'policy_rev'` 的 `value` 自增 1（缺失行——迁移后首次——视作
/// 当前 0、落新值 1），返回**新** rev。单调、跨重启存活（持久落库）、绝不回退。
///
/// 写经唯一写路径（本文件，契约 `DB_WRITE_PATH_CENTRALIZED`），以 UPSERT 落值（缺失则插、
/// 存在则改）；`policy_meta` 非业务表、无审计字段/逻辑删除语义，故不经 8 基础字段填充。
/// 同事务内调用：rev 自增与触发它的实体写共用同一事务边界，要么同 COMMIT、要么同
/// ROLLBACK（全或无，杜绝"写已落而 rev 未进"或反之）。写失败 → [`StoreError::Io`]。
pub fn bump_policy_rev(txn: &Transaction<'_>) -> Result<u64, StoreError> {
    // policy_meta 行的审计列（NOT NULL + CHECK length 24）由本写路径填充：created_by /
    // updated_by 取系统标识、created_at / updated_at 取当前墙钟（与 audit sink 同源的
    // 内部墙钟，policy_meta 非业务表故不经调用方注入的 now）。id 由 SQLite 自动分配
    // （INTEGER PRIMARY KEY rowid），version / delete_flag / enable_flag 走列默认。
    let ts = timestamp::format(Timestamp::from_unix_ms(now_unix_ms()));

    // UPSERT：首次（缺行）插 value = 1；已存在则 value + 1、version 自增、updated_*
    // 维护。冲突目标对齐 policy_meta 的 partial unique 索引（key WHERE delete_flag = 0）。
    // RETURNING value 取本次落库的新 rev（插入分支返 1，更新分支返自增后值）。
    let sql = format!(
        "INSERT INTO {table} (created_at, created_by, updated_at, updated_by, key, value) \
         VALUES (?1, ?2, ?1, ?2, ?3, 1) \
         ON CONFLICT(key) WHERE delete_flag = 0 \
         DO UPDATE SET value = value + 1, version = version + 1, \
         updated_at = ?1, updated_by = ?2 \
         RETURNING value",
        table = crate::schema::POLICY_META_TABLE,
    );

    let new_rev: i64 = txn
        .query_row(
            &sql,
            rusqlite::params![ts, SYSTEM_ACTOR, crate::schema::POLICY_REV_KEY],
            |r| r.get(0),
        )
        .map_err(classify)?;

    // value 列为单调非负序列；负值（不该出现）fail-closed 而非回绕为巨大 u64。
    u64::try_from(new_rev).map_err(|_| StoreError::Io)
}

/// 内部墙钟（Unix 毫秒），仅供 `policy_meta` 审计列填充用（非业务时间、不经调用方注入
/// 的 `now`）。早于 Unix 纪元（不可能的本地时钟）退化为 0。与 audit sink 内部墙钟同源。
fn now_unix_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

/// 把谓词里每个 `?`（无编号位置参数）顺序重编号为 `?N`，N 从 `idx` 续起，
/// 使 SET 子句与谓词共享同一套绑定下标。已带编号的 `?N` 不在此处处理（调用方
/// 用裸 `?` 或不带占位符的常量谓词，本 crate 测试即后者 `id = <literal>`）。
fn renumber_predicate(predicate: &str, idx: &mut usize) -> String {
    let mut out = String::with_capacity(predicate.len());
    for ch in predicate.chars() {
        if ch == '?' {
            out.push_str(&format!("?{}", *idx));
            *idx += 1;
        } else {
            out.push(ch);
        }
    }
    out
}
