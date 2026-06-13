//! 连接池本体：按 (ResourceCode, CredentialTier) 池键管理通道生命周期（§3.5）。
//!
//! 获取时优先复用健康的空闲连接，否则在上限内新建；归池前施加会话净化，净化失败即销毁
//! 该连接（绝不带脏会话回池，L-9）。tier 不共享：不同凭据档位走互不相通的子池（L-8）。
//! 新建连接经 `Transport::open`，凭据为上游解析出的不透明句柄按值传入，本层不构造机密类型
//! （F-7 / L-17）。超并发上限走有界队列或 deny（L-7），断连按每键指数退避重连。
//!
//! 连接审计（`connection_event`）落在池自持的内存事件序上（`recorded_events`）——本层**绝不**
//! 组装 `AuditEvent`（其 `origin: ConnOrigin` 只能在 shells 构造，本层无权构造），只读
//! `ConnPhase` / 写 `ConnectionEvent`。注入的 `AuditSink` 句柄由 boot 持有以备全局审计编排，
//! 本子域不经它落 connection_event（避开 `ConnOrigin` 构造红线）。

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use postern_core::domain::{CredentialTier, ResolvedTarget, ResourceCode};
use postern_core::plugin::{AuditSink, Channel, CredentialProvider};
use postern_secrets::error::ResolveError;

use super::lease::{Lease, PoolState, ReturnSlot};
use super::{AcquireError, ConnPhase, ConnectionEvent};
use crate::registry::TransportRegistry;

/// 代号→真实地址解析的注入点（daemon 侧抽象，挂在机密面 `UnlockedVault::resolve` 上）。
///
/// connpool 经此一次性取**不透明** `ResolvedTarget` 句柄，按值传入 `Transport::open`；
/// 本层**绝不构造** `ResolvedTarget`（构造权在 secrets，§4 边界）。解析失败 fail-closed
/// 折叠为建连失败（[`AcquireError::Resolve`]），不降级、不改路。
pub trait TargetResolver: Send + Sync {
    /// 解析资源代号为不透明真实地址句柄；未知代号 / 保险箱不可用即 `Err`。
    fn resolve(&self, code: &ResourceCode) -> Result<ResolvedTarget, ResolveError>;
}

/// 连接池容量边界（每键并发上限 + 全局上限 + 有界等待队列上限 `Q`），**常量封顶**（L-7）。
///
/// 超限的请求落入容量 `Q` 的有界队列等待，触顶即背压或 `deny, stage=connect`——
/// 缓冲峰值 `≤ Q`、与灌注量无关，杜绝无界缓冲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolCaps {
    /// 每池键（每 `(资源, tier)`）并发上限 `N`。
    pub per_key: usize,
    /// 全局并发上限。
    pub global: usize,
    /// 有界等待队列容量 `Q`（占用峰值上界）。
    pub queue: usize,
}

/// 连接池：按 `(ResourceCode, CredentialTier)` 池键治理通路生命周期。
///
/// 依赖注入集合（§5 / L-2）：传输登记册（按 `kind()` 选型）、`CredentialProvider`（一次性
/// 物化凭据句柄）、`TargetResolver`（一次性解析真实地址句柄）、`AuditSink`（落
/// `connection_event`）、容量边界。**不含** `PolicyRepo` 与 vault 写句柄——本层只取不透明
/// 句柄、即用即释（F-7 / L-17）。
pub struct ConnPool {
    /// 传输登记册：按 `Transport::kind()` 定位通路建立 / 保活 / 关闭的物理执行者。
    transports: Arc<TransportRegistry>,
    /// 凭据来源：`(res, tier)` 一次性物化不透明 `ResourceCredential`（按值入 open）。
    credentials: Arc<dyn CredentialProvider>,
    /// 目标解析：代号一次性解析不透明 `ResolvedTarget`（按值入 open）。
    resolver: Arc<dyn TargetResolver>,
    /// 全局审计落点句柄（boot 注入；本子域不经它落 connection_event，见模块注释）。
    audit: Arc<dyn AuditSink>,
    /// 容量边界（每键 / 全局 / 队列上限）。
    caps: PoolCaps,
    /// 池可变核心（池键 → 槽）。租约归还/销毁经其共享句柄回写槽。
    state: Arc<Mutex<PoolState>>,
    /// 连接事件序（`connection_event` 取证落点；建连 / 中断 / 回收各落一条）。
    events: Arc<Mutex<Vec<ConnectionEvent>>>,
}

