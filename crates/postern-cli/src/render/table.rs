//! `Page<T>` 表格化（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.3/§3.4，F-4/F-5/F-6/F-7）：把 core `Page<T>`（`items` 加分页
//! 元信息、每条携 `version`）默认渲染为对齐表格（列取自 DTO 字段），分页游标信息（当前页、
//! 页大小、是否有下一页）作页脚提示给人，便于人决定下一条命令的 `--page-no`。
//!
//! 关键纪律：集合"下一页"靠人再发一条命令携新 `page_no`，**不在客户端续抓**（分页职责
//! 整体在后端，§3.4、契约 DB_PAGINATION_MANDATORY）；CLI 端不存在"取回全量再本地切片"
//! 的代码路径。每条携带的 `version` 原样渲染给人，供后续乐观锁回传（只透传不自造，F-7）。
//! 雪花 id 列**静态类型即字符串**，原样作字符串展示、绝不数值化（F-5：从类型层杜绝 JSON
//! 数字解析路径，>2^53 不丢精度、不变科学计数）。

use serde::{Deserialize, Serialize};

use postern_core::page::Page;

use super::envelope::Format;
use crate::error::CliError;

/// CLI 侧通用集合行视图（Deserialize）。
///
/// F-5 由构造签名保证：所有雪花 id 字段（`id`/`principal_id`/`resource_id`/
/// `credential_id`）静态类型恒为 `String`，绝不 `u64`/`i64`/`serde_json::Number`——任何
/// 整型路径都会在 >2^53 时丢精度或变科学计数，类型层即不可表示。`version` 同为乐观锁
/// 透传载体，原样渲染给人（F-7，只透传不自造）。所有字段 `Option` 以容纳不同集合端点的
/// 投影差异，缺字段不报错（缺的是 id 之外的可选投影列，非契约必需字段）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RowView {
    /// 本行主体雪花 id（字符串恒定，F-5）。
    #[serde(default)]
    pub id: Option<String>,
    /// 关联 Principal 雪花 id（字符串恒定，F-5）。
    #[serde(default)]
    pub principal_id: Option<String>,
    /// 关联资源雪花 id（字符串恒定，F-5）。
    #[serde(default)]
    pub resource_id: Option<String>,
    /// 关联凭据雪花 id（字符串恒定，F-5）。
    #[serde(default)]
    pub credential_id: Option<String>,
    /// 人类可读名 / 代号列（如适用）。
    #[serde(default)]
    pub name: Option<String>,
    /// 乐观锁版本，原样渲染供回传（F-7）。`version` 在协议里是整数，不是雪花 id。
    #[serde(default)]
    pub version: Option<u64>,
}

/// 分页页脚游标提示（§3.3）：当前页 / 页大小 / 是否有下一页——纯展示，供人决定下一条
/// 命令的 `--page-no`。`has_next` 由 `items` 数、`page_no`、`page_size`、`total` 机械导出，
/// CLI **不**据此续抓（不在客户端续页，§3.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagerFooter {
    /// 当前页号（1-based，回显响应携带值）。
    pub page_no: u32,
    /// 当前页大小（回显响应携带值）。
    pub page_size: u32,
    /// 是否还有下一页（机械导出，仅提示，不触发客户端续抓）。
    pub has_next: bool,
}

impl PagerFooter {
    /// 由 `Page<T>` 的分页元信息机械导出页脚游标——纯函数，不发任何请求、不续抓。
    /// `has_next = page_no * page_size < total`（机械算式，无客户端聚合）。
    pub fn from_page<T>(page: &Page<T>) -> Self {
        let covered = (page.page_no as u64).saturating_mul(page.page_size as u64);
        Self {
            page_no: page.page_no,
            page_size: page.page_size,
            has_next: covered < page.total,
        }
    }
}

/// 列标题（与下方逐行取值一一对应；id 列恒为字符串列，F-5）。
const COLUMNS: [&str; 6] = [
    "id",
    "principal_id",
    "resource_id",
    "credential_id",
    "name",
    "version",
];

