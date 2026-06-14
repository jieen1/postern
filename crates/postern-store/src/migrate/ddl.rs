//! 有序前向迁移步：从某库版本逐级追平至当前最高已知版本的 DDL 序列。
//!
//! 每一步是"从版本 `n` 到 `n+1`"的有序 DDL（建表 / 加列 / 加索引等，绝不破坏性
//! 改写已有数据）。[`forward_steps`] 返回从 `from_version`（不含）到
//! [`CURRENT_SCHEMA_VERSION`](crate::schema::CURRENT_SCHEMA_VERSION)（含）的全部步,
//! 由 [`migrate`](crate::migrate::migrate) 在单一事务内按序施加。
//!
//! 当前最高版本为 3：v0→v1 即"空库建全套表"（[`schema::SCHEMA_SQL`](crate::schema::SCHEMA_SQL)）；
//! v1→v2 建持久 `policy_meta` 键值表（承载单调 `policy_rev`）；v2→v3 建 `settings`
//! 键值表（业务级标量配置，限制性表）。新增 schema 演进时在此追加有序步并抬升最高版本常量。
//!
//! v1→v2 只建表、**不**在本（非 `src/base/`）文件里写 INSERT/UPDATE：`policy_rev`
//! 的播种是惰性的——[`read_policy_rev`](crate::base::meta::read_policy_rev) 把缺失行视作
//! `0`，首次 [`bump_policy_rev`](crate::base::write::bump_policy_rev)（在 `src/base/`，唯一写
//! 路径）以 UPSERT 落 `policy_rev = 1`。故迁移步是纯 DDL，写路径契约不被绕过。

use crate::base::error::StoreError;
use crate::schema::{CURRENT_SCHEMA_VERSION, SCHEMA_SQL};

/// v1→v2 前向步 DDL：建持久 `policy_meta` 键值表（承载单调 `policy_rev`）。
///
/// 与全部业务表同纪律——声明全 8 基础字段在前（`DB_BASE_FIELDS_REQUIRED`：每个建表块
/// 必含 8 基础列；契约扫描器扫 store 内一切建表块，含本 Rust 字符串，故本表照样齐备
/// 8 列），业务列 `key`/`value` 在后；`key` 唯一（partial unique，`WHERE delete_flag = 0`，
/// 与既有唯一性同形）。纯建表、无数据播种——`policy_rev` 行惰性创建：首次
/// [`bump_policy_rev`](crate::base::write::bump_policy_rev)（唯一写路径）若无行则插、有则
/// 自增，故迁移步不含 INSERT/UPDATE，写路径契约不被绕过。
const MIGRATE_V1_TO_V2: &str = "\
CREATE TABLE policy_meta (\n\
  id          INTEGER PRIMARY KEY,\n\
  version     INTEGER NOT NULL DEFAULT 0,\n\
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),\n\
  created_by  TEXT    NOT NULL,\n\
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),\n\
  updated_by  TEXT    NOT NULL,\n\
  delete_flag INTEGER NOT NULL DEFAULT 0,\n\
  enable_flag INTEGER NOT NULL DEFAULT 1,\n\
  key         TEXT    NOT NULL,\n\
  value       INTEGER NOT NULL\n\
);\n\
CREATE UNIQUE INDEX uq_policy_meta_key ON policy_meta(key) WHERE delete_flag = 0;";

/// v2→v3 前向步 DDL：建 `settings` 业务级标量配置键值表（限制性表）。
///
/// 与全部业务表同纪律——声明全 8 基础字段在前（`DB_BASE_FIELDS_REQUIRED`：每个建表块
/// 必含 8 基础列；契约扫描器扫 store 内一切建表块，含本 Rust 字符串，故本表照样齐备
/// 8 列），业务列 `key`/`value` 在后；`key` 唯一（partial unique，`WHERE delete_flag = 0`，
/// 与既有唯一性同形）。限制性表语义（`CHECK (enable_flag = 1)`，禁停用），写经唯一写路径
/// （[`base::write::insert`](crate::base::write::insert)/[`update`](crate::base::write::update)，
/// 限制性表 `enable_flag != 1` fail-closed）。元数据（默认值/是否可写/类型）不入库——由
/// daemon 按已知 key 定义。纯建表、无数据播种：每个 setting 行由控制面写按需创建。
const MIGRATE_V2_TO_V3: &str = "\
CREATE TABLE settings (\n\
  id          INTEGER PRIMARY KEY,\n\
  version     INTEGER NOT NULL DEFAULT 0,\n\
  created_at  TEXT    NOT NULL CHECK (length(created_at) = 24),\n\
  created_by  TEXT    NOT NULL,\n\
  updated_at  TEXT    NOT NULL CHECK (length(updated_at) = 24),\n\
  updated_by  TEXT    NOT NULL,\n\
  delete_flag INTEGER NOT NULL DEFAULT 0,\n\
  enable_flag INTEGER NOT NULL DEFAULT 1 CHECK (enable_flag = 1),\n\
  key         TEXT    NOT NULL,\n\
  value       TEXT    NOT NULL\n\
);\n\
CREATE UNIQUE INDEX uq_settings_key ON settings(key) WHERE delete_flag = 0;";

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
/// v0→v1：空库建全套业务表（schema.sql 全文）；v1→v2：建持久 `policy_meta` 表；
/// v2→v3：建 `settings` 键值表。新增演进时在此追加有序步并抬升 [`CURRENT_SCHEMA_VERSION`]。
const STEPS: &[MigrationStep] = &[
    MigrationStep {
        from: 0,
        to: 1,
        ddl: SCHEMA_SQL,
    },
    MigrationStep {
        from: 1,
        to: 2,
        ddl: MIGRATE_V1_TO_V2,
    },
    MigrationStep {
        from: 2,
        to: 3,
        ddl: MIGRATE_V2_TO_V3,
    },
];

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
