//! 默认作用域构建器 + 分页执行器。
//!
//! - **默认作用域**：集合/单条查询自动追加 `delete_flag = 0`（业务查询默认看不到
//!   已删数据，§3.1 / 契约 `DB_DEFAULT_SCOPE_EXCLUDES_DELETED`）；`enable_flag`
//!   **不**在默认过滤内（启停是业务语义，由调用方按表语义显式使用）。
//! - **分页执行器**：接收 core [`PageQuery`]、`clamp` 后产出 `LIMIT ? OFFSET ?`
//!   并组装 core [`Page`] 信封；`total` 走纯 `COUNT(*)` 查询（投影区只有单个
//!   `COUNT`、无逗号，方满足 `DB_PAGINATION_MANDATORY` 豁免，§3.1 / §7-7）。

use crate::base::db::Db;
use crate::base::error::StoreError;
use postern_core::page::{Page, PageQuery};
use rusqlite::Row;

/// 默认作用域谓词：追加在集合/单条查询 `WHERE` 上的固定子句。
///
/// 恒为 `delete_flag = 0`——业务查询默认排除已删行；不含 `enable_flag` 过滤。
pub const DEFAULT_SCOPE_PREDICATE: &str = "delete_flag = 0";

/// 分页执行器：在 `Db` 上执行一条"列表 SQL + 纯 COUNT(*) SQL"对，按
/// `page.clamp()` 限界，组装 [`Page<T>`] 信封返回。
///
/// `page_size` 经 `clamp` 封顶到 [`PageQuery::MAX_SIZE`]（`201 → 200`），`page_no`
/// 下限为 1；`OFFSET` 由 `(page_no - 1) * page_size` 算得。`list_sql` 必带
/// `LIMIT ?1 OFFSET ?2`（`?1` 限额、`?2` 偏移），`count_sql` 必为纯 `COUNT(*)`
/// 投影且不带参数。`map_row` 把每行映射为 `T`，任一行映射失败 → fail-closed `Err`。
pub fn execute_page<T, M>(
    db: &Db,
    list_sql: &str,
    count_sql: &str,
    page: PageQuery,
    map_row: M,
) -> Result<Page<T>, StoreError>
where
    M: Fn(&Row<'_>) -> Result<T, StoreError>,
{
    let clamped = page.clamp();
    let limit = i64::from(clamped.page_size);
    // (page_no - 1) * page_size，page_no 已 clamp 到 ≥1；u64 中间量避免溢出。
    let offset = i64::try_from((u64::from(clamped.page_no) - 1) * u64::from(clamped.page_size))
        .map_err(|_| StoreError::Io)?;

    db.with_read(|conn| {
        // total：纯 COUNT(*) 单值查询，无参数。
        let total: i64 = conn
            .query_row(count_sql, [], |r| r.get(0))
            .map_err(|_| StoreError::Io)?;

        let mut stmt = conn.prepare(list_sql).map_err(|_| StoreError::Io)?;
        let mut rows = stmt
            .query(rusqlite::params![limit, offset])
            .map_err(|_| StoreError::Io)?;

        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|_| StoreError::Io)? {
            items.push(map_row(row)?);
        }

        Ok(Page {
            items,
            page_no: clamped.page_no,
            page_size: clamped.page_size,
            total: total.max(0) as u64,
        })
    })
}
