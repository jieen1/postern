//! 审计域：append-only JSONL 载体与扫描读路径。
//!
//! `JsonlAuditSink` 实现 `core::AuditSink`（写路径 `record`）并提供本地读路径
//! `scan`，完全独立于 policy.db（不走写锁、不碰关系数据库驱动）。读路径返回 store
//! 本地读模型 `Page<AuditRecord>`（origin 用本地 `OriginEnvelope`），全程不构造
//! `ConnOrigin`（设计裁定，§5 读模型）。

pub mod record;
pub mod scan;
pub mod sink;

pub use record::{AuditRecord, OriginEnvelope};
pub use scan::AuditFilter;
pub use sink::{FsyncPolicy, JsonlAuditSink};
