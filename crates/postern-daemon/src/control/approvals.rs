//! 控制面审批：内存待审升权队列，`on_timeout` 恒固定为 deny（模块文档 06 §6.10、§8 L-12）。
//!
//! escalate → 内存待审队列；`on_timeout` **恒固定为 deny**（fail-closed，不可配置）。审批
//! **关闭**时 escalate **不入队**——直接 `escalate_denied`（L-12）；进程**重启** ⇒ 所有待审
//! 一律 deny（队列只在内存，core 不持 pending state）。把"恒拒绝"显式编码于此，杜绝被误配置
//! 成在线放行（fail-closed 的设计冗余）。
//!
//! 内存待审队列：审批关闭即不入队恒 deny；开启入队但在线恒 deny；重启 deny 全部并清空。

use std::sync::Mutex;

use super::{ApprovalOutcome, PendingApproval};

/// 内存待审升权队列（§8 L-12）。
///
/// 仅活在内存——进程重启即清空，所有待审一律 deny（无持久 pending state）。审批是否开启由
/// `approvals_enabled` 决定：**关闭**时 [`submit`] 不入队、直接回 [`ApprovalOutcome::Denied`]。
/// `on_timeout` 在类型层不存在——超时处置恒 deny，无 allow 配置面（不可被改成在线放行）。
///
/// [`submit`]: ApprovalQueue::submit
pub struct ApprovalQueue {
    /// 审批是否开启：关闭时 escalate 不入队、直接 deny（L-12）。
    approvals_enabled: bool,
    /// 内存待审队列；进程重启即丢失（所有待审随之 deny）。
    pending: Mutex<Vec<PendingApproval>>,
}

impl ApprovalQueue {
    /// 装配一个内存待审队列（审批开关由 boot 据 settings 传入）。
    pub fn new(approvals_enabled: bool) -> Self {
        Self {
            approvals_enabled,
            pending: Mutex::new(Vec::new()),
        }
    }

    /// 提交一次 escalate（§8 L-12）。
    ///
    /// 审批**关闭** ⇒ **不入队**、直接回 [`ApprovalOutcome::Denied`]（`escalate_denied`，
    /// fail-closed）；审批开启 ⇒ 入内存待审队列、回 [`ApprovalOutcome::Denied`]（在线恒不
    /// allow——审批结果由带外人工流程在控制面 `POST /v1/approvals` 落地，绝非在线放行）。
    /// 无论开关，**在线提交恒不返回 allow**（ApprovalOutcome 无 allow 变体）。
    pub fn submit(&self, request: PendingApproval) -> ApprovalOutcome {
        // 审批关闭 ⇒ 绝不入队、直接 deny（escalate_denied，fail-closed）。
        if self.approvals_enabled {
            // 审批开启：入内存待审队列（在线结果仍恒 deny——allow 由带外人工流程落地）。
            if let Ok(mut pending) = self.pending.lock() {
                pending.push(request);
            }
        }
        // 无论开关，在线提交恒不返回 allow（ApprovalOutcome 无 allow 变体）。
        ApprovalOutcome::Denied
    }

    /// 当前内存待审队列长度（对账锚点：审批关闭时恒 0——不入队）。
    pub fn pending_len(&self) -> usize {
        match self.pending.lock() {
            Ok(pending) => pending.len(),
            // 锁中毒 ⇒ fail-closed 视作不可对账（绝不谎报队列空让在线放行）。
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    /// 进程重启 / 收口：把所有待审一律 deny 并清空队列（§8 L-12）。
    ///
    /// 返回被 deny 的待审条数。重启后队列为空——重启即 deny，无持久 pending state。
    pub fn deny_all_on_restart(&self) -> usize {
        let mut pending = match self.pending.lock() {
            Ok(pending) => pending,
            Err(poisoned) => poisoned.into_inner(),
        };
        let denied = pending.len();
        pending.clear();
        denied
    }
}