impl ConnPool {
    /// 装配连接池（注入传输登记册 / 凭据来源 / 目标解析 / 审计落点 / 容量边界）。
    ///
    /// 构造无 IO；池槽并发表按需懒建。注入集合**不含**任何写路径与机密写句柄（L-2）。
    pub fn new(
        transports: Arc<TransportRegistry>,
        credentials: Arc<dyn CredentialProvider>,
        resolver: Arc<dyn TargetResolver>,
        audit: Arc<dyn AuditSink>,
        caps: PoolCaps,
    ) -> Self {
        Self {
            transports,
            credentials,
            resolver,
            audit,
            caps,
            state: Arc::new(Mutex::new(PoolState::default())),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 按池键 `(resource, tier)` 获取一条可用连接的租约（§3.5 acquire 数据流）。
    ///
    /// **入参即已选定 tier**——本调用栈内**无任何动词→tier 映射**、不读 `Capability` 决定
    /// tier（L-17）；tier 的产出唯一在 `Evaluator::evaluate` 的 allow 路径。
    ///
    /// 数据流：定位池键 `(ResourceCode, CredentialTier)` 的池槽 → 命中空闲且健康的
    /// `Channel` 即出借（复用，不重建）→ 无空闲且未达上限 → 向机密面**一次性**取
    /// `(ResolvedTarget, ResourceCredential)` 不透明句柄、**即时**按值传入 `Transport::open`
    /// 得新 `Channel`（句柄不出本次调用边界，调用一返回即释放、不入池不缓存）→ 达上限 →
    /// 进有界等待队列或 `Err`（fail-closed）。
    ///
    /// 失败折叠（L-6）：凭据 / 解析 / 通路建立失败 → `Err(AcquireError)`（`stage()` 恒为
    /// `Stage::Transport`），错误经脱敏、不含真实地址；绝不静默重试到他路或降级放行。
    pub async fn acquire(
        &self,
        resource: &ResourceCode,
        tier: &CredentialTier,
    ) -> Result<Lease, AcquireError> {
        let key = (resource.clone(), tier.clone());

        // 选 transport：登记册未命中即 fail-closed（无兜底他路）。读 persistent 决定是否池化。
        let transport = self
            .transports
            .transport_for(TRANSPORT_KIND_DIRECT)
            .ok_or(AcquireError::NoTransport)?;
        let persistent = transport.persistent();

        // —— 阶段一：纯内存判定（持锁，绝不跨 await）。复用命中即直接出借；超限走有界队列
        // 等待或立即 deny；退避期内 deny 不重连。等待者在锁外挂 `Notify`，被唤醒后重判，
        // 占用恒 `≤ Q`（L-7）。本调用栈内是否曾入队记于 `queued`，退出时回落占用。
        let mut queued = false;
        loop {
            let waiter;
            {
                let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                let slot = st.slots.entry(key.clone()).or_default();

                if persistent {
                    if let Some(channel) = slot.idle.pop() {
                        // 命中空闲且健康连接 → 复用（不重建）。出借即占用一席。
                        slot.in_use += 1;
                        if queued {
                            slot.waiters = slot.waiters.saturating_sub(1);
                        }
                        let epoch = slot.abort_epoch;
                        return Ok(self.make_lease(channel, key, persistent, epoch));
                    }
                }

                // 退避窗口判定：上次建连死亡后处于指数退避期内 → deny，绝不立即风暴重连（§8）。
                if let Some(t) = slot.retry_after {
                    if Instant::now() < t {
                        if queued {
                            slot.waiters = slot.waiters.saturating_sub(1);
                        }
                        return Err(AcquireError::BackoffActive);
                    }
                    // 退避窗口已到期：清窗口，允许本次重连尝试。
                    slot.retry_after = None;
                }

                // 容量判定（口径 = 该键在用数）。未达上限 → 占席建连。
                if slot.in_use < self.caps.per_key {
                    slot.in_use += 1;
                    if queued {
                        slot.waiters = slot.waiters.saturating_sub(1);
                    }
                    break;
                }

                // 达上限：已在队列里的等待者直接挂起等下一次唤醒（不重复占额）。
                if queued {
                    waiter = slot.notify();
                } else if self.caps.queue == 0 || slot.waiters >= self.caps.queue {
                    // 无等待位（queue=0）或有界队列已满（占用已达 Q）→ 立即 deny，绝不无界缓冲。
                    return Err(AcquireError::CapacityExceeded);
                } else {
                    // 入有界等待队列：占用一席（恒 `≤ Q`），挂 `Notify` 待释放后唤醒重判。
                    slot.waiters += 1;
                    queued = true;
                    waiter = slot.notify();
                }
            }
            // 锁外等待：被归还/销毁释放一席时唤醒，回环重判（占用 `≤ Q`，绝不无界缓冲）。
            waiter.notified().await;
        }

        // —— 阶段二：建连（在锁外，绝不持锁跨 await）。一次性取不透明句柄，即时按值入 open。——
        let cred = match self.credentials.credential_for(resource, tier).await {
            Ok(c) => c,
            Err(_) => {
                self.release_reservation(&key);
                return Err(AcquireError::Credential);
            }
        };
        let target = match self.resolver.resolve(resource) {
            Ok(t) => t,
            Err(_) => {
                self.release_reservation(&key);
                return Err(AcquireError::Resolve);
            }
        };

        // 句柄一次性、即时按值移入 `open`，不出本次调用边界（不入池、不缓存，F-7 / L-17）。
        let channel = match transport.open(target, cred).await {
            Ok(ch) => ch,
            Err(_) => {
                // 通路死亡：推进该键退避档位并记录退避窗口截止时刻——窗口内对该键的 acquire
                // 走 deny（`BackoffActive`）而非立即重连（§8 健康与退避状态机，绝不风暴重连）。
                self.release_with_backoff(&key);
                return Err(AcquireError::Transport);
            }
        };

        // 建连成功：清退避档位与窗口（下次失败重新从基数起退），落 establish 事件。
        {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(slot) = st.slots.get_mut(&key) {
                slot.backoff.reset();
                slot.retry_after = None;
            }
        }
        // 建连成功：落 establish 事件（字段恰为 resource / tier 名 / transport 种类）。
        self.record_event(ConnPhase::Establish, resource, tier, transport.kind());

        let epoch = {
            let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            st.slots.get(&key).map(|s| s.abort_epoch).unwrap_or(0)
        };
        Ok(self.make_lease(channel, key, persistent, epoch))
    }

    /// freeze / 吊销时对相关 `(resource[, principal])` 在用连接**强制 abort/cancel**
    /// （取消底层查询、关闭隧道；物理执行归传输、决策归本层），落 `connection_event`
    /// （phase=abort）——非仅优雅排空（L-10）。
    pub async fn force_abort(&self, resource: &ResourceCode) {
        // 收集匹配该资源（所有 tier）的在用连接，逐条记 abort 事件，并递增其槽的中断纪元——
        // 使这些在用连接归还时销毁不回池（中断不悄悄复用）。空闲连接同样从池中剔除。
        let mut aborted: Vec<CredentialTier> = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for ((res, tier), slot) in st.slots.iter_mut() {
                if res == resource {
                    let count = slot.in_use + slot.idle.len();
                    slot.idle.clear();
                    if count > 0 {
                        slot.abort_epoch += 1;
                    }
                    for _ in 0..count {
                        aborted.push(tier.clone());
                    }
                }
            }
        }
        for tier in &aborted {
            self.record_event(ConnPhase::Abort, resource, tier, TRANSPORT_KIND_DIRECT);
        }
    }

    /// 周期健康检查判定通路死亡 → 从池槽**剔除**该键的空闲死连接，逐条落
    /// `connection_event`（phase=health-evict），并推进退避档位与退避窗口（死亡后不立即重连，
    /// §8）。传输层只上报通路死亡事实、绝不自行重建——重建决策与节奏归本层（§3.5）。
    ///
    /// 剔除只动空闲连接（在用连接的中断走 `force_abort`）；剔除后该键无空闲可复用，下次
    /// acquire 在退避窗口外重建。
    pub fn health_evict(&self, resource: &ResourceCode) {
        let mut evicted: Vec<CredentialTier> = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for ((res, tier), slot) in st.slots.iter_mut() {
                if res == resource && !slot.idle.is_empty() {
                    let count = slot.idle.len();
                    slot.idle.clear();
                    slot.backoff.record_failure();
                    if let Some(delay) = slot.backoff.next_delay() {
                        slot.retry_after = Some(Instant::now() + delay);
                    }
                    for _ in 0..count {
                        evicted.push(tier.clone());
                    }
                }
            }
        }
        for tier in &evicted {
            self.record_event(
                ConnPhase::HealthEvict,
                resource,
                tier,
                TRANSPORT_KIND_DIRECT,
            );
        }
    }

