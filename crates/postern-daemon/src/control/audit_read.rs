//! 控制面审计**读**句柄的真实实现：把 [`AuditRead`](super::AuditRead) 接到 store 的
//! append-only 审计载体（[`JsonlAuditSink::scan`](postern_store::audit::sink::JsonlAuditSink::scan)
//! + deny 聚合），与 policy.db 读写句柄（[`PolicyRepo`](super::PolicyRepo)）截然分离。
//!
//! 审计落 JSONL（按 UTC 日轮转、物理只追加），不走 policy.db 写锁——故 policy 读句柄够不到
//! 审计读。本句柄持**同一** `JsonlAuditSink` 实例（boot 在控制面装配处与三联动审计写支复用一
//! 个实例，单一权威载体，无双源）：
//! - `scan_audit`：经 `scan(AuditFilter::all(), page)` 取倒序分页窗口，投影为 [`AuditEventDto`]。
//! - `denials_summary`：经 `scan(AuditFilter::by_kind("deny"), page)` 取 deny 类记录，投影为
//!   [`DenialSummaryDto`]（聚合记录 `count` 为窗口折叠条数，逐事件 deny 视同 1）。
//!
//! 纪律（雷区）：读路径**绝不**构造 `ConnOrigin`——store 本地 [`OriginEnvelope`] 经 [`origin_text`]
//! 投影为**已脱敏不透明文本**（uid/gid 为本地信任域门标记；TCP 仅脱敏占位、不回显真实地址语义，
//! 公理四 / §5 读模型）。id 一律字符串（store 写时已序列化）；序列化失败 fail-closed → Boot。

use std::sync::Arc;

use postern_core::page::{Page, PageQuery};

use postern_store::audit::record::{AuditRecord, OriginEnvelope};
use postern_store::audit::scan::AuditFilter;
use postern_store::audit::sink::JsonlAuditSink;

use super::dto::{AuditEventDto, DenialSummaryDto};
use super::AuditRead;
use crate::error::DaemonError;

/// 真实审计读句柄：持与三联动审计写支**同一** [`JsonlAuditSink`] 实例（boot 复用，单一载体）。
///
/// 只读消费（`scan` / deny 聚合投影）——`record`（写支）由注入集合里的
/// [`AuditSink`](postern_core::plugin::AuditSink) 持有，二者是同一物理载体的读 / 写两支。
pub struct JsonlAuditReader {
    sink: Arc<JsonlAuditSink>,
}

impl JsonlAuditReader {
    /// 由共享的 [`JsonlAuditSink`] 实例构造（boot 在控制面装配处与审计写支复用同一 `Arc`）。
    pub fn new(sink: Arc<JsonlAuditSink>) -> Self {
        Self { sink }
    }
}

impl AuditRead for JsonlAuditReader {
    fn scan_audit(&self, page: PageQuery) -> Result<Page<serde_json::Value>, DaemonError> {
        // 全量倒序扫描（kind 不限），分页窗口截断；扫描失败 fail-closed → Boot（不伪造空信封）。
        let page = self
            .sink
            .scan(&AuditFilter::all(), page)
            .map_err(|_| DaemonError::Boot)?;
        project(page, audit_event_dto)
    }

    fn denials_summary(&self, page: PageQuery) -> Result<Page<serde_json::Value>, DaemonError> {
        // 只取 deny 类记录（含聚合记录），投影为拒绝摘要行；扫描失败 fail-closed → Boot。
        let page = self
            .sink
            .scan(&AuditFilter::by_kind("deny"), page)
            .map_err(|_| DaemonError::Boot)?;
        project(page, denial_summary_dto)
    }
}

/// 把一页 [`AuditRecord`] 经 `to_dto` 投影为出线 DTO 的 `serde_json::Value`，保留分页元数据。
/// 序列化失败（DTO 恒可序列化，理应不发生）⇒ fail-closed → Boot。
fn project<D, F>(page: Page<AuditRecord>, to_dto: F) -> Result<Page<serde_json::Value>, DaemonError>
where
    D: serde::Serialize,
    F: Fn(AuditRecord) -> D,
{
    let mut items = Vec::with_capacity(page.items.len());
    for record in page.items {
        let value = serde_json::to_value(to_dto(record)).map_err(|_| DaemonError::Boot)?;
        items.push(value);
    }
    Ok(Page {
        items,
        page_no: page.page_no,
        page_size: page.page_size,
        total: page.total,
    })
}

/// [`AuditRecord`] → [`AuditEventDto`]：`origin` 经 [`origin_text`] 脱敏为不透明文本
/// （绝不构造 `ConnOrigin`）；`policy_rev` 字符串化（u64 同雪花纪律）。
fn audit_event_dto(record: AuditRecord) -> AuditEventDto {
    AuditEventDto {
        id: record.id,
        v: record.v,
        kind: record.kind,
        ts: record.ts,
        entry: record.entry,
        origin: origin_text(&record.origin),
        decision: record.decision,
        resource: record.resource,
        policy_rev: record.policy_rev.to_string(),
    }
}

/// deny 类 [`AuditRecord`] → [`DenialSummaryDto`]：`count` 取聚合多重性（`None` 视同 1）；
/// `resource` 恒代号（绝非真实地址）；`policy_rev` 字符串化。
fn denial_summary_dto(record: AuditRecord) -> DenialSummaryDto {
    DenialSummaryDto {
        resource: record.resource,
        count: record.count.unwrap_or(1),
        policy_rev: record.policy_rev.to_string(),
    }
}

/// store 本地 [`OriginEnvelope`] → **已脱敏不透明来源文本**（绝不构造 `ConnOrigin`、绝不回显
/// 真实 TCP 地址语义）。本地对端只出信任域门（uid/gid）；TCP 只出脱敏占位标记（真实地址语义
/// 由内核出口 Sanitizer 在数据面保证，控制面审计读模型本就不持真实地址）。
fn origin_text(origin: &OriginEnvelope) -> String {
    match origin {
        OriginEnvelope::UnixPeer { uid, gid } => format!("unix:{uid}:{gid}"),
        // TCP 来源只出脱敏标记，绝不回显 `remote` 文本（控制面审计读模型不泄露地址）。
        OriginEnvelope::Tcp { .. } => "tcp".to_string(),
    }
}
