//! 数据面请求内核子域（模块文档 06 §8.2）。
//!
//! 承载 [0]→[10] 的线性短路求值链：每一阶段（auth / classify / rbac / constraint /
//! condition / connect）要么放行进入下一阶，要么立即以带 stage 的结构化 deny 短路。
//! 求值链任一步的 Err 必须落到对应 stage 的 deny，绝不吞错放行（契约
//! EVAL_NO_ERROR_SWALLOWING 扫描本目录）。[4] check_constraint 先于 evaluate；两阶段
//! 审计有严格时序；出口统一脱敏。
//!
//! 本目录任何文件禁出现错误吞没字样（求值链失败显式带 stage）。需要 ConnOrigin 时以
//! `postern_core::request::ConnOrigin as Origin` 别名读/解构，绝不写字面变体。
//!
//! 本波次为骨架：管线与审计阶段桩，零求值逻辑。

pub mod audit_phase;
pub mod pipeline;

pub use audit_phase::{AuditClass, AuditPhase};
pub use pipeline::{ConnAcquire, Kernel};
