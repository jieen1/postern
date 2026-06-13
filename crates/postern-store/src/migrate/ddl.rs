//! 有序前向迁移步：从某库版本逐级追平至当前最高已知版本的 DDL 序列。
//!
//! 每一步是"从版本 `n` 到 `n+1`"的有序 DDL（建表 / 加列 / 加索引等，绝不破坏性
//! 改写已有数据）。[`forward_steps`] 返回从 `from_version`（不含）到
//! [`CURRENT_SCHEMA_VERSION`](crate::schema::CURRENT_SCHEMA_VERSION)（含）的全部步,
//! 由 [`migrate`](crate::migrate::migrate) 在单一事务内按序施加。
//!
//! 当前最高版本为 1：v0→v1 即"空库建全套表"（[`schema::SCHEMA_SQL`](crate::schema::SCHEMA_SQL)），
//! 尚无 v1 之后的演进步。新增 schema 演进时在此追加有序步并抬升最高版本常量。

use crate::base::error::StoreError;
use crate::schema::{SCHEMA_SQL, CURRENT_SCHEMA_VERSION};

/// 一条前向迁移步：把库从 `from`（其前置版本）演进到 `to`（其后置版本）的 DDL 文本。
pub struct MigrationStep {
    /// 该步的前置 schema 版本（施加前库应处的版本）。
    pub from: i64,
    /// 该步的后置 schema 版本（施加后库达到的版本）。
    pub to: i64,
    /// 该步的有序 DDL 文本（在迁移事务内 `execute_batch` 施加）。
    pub ddl: &'static str,
}

/// 全部有序迁移步（按 `to` 升序，逐步 `from == 前一步 to`，无缺口）。
/// 当前仅 v0→v1：空库建全套表（schema.sql 全文）。新增演进时在此追加。
const STEPS: &[MigrationStep] = &[MigrationStep {
    from: 0,
    to: 1,
    ddl: SCHEMA_SQL,
}];

/// 返回从 `from_version`（不含）追平至当前最高已知版本（含）所需的有序迁移步。
///
/// `from_version` 为 0 表示空库（首步即建全套表）。返回步按 `to` 升序，逐步
/// `from == 前一步 to`，无缺口。`from_version >= 当前最高版本` 时返回空切片
/// （幂等 / 不识别由调用方在更高层判定）。
pub fn forward_steps(from_version: i64) -> Result<Vec<MigrationStep>, StoreError> {
    // 已处或超过最高已知版本：无步可施（幂等 / 不识别在更高层判定）。
    if from_version >= CURRENT_SCHEMA_VERSION {
        return Ok(Vec::new());
    }
    let steps: Vec<MigrationStep> = STEPS
        .iter()
        .filter(|s| s.from >= from_version)
        .map(|s| MigrationStep {
            from: s.from,
            to: s.to,
            ddl: s.ddl,
        })
        .collect();
    Ok(steps)
}
