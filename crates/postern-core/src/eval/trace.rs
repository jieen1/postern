//! 求值轨迹累积（EvalTrace 的逐步组装）。
//!
//! 轨迹类型 `EvalTrace` / `TraceStep` 的**所有权**在 `decision` 单元
//! （已完成、冻结）：本模块不重定义、不分裂所有权——只在求值管线命名空间内
//! **再导出**这些权威类型，并提供一个**累积辅助**（构建器）供 `evaluate`
//! 在管线推进时逐步追加记录、末了产出确定性 `EvalTrace`。
//!
//! 累积语义（模块设计 §3.3 / 详细设计 8.3）：每进入一步即登记"到达该步"，
//! 判定时登记"在该步、因何判定"（命中/未命中、谓词 kind 与结论、tier 选择
//! 结果）。短路发生时轨迹截止于当前步，其最后一条 `stage` 即拒绝阶段——
//! 直接喂给审计 `stage` 字段与拒绝响应组装。
//!
//! 确定性：`detail` 文本机械取自快照/入参事实（stage 名、谓词 kind、资源代号、
//! 动词等策略事实），同输入逐字一致；容器恒为 `Vec`（已定）保证迭代序确定。
//! 轨迹只承载代号与策略事实，绝无机密（`Intent` / `PresentedCredential`
//! 不入 `detail`）。本模块不做判定、不调插件，纯轨迹组装。

use crate::error::Stage;

pub use crate::decision::{EvalTrace, TraceStep};

/// 求值轨迹累积器：按管线顺序逐步 `push` 一条 `(Stage, detail)` 记录，
/// 末了 `finish` 产出权威 `EvalTrace`。内部容器恒为 `Vec`，迭代序即追加序，
/// 同输入逐字节一致。
pub struct TraceBuilder {
    steps: Vec<TraceStep>,
}

impl TraceBuilder {
    /// 起一条空轨迹累积器（尚未到达任何步）。
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    /// 追加一条记录："到达 `stage`，在该步 `detail` 所述判定"。
    /// 短路时最后一次 `push` 的 `stage` 即拒绝阶段。`detail` 文本机械取自
    /// 快照/入参事实——只承载代号与策略事实，调用方负责不传入任何机密。
    pub fn push(&mut self, stage: Stage, detail: impl Into<String>) {
        self.steps.push(TraceStep {
            stage,
            detail: detail.into(),
        });
    }

    /// 消费累积器、产出权威 `EvalTrace`（steps 顺序即追加序，迭代序确定）。
    pub fn finish(self) -> EvalTrace {
        EvalTrace { steps: self.steps }
    }
}

impl Default for TraceBuilder {
    fn default() -> Self {
        Self::new()
    }
}