/// 取一行各列的文本值（缺省列以空串占位；雪花 id 原样字符串、`version` 十进制文本）。
fn row_cells(row: &RowView) -> [String; 6] {
    let opt = |v: &Option<String>| v.clone().unwrap_or_default();
    [
        opt(&row.id),
        opt(&row.principal_id),
        opt(&row.resource_id),
        opt(&row.credential_id),
        opt(&row.name),
        row.version.map(|v| v.to_string()).unwrap_or_default(),
    ]
}

/// 把 `Page<RowView>` 默认渲染为对齐表格 + 分页页脚（F-4）。
///
/// 设计承诺：列取自 `RowView` 字段；每行雪花 id 列原样作字符串（F-5）；页脚回显当前
/// 页 / 页大小 / 是否有下一页（§3.3）。表格本身**回显该响应携带的 `version`**（F-4 判定
/// 要求"原样回显该响应携带的 `version`"，F-7 供乐观锁回传）。
///
/// 渲染失败（格式化 / 写出）→ `CliError`，不 `panic`（§3.9 fail-closed）。
pub fn render_page(page: &Page<RowView>) -> Result<String, CliError> {
    use std::fmt::Write as _;

    let rows: Vec<[String; 6]> = page.items.iter().map(row_cells).collect();

    // 逐列对齐宽度 = 标题与所有单元格里最长的那个（雪花 id 全宽展示，绝不截断）。
    let mut widths = [0usize; 6];
    for (col, head) in COLUMNS.iter().enumerate() {
        widths[col] = head.len();
    }
    for cells in &rows {
        for (col, cell) in cells.iter().enumerate() {
            widths[col] = widths[col].max(cell.len());
        }
    }

    let mut out = String::new();

    // 标题行。
    for (col, head) in COLUMNS.iter().enumerate() {
        let _ = write!(out, "{:<width$}  ", head, width = widths[col]);
    }
    out.push('\n');

    // 数据行（id 列已是字符串，version 列已是十进制文本——无浮点 / 科学计数路径）。
    for cells in &rows {
        for (col, cell) in cells.iter().enumerate() {
            let _ = write!(out, "{:<width$}  ", cell, width = widths[col]);
        }
        out.push('\n');
    }

    // 分页页脚：当前页 / 页大小 / 是否有下一页（纯展示，不触发客户端续抓）。
    let footer = PagerFooter::from_page(page);
    let _ = writeln!(
        out,
        "page {} / size {} / has_next {} / total {}",
        footer.page_no, footer.page_size, footer.has_next, page.total
    );

    Ok(out)
}

/// `--format jsonl`：把后端**已分页**的 `items` 逐行打印为独立可解析 JSON（F-4）。
///
/// 设计承诺：每行一个 JSON 对象（独立可被 JSON 解析），雪花 id 仍为字符串（F-5）；此形态
/// 只是把后端 `items` 逐行回放，**不做客户端重排或聚合**（§3.3）。
pub fn render_jsonl(page: &Page<RowView>) -> Result<String, CliError> {
    use std::fmt::Write as _;

    // 严格按后端 `items` 原序逐行序列化，不重排、不聚合；id 字段恒为 JSON 字符串。
    let mut out = String::new();
    for row in &page.items {
        let line = serde_json::to_string(row).map_err(|_| CliError::DecodeFailed {
            detail: "failed to serialize paged item to jsonl".to_string(),
        })?;
        let _ = writeln!(out, "{line}");
    }
    Ok(out)
}

/// 信封三分支的"集合"一支总入口：据 `Format` 选表格或 jsonl 渲染器。
///
/// 反序列化失败即返回 `CliError::DecodeFailed`（L-3，不补全、不当成功）。
pub fn render_page_envelope(bytes: &[u8], format: Format) -> Result<String, CliError> {
    // items 内任一行 id 是 JSON 数字（无引号）→ String 字段拒收 → DecodeFailed（无数字路径）。
    let page: Page<RowView> =
        serde_json::from_slice(bytes).map_err(|_| CliError::DecodeFailed {
            detail: "page envelope did not match shared-type contract".to_string(),
        })?;
    match format {
        Format::Table => render_page(&page),
        Format::Jsonl => render_jsonl(&page),
    }
}
