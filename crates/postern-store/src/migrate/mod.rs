//! schema 版本检查与前向迁移：单事务前向、未知高版本 fail-closed。
//!
//! 以 `PRAGMA user_version` 标识 schema 版本，由 daemon 启动序列调用 [`migrate`]。
//! **怎么做**（§3.2）——开库后先读 `user_version`，与当前实现内置的
//! [最高已知版本](crate::schema::CURRENT_SCHEMA_VERSION) 比对，分三态处置：
//!
//! - `== 0`：空库 → [`init_schema`] 单事务建全套表 + 前进 `user_version` 至当前；
//! - `0 < v < cur`（旧库）：从库版本起逐级施加有序前向迁移步直至追平，**全程包在
//!   单一事务内**（任一步失败即整体 ROLLBACK、`user_version` 不前进，杜绝半截
//!   schema），追平后才前进 `user_version`；
//! - `== cur`：幂等无操作；
//! - `> cur`（库由更新实现写过）：**fail-closed** 返 [`StoreError::UnknownSchemaVersion`]，
//!   绝不按旧假设解析未知 schema，**库不变**（不触任何 DDL）。
//!
//! **为什么只前向、不回退**：降级解析未知结构与公理二相悖，故未知高版本只拒不猜；
//! 迁移单事务化是"要么整套新 schema、要么原样旧库"的原子性前提。

pub mod ddl;

use crate::base::db::Db;
use crate::base::error::StoreError;
use crate::schema::CURRENT_SCHEMA_VERSION;

/// 开库后的统一 PRAGMA：外键强制（`foreign_keys=ON`）+ WAL 日志模式。
///
/// 任一施加失败 → [`StoreError::Io`]（不回显库路径 / 原始驱动错误串）。内存库不
/// 支持 WAL（其值落回 memory），设置失败不致命：仅在持久库生效。
pub fn apply_pragmas(conn: &rusqlite::Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| StoreError::Io)?;
    // 内存库不支持 WAL（其值会落回 memory），设置失败不致命：仅在持久库生效。
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    Ok(())
}

/// 读取库的 `PRAGMA user_version`（schema 版本标识）。读失败 → [`StoreError::Io`]。
pub fn schema_version(db: &Db) -> Result<i64, StoreError> {
    db.with_read(|conn| {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| StoreError::Io)
    })
}

/// 设置库的 `PRAGMA user_version`（迁移追平后前进版本号；测试构造库版本亦经此）。
///
/// 写失败 → [`StoreError::Io`]。`user_version` 是 SQLite 头部字段，前进只在 DDL
/// 整体成功后于同一事务尾发生（见 [`init_schema`] / [`migrate`]）。
pub fn set_schema_version(db: &Db, version: i64) -> Result<(), StoreError> {
    db.with_write_txn(|txn| {
        set_version_in_txn(txn, version)?;
        Ok(())
    })
}

/// 在事务内置 `user_version`（`PRAGMA user_version = N` 不接受绑定参数，故格式化常量）。
fn set_version_in_txn(txn: &rusqlite::Transaction<'_>, version: i64) -> Result<(), StoreError> {
    txn.pragma_update(None, "user_version", version)
        .map_err(|_| StoreError::Io)
}

/// 空库建库：在**单一事务**内按 [`schema::SCHEMA_SQL`](crate::schema::SCHEMA_SQL)
/// 建全套业务表（含 8 基础列、各表 CHECK、partial unique 索引），尾部把
/// `user_version` 前进至 [`CURRENT_SCHEMA_VERSION`](crate::schema::CURRENT_SCHEMA_VERSION)。
///
/// 任一步失败 → 整体 ROLLBACK、`user_version` 不前进、库不留半截 schema
/// （[`StoreError::Io`] / [`StoreError::ConstraintViolation`]）。仅当库当前
/// `user_version == 0`（空库）时调用；非空库由 [`migrate`] 分流。
pub fn init_schema(db: &Db) -> Result<(), StoreError> {
    apply_forward(db, 0)
}

/// 迁移入口：读 `user_version`，与当前实现最高已知版本比对，分三态前向 fail-closed。
///
/// - `0` → [`init_schema`]；
/// - `0 < v < cur` → 单事务前向迁移追平（任一步失败整体 ROLLBACK、版本不进）；
/// - `== cur` → 幂等无操作；
/// - `> cur` → [`StoreError::UnknownSchemaVersion`]，**库不变**（不触任何 DDL）。
pub fn migrate(db: &Db) -> Result<(), StoreError> {
    let current = schema_version(db)?;
    if current == CURRENT_SCHEMA_VERSION {
        // 已处当前版本：幂等无操作（不触任何 DDL、版本不动）。
        return Ok(());
    }
    if current > CURRENT_SCHEMA_VERSION {
        // 库由更新实现写过：fail-closed 拒解析未知 schema，库不变（不触任何 DDL）。
        return Err(StoreError::UnknownSchemaVersion);
    }
    // 0 <= current < cur：单事务前向迁移追平（含空库 init）。
    apply_forward(db, current)
}

/// 在**单一事务**内按序施加从 `from_version`（不含）追平至当前最高版本（含）的全部
/// 前向迁移步，尾部把 `user_version` 前进至当前最高版本。任一步失败 → 整体 ROLLBACK
/// （`user_version` 不前进、库不留半截 schema），由 [`Db::with_write_txn`] 保证。
fn apply_forward(db: &Db, from_version: i64) -> Result<(), StoreError> {
    let steps = ddl::forward_steps(from_version)?;
    db.with_write_txn(|txn| {
        for step in &steps {
            txn.execute_batch(step.ddl).map_err(|_| StoreError::Io)?;
        }
        set_version_in_txn(txn, CURRENT_SCHEMA_VERSION)?;
        Ok(())
    })
}
