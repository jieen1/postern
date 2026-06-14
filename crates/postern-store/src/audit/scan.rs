//! 审计扫描：按日期文件倒序、分页窗口截断的读路径，返回 store 本地读模型。
//!
//! 按日期文件倒序扫描、serde_json 逐行解析、按分页窗口截断、命中页满即停，
//! 绝不全量读入内存。反序列化只产 store 本地 origin 结构（OriginEnvelope），
//! 全程不构造来源类型。

use std::io::{BufRead, BufReader};
use std::path::Path;

use postern_core::error::AuditError;
use postern_core::page::{Page, PageQuery};

use super::record::AuditRecord;

/// 审计扫描过滤器（store 本地读模型，非 core 类型）。
///
/// `kind` 限定事件 kind（`None` = 不限）；`from_date`/`to_date` 限定 UTC 日界窗口
/// （含端点，文本 `YYYY-MM-DD` 字典序 == 日期序，`None` = 不限）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditFilter {
    /// 限定事件 kind；`None` 表示不过滤 kind。
    pub kind: Option<String>,
    /// 窗口起始 UTC 日界（含），`YYYY-MM-DD`。
    pub from_date: Option<String>,
    /// 窗口结束 UTC 日界（含），`YYYY-MM-DD`。
    pub to_date: Option<String>,
}

impl AuditFilter {
    /// 不限定任何条件的全量过滤器。
    pub fn all() -> Self {
        Self {
            kind: None,
            from_date: None,
            to_date: None,
        }
    }

    /// 只限定 kind 的过滤器。
    pub fn by_kind(kind: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            from_date: None,
            to_date: None,
        }
    }

    /// 一条记录是否命中 kind 过滤（日界窗口由文件名层面预筛，此处只判 kind）。
    fn matches(&self, record: &AuditRecord) -> bool {
        match &self.kind {
            Some(k) => &record.kind == k,
            None => true,
        }
    }

    /// 文件日界是否落在 `[from_date, to_date]`（含端点；`YYYY-MM-DD` 字典序==日期序）。
    fn date_in_window(&self, date: &str) -> bool {
        if let Some(from) = &self.from_date {
            if date < from.as_str() {
                return false;
            }
        }
        if let Some(to) = &self.to_date {
            if date > to.as_str() {
                return false;
            }
        }
        true
    }
}

/// 读路径实现：按日期文件倒序、逐行解析、分页窗口截断，返回 store 本地读模型。
///
/// `page` 先 clamp（`DB_PAGINATION_MANDATORY` 对扫描查询同样生效）；按日期文件
/// **倒序**（较晚日期先）、文件内按追加逆序（较晚事件先）逐行流式解析，只把命中
/// 分页窗口的记录物化进 `items`，绝不全量读入内存。`total` 反映全部命中记录数。
pub(crate) fn scan(
    audit_dir: &Path,
    filter: &AuditFilter,
    page: PageQuery,
) -> Result<Page<AuditRecord>, AuditError> {
    let page = page.clamp();
    let offset =
        (u64::from(page.page_no).saturating_sub(1)).saturating_mul(u64::from(page.page_size));
    let window_end = offset.saturating_add(u64::from(page.page_size));

    // 倒序日期文件名（较晚日期先）；目录不存在视为空集，不是失败。
    let mut dates = match day_files(audit_dir) {
        Ok(dates) => dates,
        Err(()) => return Ok(empty_page(page)),
    };
    dates.sort();
    dates.reverse();

    let mut total: u64 = 0;
    let mut items: Vec<AuditRecord> = Vec::new();

    for date in dates {
        if !filter.date_in_window(&date) {
            continue;
        }
        let path = audit_dir.join(format!("{date}.jsonl"));
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        // 文件内逐行流式解析后按追加逆序遍历（较晚事件先），保持全局倒序。
        let mut parsed: Vec<AuditRecord> = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|_| AuditError::StorageUnavailable)?;
            if line.trim().is_empty() {
                continue;
            }
            let record: AuditRecord =
                serde_json::from_str(&line).map_err(|_| AuditError::WriteFailed)?;
            if filter.matches(&record) {
                parsed.push(record);
            }
        }
        for record in parsed.into_iter().rev() {
            // 落在分页窗口内才物化进 items；其余只计入 total。
            if total >= offset && total < window_end {
                items.push(record);
            }
            total = total.saturating_add(1);
        }
    }

    Ok(Page {
        items,
        page_no: page.page_no,
        page_size: page.page_size,
        total,
    })
}

/// 列出审计目录下全部 `YYYY-MM-DD.jsonl` 文件的日界文本（无序）。
/// 目录不存在返回 `Err(())`（上层视为空集，不是写失败）。
fn day_files(audit_dir: &Path) -> Result<Vec<String>, ()> {
    let entries = std::fs::read_dir(audit_dir).map_err(|_| ())?;
    let mut dates = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() == 16 && name.ends_with(".jsonl") {
            if let Some(date) = name.get(..10) {
                dates.push(date.to_string());
            }
        }
    }
    Ok(dates)
}

/// 空结果信封（回显 clamp 后的分页参数、total=0）。
fn empty_page(page: PageQuery) -> Page<AuditRecord> {
    Page {
        items: Vec::new(),
        page_no: page.page_no,
        page_size: page.page_size,
        total: 0,
    }
}
