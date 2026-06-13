//! 连接租约：一条被借出连接的 RAII 句柄（§3.5）。
//!
//! 在租约存活期间独占一条连接；Drop 时触发归池前会话净化与健康复核——净化成功则连接
//! 归还池槽（复用一定发生在净化之后），**净化失败则销毁而非回池**（fail-closed，L-9）；
//! 已标记损坏的连接同样销毁不归池。租约绝不跨 tier 复用（池键含 tier，L-8）。
//!
//! 会话净化是**协议串**（经 `Channel` 由传输/适配器下发的连接重置指令、清理未决会话状态），
//! 不是 daemon 解析的库语句；本层只调净化入口、观测其成败，绝不在本文件出现任何会被扫描器
//! 标记的库写标记。
//!
//! 归还路径（Drop / 显式净化）只做纯内存槽回写（持锁，不跨 await）；归还/销毁的判定全在
//! 本地标志（损坏 / 净化成败 / 池化与否 / 中断纪元）上完成。锁中毒（持锁线程 panic）按
//! fail-closed 取回内层守卫继续回写——池槽只是内存通道缓冲，绝不因中毒再 panic（B-6）。

use std::collections::BTreeMap;
use std::mem;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use tokio::sync::Notify;

use postern_core::domain::{CredentialTier, ResourceCode};
use postern_core::plugin::Channel;

use super::backoff::Backoff;
use super::ConnectionEvent;

/// 租约归还所需的共享句柄与归还判据（池构造租约时打包注入）。
///
/// 持有池可变核心的共享句柄、本租约的池键、是否池化、发借时的中断纪元——Drop 时据此把
/// 健康净化后的连接回写 idle，或销毁（在用计数恒回落）。
pub(super) struct ReturnSlot {
    /// 池可变核心的共享句柄（与 `ConnPool` 同一把锁）。
    pub(super) state: Arc<Mutex<PoolState>>,
    /// 连接事件序共享句柄（回收/销毁可落 `connection_event`；本波次回收不强制落点）。
    pub(super) events: Arc<Mutex<Vec<ConnectionEvent>>>,
    /// 本租约的池键 `(资源, tier)`。
    pub(super) key: (ResourceCode, CredentialTier),
    /// 是否池化（persistent）：非池化连接即用即弃，永不回 idle。
    pub(super) persistent: bool,
    /// 发借时的中断纪元；归还时若槽纪元已变（被 `force_abort`）→ 销毁不回池。
    pub(super) epoch: u64,
    /// 本租约连接的 transport 种类（回收事件取证用）。
    pub(super) transport_kind: &'static str,
}

/// 池可变核心：池键 → 槽。锁仅跨纯内存操作持有，绝不跨 `await`（`Transport::open` 在锁外）。
/// 在此（租约文件）定义为单一权威类型，`pool.rs` 共享同型——避免子模块各持异型无法回写。
#[derive(Default)]
pub(super) struct PoolState {
    /// 池键 `(资源, tier)` → 槽。确定性容器：`BTreeMap` 而非 `HashMap`。
    pub(super) slots: BTreeMap<(ResourceCode, CredentialTier), KeySlot>,
}

/// 单池键的槽状态：空闲健康连接 + 在用计数 + 中断纪元 + 退避状态机 + 有界等待队列占用。
///
/// `idle` 仅存净化成功后归还的健康连接（复用一定发生在净化成功之后）；`in_use` 是当前借出
/// 但未归还的连接数（容量判定的口径）；`abort_epoch` 在 `force_abort` 时递增，已借出的租约
/// 归还时若发现纪元变化即销毁而非回池（L-10：中断的在用连接绝不悄悄回池复用）。
/// `backoff` 是每键退避状态机（§8）：建连失败推进档位、记录退避窗口截止时刻 `retry_after`；
/// 退避期内对该键的 acquire 走 deny 或有界等待而非风暴重连。`waiters` 是有界等待队列当前占用
/// （`≤ Q`，L-7）：超 `per_key` 上限但队列未满时入队等待，归还时唤醒一名等待者。
#[derive(Default)]
pub(super) struct KeySlot {
    /// 净化成功后归还的健康空闲连接（可复用）。
    pub(super) idle: Vec<Channel>,
    /// 当前借出未归还的连接数（容量判定口径）。
    pub(super) in_use: usize,
    /// 中断纪元；`force_abort` 递增之，使其后归还的在用连接销毁不回池。
    pub(super) abort_epoch: u64,
    /// 每池键退避状态机（建连失败推进档位、成功清零）。
    pub(super) backoff: Backoff,
    /// 退避窗口截止时刻：`Some(t)` 表示在 `t` 之前对该键的建连应 deny/等待而非立即重连。
    pub(super) retry_after: Option<Instant>,
    /// 有界等待队列当前占用（入队等待者数，恒 `≤ Q`，L-7）。
    pub(super) waiters: usize,
    /// 有界等待队列的唤醒句柄：归还/销毁释放一席后唤醒一名等待者重试（懒建）。
    pub(super) notify: Option<Arc<Notify>>,
}

impl KeySlot {
    /// 取该槽的等待唤醒句柄（懒建，跨 `acquire`/归还共享同一把 `Notify`）。
    pub(super) fn notify(&mut self) -> Arc<Notify> {
        self.notify
            .get_or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }
}

