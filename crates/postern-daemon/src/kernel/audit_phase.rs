//! 两阶段审计时序（模块文档 06 §8.2 / §6.2；§8 F-3 / L-3）。
//!
//! 动词分两类，审计时序不同：
//! - **只读动词**（observe / query）：execute 后**单条** record；record 写失败 → deny
//!   （不可留痕即不可放行，stage=audit）。读动词**绝不**写意图痕（F-3）。
//! - **有副作用动词**（mutate / execute / manage / destroy）：execute **之前**写意图痕
//!   [7a]；意图写失败 → deny 于 execute 之前、`Adapter::execute` **不被调用**（L-3 第②分支）。
//!   execute **之后**写结果痕 [10]；结果写失败 → 返回「已执行但审计降级」码，**绝不 deny**
//!   （L-3 第③分支：已执行的请求永不返 deny，两阶段不变量）。
//!
//! 两阶段有严格先后；审计为同步 store 调用，由 kernel 置于 spawn_blocking 边界，绝不在
//! async worker 直接阻塞。本目录禁吞错字样：写痕失败显式带 stage / 显式降级码。
//!
//! 错误处理全程显式 `match` / `?`，绝不吞错放行（契约 EVAL_NO_ERROR_SWALLOWING 扫本目录）。

use std::sync::Arc;

use postern_core::domain::Capability;
use postern_core::error::AuditError;
use postern_core::plugin::{AuditEvent, AuditSink};

use crate::error::{DaemonError, OutcomeDegraded};

/// 审计动词类别（§6.2）：决定两阶段时序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditClass {
    /// 只读动词（observe / query）：execute 后单条 record，无意图痕。
    Read,
    /// 有副作用动词（mutate / execute / manage / destroy）：意图痕在前、结果痕在后。
    SideEffecting,
}

impl AuditClass {
    /// 由分类后的动词判别审计类别（§6.2 读 vs 副作用边界）。
    ///
    /// 穷尽 per-variant match、无 `_ =>` 兜底：新增 `Capability` 变体不在此声明类别即编译
    /// 失败（绝不让未知动词静默落入某一类、错配审计时序）。
    pub fn of(capability: Capability) -> AuditClass {
        match capability {
            Capability::Observe => AuditClass::Read,
            Capability::Query => AuditClass::Read,
            Capability::Mutate => AuditClass::SideEffecting,
            Capability::Execute => AuditClass::SideEffecting,
            Capability::Manage => AuditClass::SideEffecting,
            Capability::Destroy => AuditClass::SideEffecting,
        }
    }
}

/// 两阶段审计协调器：持审计汇句柄，按动词类别编排意图痕 / 结果痕 / 单条读痕的时序。
pub struct AuditPhase {
    /// 同步审计汇（store 实现）；每次写痕在 spawn_blocking 边界调用，绝不阻塞 async worker。
    sink: Arc<dyn AuditSink>,
}

impl AuditPhase {
    /// 由注入的审计汇句柄构造协调器。
    pub fn new(sink: Arc<dyn AuditSink>) -> Self {
        Self { sink }
    }

    /// [7a] 有副作用动词的意图痕（execute **之前**）。
    ///
    /// 写失败 → `Err`（kernel 据此在 execute 之前 deny，`Adapter::execute` 不被调用）。
    /// 同步 store 调用置于 spawn_blocking 边界。
    pub async fn record_intent(&self, event: AuditEvent) -> crate::error::Result<()> {
        match self.record_blocking(event).await {
            Ok(()) => Ok(()),
            Err(_cause) => Err(DaemonError::Boot),
        }
    }

    /// [10] 有副作用动词的结果痕（execute **之后**）。
    ///
    /// 写失败 → `Err(OutcomeDegraded)`「已执行但审计降级」，**绝不 deny**（已执行不变量）。
    pub async fn record_outcome(
        &self,
        event: AuditEvent,
    ) -> std::result::Result<(), OutcomeDegraded> {
        match self.record_blocking(event).await {
            Ok(()) => Ok(()),
            Err(cause) => Err(OutcomeDegraded { cause }),
        }
    }

    /// [10] 只读动词的单条结果痕（execute **之后**；读动词无意图痕）。
    ///
    /// 写失败 → `Err`（kernel 据此 deny，stage=audit：不可留痕即不可放行）。
    pub async fn record_read(&self, event: AuditEvent) -> crate::error::Result<()> {
        match self.record_blocking(event).await {
            Ok(()) => Ok(()),
            Err(_cause) => Err(DaemonError::Boot),
        }
    }

    /// 把同步 `AuditSink::record` 置于 spawn_blocking 边界执行（绝不在 async worker 直接阻塞）。
    ///
    /// 返回底层审计写失败族（供调用方按 read/intent/outcome 各自语义分流）。`spawn_blocking`
    /// 的 join 本身失败（OS 线程池 panic / 取消）一律 fail-closed 折叠为一次写失败
    /// （`AuditError::WriteFailed`），绝不静默成功。
    async fn record_blocking(&self, event: AuditEvent) -> std::result::Result<(), AuditError> {
        let sink = Arc::clone(&self.sink);
        let joined = tokio::task::spawn_blocking(move || sink.record(event)).await;
        match joined {
            Ok(write_result) => write_result,
            Err(_join) => Err(AuditError::WriteFailed),
        }
    }
}
