//! postgres 细则语义：`table_allow` / `column_mask`（§3.2）。
//!
//! 本 crate 是这两个 `kind` 语义的属主。`check` 是**纯函数**——只读 `spec` 与已物化的
//! `ci.objects`（§3.1 提取的对象集），不触底层资源、不发 IO。
//!
//! - `table_allow`（集合包含类）：归类提取的 schema.table 必须**全部**落在 `spec.tables`
//!   白名单内（全称量化——任一对象越界即 `Ok(false)`，绝非「有一个命中即放行」的
//!   存在量化 fail-open）。
//! - `column_mask`（触达禁止类）：`ci.objects` 列集与 `spec` 声明的禁止集交集**必须为空**，
//!   非空即 `Ok(false)`。它在求值期拒绝**触达**，区别于出口期擦除响应的 `mask_fields`。
//!
//! 判定所需信息缺失（对象未能可靠提取）即 `Err(ConstraintError::MissingObjects)`——
//! 「判不了」必须等价于「不通过」，绝不放行（L-7）。
//!
//! `kind` 非本属主（含出口期 `mask_fields`、跨协议 kind）→ `Err(UnknownKind)`；
//! `spec` 负载解析失败 / schema 不符 → `Err(InvalidSpec)`（绝不吞错放行）。

use serde::Deserialize;

use postern_core::domain::ConstraintSpec;
use postern_core::error::ConstraintError;
use postern_core::request::{ClassifiedIntent, ObjectRef};

/// 列维度 `ObjectRef` 的前缀约定（§3.1）：`col:schema.table.column`。
/// 不带此前缀的对象即表维度（裸 `schema.table`）。
const COLUMN_PREFIX: &str = "col:";

/// `table_allow` 负载：白名单表集（裸 `schema.table`）。缺 `tables` 键即 schema 不符。
#[derive(Deserialize)]
struct TableAllowSpec {
    tables: Vec<String>,
}

/// `column_mask` 负载：禁止列集（裸 `schema.table.column`）。缺 `columns` 键即 schema 不符。
#[derive(Deserialize)]
struct ColumnMaskSpec {
    columns: Vec<String>,
}

/// 步骤[4] 细则判定（§3.2）：对一个已物化 `ci` 按 `spec` 判通过 / 不通过。
///
/// `Ok(true)`=白名单内 / 未触达禁止集放行；`Ok(false)`=白名单外 / 触达禁止集不通过；
/// `Err(ConstraintError)`=kind 未知 / spec 非法 / 判定所需对象缺失。后两者皆由内核翻译
/// 为拒绝，「判不了」绝不退化为 `Ok(true)`（L-7 fail-closed）。
pub fn check(spec: &ConstraintSpec, ci: &ClassifiedIntent) -> Result<bool, ConstraintError> {
    match spec.kind.as_str() {
        "table_allow" => check_table_allow(spec, ci),
        "column_mask" => check_column_mask(spec, ci),
        // 非本属主（跨协议 kind、出口期 mask_fields 等）→ 未知，绝不误判为属主 kind。
        _ => Err(ConstraintError::UnknownKind),
    }
}

/// `table_allow`（集合包含类·全称量化）：`ci` 触达的**每一张**表都须在白名单内。
///
/// 表维度对象=不带 `col:` 前缀的 `ObjectRef`。无任何表维度对象 → `MissingObjects`
/// （判不了）。任一表越界（空白名单时任何表皆越界）→ `Ok(false)`。
fn check_table_allow(
    spec: &ConstraintSpec,
    ci: &ClassifiedIntent,
) -> Result<bool, ConstraintError> {
    let parsed: TableAllowSpec =
        serde_json::from_str(&spec.spec).map_err(|_| ConstraintError::InvalidSpec)?;

    let touched: Vec<&str> = ci
        .objects
        .iter()
        .map(ObjectRef::as_str)
        .filter(|o| !is_column_ref(o))
        .collect();

    // 表维度对象缺失：判不了 → MissingObjects，绝不 Ok(true)（L-7）。
    if touched.is_empty() {
        return Err(ConstraintError::MissingObjects);
    }

    // 全称量化：每一张触达表都须在白名单内；任一越界即整体不通过。
    let all_inside = touched.iter().all(|t| spec_contains(&parsed.tables, t));
    Ok(all_inside)
}

/// `column_mask`（触达禁止类）：`ci` 触达列集与禁止集交集须为空。
///
/// 列维度对象=带 `col:` 前缀的 `ObjectRef`（取前缀后的裸 `schema.table.column` 比对）。
/// 无任何列维度对象 → `MissingObjects`（无法确认未触达，判不了）。交集非空 → `Ok(false)`。
fn check_column_mask(
    spec: &ConstraintSpec,
    ci: &ClassifiedIntent,
) -> Result<bool, ConstraintError> {
    let parsed: ColumnMaskSpec =
        serde_json::from_str(&spec.spec).map_err(|_| ConstraintError::InvalidSpec)?;

    let touched: Vec<&str> = ci
        .objects
        .iter()
        .map(ObjectRef::as_str)
        .filter_map(strip_column_prefix)
        .collect();

    // 列维度对象缺失：无法确认未触达 → 判不了 → MissingObjects，绝不 Ok(true)（L-7）。
    if touched.is_empty() {
        return Err(ConstraintError::MissingObjects);
    }

    // 触达禁止类：交集须为空；任一触达列落禁止集即整体不通过。
    let any_forbidden = touched.iter().any(|c| spec_contains(&parsed.columns, c));
    Ok(!any_forbidden)
}

/// 是否为列维度 `ObjectRef`（带 `col:` 前缀）。
fn is_column_ref(reference: &str) -> bool {
    reference.starts_with(COLUMN_PREFIX)
}

/// 取列维度对象前缀后的裸 `schema.table.column`；非列维度对象返回 `None`。
fn strip_column_prefix(reference: &str) -> Option<&str> {
    reference.strip_prefix(COLUMN_PREFIX)
}

/// 声明集是否含某项（白名单包含 / 禁止集命中的统一判据）。
fn spec_contains(set: &[String], item: &str) -> bool {
    set.iter().any(|e| e == item)
}