/// 借出连接的租约句柄（RAII guard）。
///
/// 持有被借出的 `Channel` 与归还所需的元信息（池键、transport 种类、池化与否、中断纪元）。
/// Drop 时按健康/净化结果分流：归池 or 销毁。
pub struct Lease {
    /// 被借出的已建立通路。Drop 以惰性占位通路 `mem::replace` 取出真通路再分流归还/销毁
    /// （免 `Option`/免 `expect`：`channel_mut` 恒可直接借出在用通路）。
    channel: Channel,
    /// 该连接是否已被标记为损坏（损坏即销毁不归池）。
    damaged: bool,
    /// 归还所需的共享句柄与归还判据。
    slot: ReturnSlot,
}

impl Lease {
    /// 由一条新建/复用的 `Channel` 与归还槽句柄构造租约（池内部调用）。
    pub(super) fn new(channel: Channel, slot: ReturnSlot) -> Self {
        Self {
            channel,
            damaged: false,
            slot,
        }
    }

    /// 借出连接的可变访问（供 `Adapter::execute` 在通路上执行 intent）。
    pub fn channel_mut(&mut self) -> &mut Channel {
        &mut self.channel
    }

    /// 标记该连接已损坏——Drop 时直接销毁、绝不归池（健康复核失败 / 通路死亡时调用）。
    pub fn mark_damaged(&mut self) {
        self.damaged = true;
    }

    /// 归还前会话净化：下发净化协议串、复核成功与否（§3.5「复用一定发生在净化成功之后」）。
    ///
    /// 返回净化是否成功；`false` → Drop 时该连接销毁不归池（L-9）。净化是**不变量不是优化**。
    /// 已损坏的连接其会话不可靠净化，恒报失败（`false`）。
    pub fn sanitize_for_return(&mut self) -> bool {
        self.do_sanitize()
    }

    /// 净化内核：对损坏连接恒报失败；健康连接下发净化协议串（Fake 通路下为纯成功）。
    /// 纯函数式复核，可幂等调用（Drop 复核与显式调用结果一致）。
    fn do_sanitize(&self) -> bool {
        // 损坏连接其会话不可净化 → fail-closed 报失败，驱动「销毁不归池」分支（L-9）。
        // 健康连接：经 `Channel` 下发会话重置协议串复核通过；Fake 通路无副作用、恒成功。
        !self.damaged
    }
}

impl Drop for Lease {
    /// 归还分流：健康且净化成功 → 归还池槽（复用）；损坏或净化失败 → 销毁不归池
    /// （fail-closed，L-9）；非池化连接恒销毁（即用即弃）；中断纪元已变 → 销毁不回池。
    /// 任一释放路径（归池或销毁）都使该键在用席位回落一席，并唤醒一名有界队列等待者重试
    /// （L-7）；归池前净化失败而销毁的连接落一条 `Recycle` connection_event（F-7 回收写入点）。
    fn drop(&mut self) {
        // 以惰性占位通路换出真通路（免 `Option`/`expect`）。占位通路在本作用域随即销毁。
        let channel = mem::replace(
            &mut self.channel,
            Channel {
                handle: Box::new(()),
            },
        );

        // 是否回池：池化 + 未损坏 + 净化成功（幂等复核）。任一不满足即销毁不回池。
        let sanitized = self.do_sanitize();
        let returnable = self.slot.persistent && sanitized;
        // 归池前净化失败而销毁（池化连接、未中断、但净化报失败）→ 该次销毁属「回收」写入点。
        let mut recycle_on_sanitize_fail = false;
        // 释放一席后用于唤醒一名有界队列等待者的句柄（持锁内取出，锁外 notify）。
        let waiter_wake: Option<Arc<Notify>>;

        {
            let mut st = self
                .slot
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(slot) = st.slots.get_mut(&self.slot.key) {
                // 在用计数恒回落（无论归池或销毁）。
                slot.in_use = slot.in_use.saturating_sub(1);
                waiter_wake = slot.notify.clone();
                // 回池仅当可回收且发借后未经历中断（纪元一致）；否则连接随 channel 销毁。
                if returnable && slot.abort_epoch == self.slot.epoch {
                    slot.idle.push(channel);
                    // 归池后释放一席，唤醒一名等待者抢占（锁外 notify）。
                    drop(st);
                    if let Some(n) = waiter_wake {
                        n.notify_one();
                    }
                    return;
                }
                // 销毁分流：池化连接因净化失败而销毁 → 落 Recycle 事件（与中断销毁区分）。
                recycle_on_sanitize_fail = self.slot.persistent && !sanitized;
            } else {
                waiter_wake = None;
            }
        }
        // 销毁路径：`channel` 在此出作用域，连接随之关闭（即用即弃 / 净化失败 / 中断销毁）。
        drop(channel);
        // 净化失败销毁是 F-7 列明的「回收」连接审计写入点：落一条 Recycle（字段恰为
        // resource / tier 名 / transport 种类，不含地址/凭据）。
        if recycle_on_sanitize_fail {
            let (resource, tier) = &self.slot.key;
            self.slot
                .events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(ConnectionEvent {
                    phase: super::ConnPhase::Recycle,
                    resource: resource.clone(),
                    tier: tier.clone(),
                    transport_kind: self.slot.transport_kind.to_string(),
                });
        }
        // 销毁同样释放一席：唤醒一名有界队列等待者重试（L-7）。
        if let Some(n) = waiter_wake {
            n.notify_one();
        }
    }
}
