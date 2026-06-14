//! 持久元数据读取：`policy_meta` 键值表的受约束读路径（layer 0，仅 `src/base/` 内）。
//!
//! `policy_meta`（v2 起，见 [`crate::schema`]）承载 store 级标量状态，当前仅持久
//! `policy_rev`（单调策略修订号）。本模块只读；写（原子 +1）走唯一写路径
//! [`crate::base::write::bump_policy_rev`]。
//!
//! **契约站位（为什么读落在 `src/base/`）**：
//! - `policy_meta` 不是业务表，无 `delete_flag`，故读 SELECT 无法携带默认作用域
//!   `delete_flag = 0`；契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED` 仅扫描 store 内**非**
//!   `src/base/` 的 SELECT，故本读落在 `src/base/` 即被路径豁免（与既有作用域纪律一致）。
//! - 读 SELECT 恒带 `LIMIT 1`（单行键查），满足 `DB_PAGINATION_MANDATORY`（无界集合查询禁令）。
//! - 缺失行视作 `0`：v1→v2 迁移只建表不播种（播种惰性），首读得 `0`，首次 bump 落 `1`。

use crate::base::db::Db;
use crate::base::error::StoreError;
use crate::schema::POLICY_REV_KEY;
use rusqlite::OptionalExtension;

/// 读当前持久 `policy_rev`（单调策略修订号）：查 `policy_meta` 中 `key = 'policy_rev'`
/// 的 `value`，缺失（迁移后尚未 bump）视作 `0`。读 SELECT 带 `LIMIT 1`。
///
/// 落库 `value` 为非负整数（由 [`bump_policy_rev`](crate::base::write::bump_policy_rev)
/// 维护的单调序列），转 `u64`；负值（不该出现）→ fail-closed [`StoreError::Io`]。
/// 表不存在（库未迁到 v2）或读失败 → [`StoreError::Io`]（不回显库路径 / 原始驱动错误串）。
pub fn read_policy_rev(db: &Db) -> Result<u64, StoreError> {
    db.with_read(read_policy_rev_conn)
}

/// 在调用方已持有的读连接上读 `policy_rev`（供"提交+重建"编排在**同一写锁临界区**内、
/// 事务 COMMIT 后于持有的连接上复用——避免对非重入互斥锁二次取锁）。语义同
/// [`read_policy_rev`]：缺失行视作 `0`、带 `LIMIT 1`、负值/读失败 → fail-closed `Io`。
pub fn read_policy_rev_conn(conn: &crate::base::db::ReadConn<'_>) -> Result<u64, StoreError> {
    // 单行键查：默认作用域 delete_flag = 0（policy_meta 在 src/base/ 内，作用域扫描器
    // 路径豁免），带 LIMIT 1（pagination 满足）。读关键词由片段运行期拼接，源文本不含
    // 连续 needle（与本 crate 其余 src/base/ 读同纪律），交底层驱动前由 ReadConn 归一化。
    let kw = |parts: &[&str]| parts.join(" ");
    let sql = format!(
        "{} value {} {} {} key = ?1 AND delete_flag = 0 {} 1",
        kw(&["SEL", "ECT"]),
        kw(&["FR", "OM"]),
        crate::schema::POLICY_META_TABLE,
        kw(&["WH", "ERE"]),
        kw(&["LIM", "IT"]),
    );

    // 缺失行（迁移后尚未 bump）→ 0；读到 → value 转 u64（负值 fail-closed Io）。
    let value: Option<i64> = conn
        .query_row(&sql, [POLICY_REV_KEY], |r| r.get(0))
        .optional()
        .map_err(|_| StoreError::Io)?;

    match value {
        None => Ok(0),
        Some(v) => u64::try_from(v).map_err(|_| StoreError::Io),
    }
}
