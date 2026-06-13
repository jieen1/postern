//! 启动期自检：data.sock 开放前的「可连 uid」前置不变量复核（F-2 / §3.1·6，设计承诺级桩）。
//!
//! 在 data.sock **创建之前**确认其可连 uid 有效集合不含 daemon 自身 uid——若 Agent 与 daemon
//! 同 uid（可连集含自身），fail-closed 拒绝启动（非零退出、data.sock 不创建、数据面未开放），
//! 杜绝「同 uid 还能跑」的静默不安全状态。判定是对「当前 umask/属组/ACL 下哪些 uid 能 connect」
//! 的**有效集合**测，**不是**读请求自报字段。自检过即放行进入 data.sock 创建（整链终结动作）。

/// 可连 uid 自检结果：是否放行 data.sock 开放（含自身 uid 即拒绝）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheck {
    /// 可连 uid 集合**不含**自身 uid → 放行 data.sock 开放（F-2 正常路径）。
    Pass,
    /// 可连 uid 集合**含**自身 uid → fail-closed 拒绝启动（data.sock 不创建）。
    RefuseSameUid,
}

impl SelfCheck {
    /// 是否放行 data.sock 开放（仅 `Pass` 放行）。
    pub fn is_pass(self) -> bool {
        matches!(self, SelfCheck::Pass)
    }
}

/// 可连 uid 自检：给定 daemon 自身 uid 与 data.sock 可连 uid 有效集合，判定是否放行。
///
/// 集合含自身 uid → [`SelfCheck::RefuseSameUid`]（fail-closed）；不含 → [`SelfCheck::Pass`]。
/// 纯判定、无 IO；有效集合由调用方（[`ConnectableUidProbe`](crate::boot::ConnectableUidProbe)）
/// 经真实环境探测得出，本函数只做集合成员判定（占位）。
pub fn connectable_uid_check(self_uid: u32, connectable: &[u32]) -> SelfCheck {
    if connectable.contains(&self_uid) {
        SelfCheck::RefuseSameUid
    } else {
        SelfCheck::Pass
    }
}