    /// 读取已落审计的连接事件序（测试 / 观测取证；生产经 `AuditSink` 落库）。
    pub fn recorded_events(&self) -> Vec<ConnectionEvent> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 构造一条租约：把归还所需的共享句柄（槽状态、事件序）打包进租约的 RAII 归还闭包。
    fn make_lease(
        &self,
        channel: Channel,
        key: (ResourceCode, CredentialTier),
        persistent: bool,
        epoch: u64,
    ) -> Lease {
        let slot = ReturnSlot {
            state: self.state.clone(),
            events: self.events.clone(),
            key,
            persistent,
            epoch,
            transport_kind: TRANSPORT_KIND_DIRECT,
        };
        Lease::new(channel, slot)
    }

    /// 建连失败回滚：释放阶段一占下的席位（in_use--），不留泄漏占用，并唤醒一名有界队列
    /// 等待者重判（席位已回落，L-7）。
    fn release_reservation(&self, key: &(ResourceCode, CredentialTier)) {
        let waiter = {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match st.slots.get_mut(key) {
                Some(slot) => {
                    slot.in_use = slot.in_use.saturating_sub(1);
                    slot.notify.clone()
                }
                None => None,
            }
        };
        if let Some(n) = waiter {
            n.notify_one();
        }
    }

    /// 通路死亡回滚：释放阶段一占下的席位（in_use--），并推进该键退避档位、记录退避窗口
    /// 截止时刻（`record_failure` + `next_delay` → `retry_after = now + delay`）——窗口内对该键
    /// 的 acquire 走 deny 而非立即风暴重连（§8）。同样唤醒一名等待者（席位已回落）。
    fn release_with_backoff(&self, key: &(ResourceCode, CredentialTier)) {
        let waiter = {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match st.slots.get_mut(key) {
                Some(slot) => {
                    slot.in_use = slot.in_use.saturating_sub(1);
                    slot.backoff.record_failure();
                    if let Some(delay) = slot.backoff.next_delay() {
                        slot.retry_after = Some(Instant::now() + delay);
                    }
                    slot.notify.clone()
                }
                None => None,
            }
        };
        if let Some(n) = waiter {
            n.notify_one();
        }
    }

    /// 追加一条 `connection_event`（字段恰为 resource / tier 名 / transport 种类）。
    fn record_event(
        &self,
        phase: ConnPhase,
        resource: &ResourceCode,
        tier: &CredentialTier,
        transport_kind: &str,
    ) {
        let _ = &self.audit; // 全局审计句柄由 boot 持有；connection_event 不经其落（见模块注释）。
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ConnectionEvent {
                phase,
                resource: resource.clone(),
                tier: tier.clone(),
                transport_kind: transport_kind.to_string(),
            });
    }
}

/// 直连传输的形态键（本波次单测只装配 `direct` 一种 transport）。
const TRANSPORT_KIND_DIRECT: &str = "direct";
