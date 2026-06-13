//! 心跳 / 续约状态机。
//!
//! 心跳成功 / 续约成功 → 维持「活」并（续约成功时）按远端返回的新到期点刷新
//! `expiry`；续约失败 / 心跳判定僵死 → 一次性转「死亡」写入健康视图，就地停摆。
//! 续约提前量取舍：宁可早续不可晚续（晚续即通路死亡，fail-closed，§3.3）。
//! 无「失败后重建」转移边、无接口层重建入口（§7-3）。
//!
//! **本单元的边界（雷区收口）**：
//! - 状态机里**根本不存在**「失败后重建」转移边——无重连 / 重建 / 故障切换 / 重试 /
//!   退避任何符号、无退避器、无新建连接尝试入口（§3.3 / §7-3 / L-2/L-3）。
//!   续约 / 心跳失败的唯一出口是 [`KeepaliveOutcome::Died`]：一次性转「死亡」、就地停摆。
//! - `expiry` **不是**本域配置常量：建立 / 续约的到期点一律由注入的 [`KeepaliveBackend`]
//!   端口给出（[`Renewal::new_expiry`]），本域只按其驱动续约节律，不臆造时限（§3.3）。
//! - 心跳 / 续约的底层动作经 [`KeepaliveBackend`] 端口注入，**绝不**在本单元直连真实
//!   ssh/ssm/aws-sdk（那是 ssh/ssm 单元 feature-gated 实现要满足的端口，§9）。
//! - 本单元不持机密、不知道远端真实地址：端口不接触 `ResolvedTarget`/`ResourceCredential`。
//! - 失败显式转「死亡」、**不** `.ok()` 吞错、**不** unwrap/expect/panic（fail-closed：
//!   晚续即死亡，公理二）。

use std::time::Duration;

use async_trait::async_trait;

use super::clock::{Clock, Instant};
use crate::health::{Health, HealthWriter};

/// 一次成功续约的结果（§3.3）：远端给出的**新到期点**。
///
/// 关键纪律：`new_expiry` 由 [`KeepaliveBackend::renew`] 返回（来自远端 / 底层会话），
/// **不是**本域常量——续约成功后状态机据此刷新本地 `expiry`，下一次续约触发点
/// （`new_expiry − skew`）随之前移到新到期点附近，证明时限来自远端而非本域臆造。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Renewal {
    /// 远端给出的新到期点（逻辑时间轴上的时刻，§9）。
    pub new_expiry: Instant,
}

/// 心跳探测结果（§3.3）：通路是否仍被远端判定为「活」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heartbeat {
    /// 心跳应答正常：通路仍活。
    Alive,
    /// 心跳判定僵死（探测超时 / 应答异常）：本域据此转「死亡」。
    Dead,
}

/// 保活后端端口（§3.3 / §9）：心跳 / 续约底层动作的**抽象注入点**。
///
/// 生产侧由 ssh/ssm 单元的 feature-gated 实现满足（SSH keepalive 报文 / SSM 会话续期 /
/// 租约续租）；测试侧用**记录次数的桩后端**（成功桩 / 失败桩）做行为观察（§9）。
///
/// 端口**不接触机密**：方法不收 `ResolvedTarget`/`ResourceCredential`，本域不知道远端
/// 真实地址（§7-1/-8、雷区）。`renew` 成功必返回远端给出的**新 `expiry`**（[`Renewal`]），
/// 失败返回 `Err`——失败即触发状态机转「死亡」（L-4），端口**无**重连 / 新建连接入口。
#[async_trait]
pub trait KeepaliveBackend: Send + Sync {
    /// 对有时限通路发起一次**协议级续约**（SSM 会话续期 / 租约续租）。
    ///
    /// 成功返回远端给出的新到期点 [`Renewal`]；失败返回 `Err(())`——失败一律由状态机
    /// 转「死亡」、就地停摆，端口**绝不**自行新建连接 / 重连（§3.3 / L-3/L-4）。
    async fn renew(&self) -> Result<Renewal, ()>;

    /// 对长连接通路发起一次**协议级心跳探测**（SSH keepalive 报文 / 探测包）。
    ///
    /// 返回 [`Heartbeat::Alive`] / [`Heartbeat::Dead`]；判定僵死即由状态机转「死亡」。
    async fn heartbeat(&self) -> Heartbeat;
}

/// 一次状态机推进（tick）后的结果（§3.3 状态机出口）。
///
/// 仅两种出口——维持「活」或一次性转「死亡」终态。**根本不存在**「失败后重建」出口：
/// 无 `Reconnect`/`Rebuilt`/`Failover` 变体（§7-3 / L-3 核心纪律）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepaliveOutcome {
    /// 本次推进未到任何阈值：无心跳 / 续约动作被发起，维持「活」。
    Idle,
    /// 本次推进触发并完成了一次成功续约：刷新 `expiry`，维持「活」。
    Renewed,
    /// 本次推进触发并完成了一次成功心跳：维持「活」。
    Probed,
    /// 续约失败 / 心跳判定僵死：状态机一次性转「死亡」、就地停摆（终态，无重建边）。
    Died,
}

/// 长连接型保活状态机（§3.3）：绑定单条已建通路生命周期，以「活」为初态。
///
/// 两类手段经 [`KeepaliveBackend`] 端口发起：①心跳（固定节律探测）；②协议级续约
/// （`expiry − skew` 触发）。时间依赖收敛到注入的 [`Clock`]（测试手推、确定可复现）。
/// 死活事实经 [`HealthWriter`] 单向写入健康视图（续约失败 / 心跳僵死 → 一次性 `Dead`）。
///
/// **不变量**：`persistent == false`（非长连接）→ 状态机立即空转，永不发起心跳 / 续约
/// （F-3）；一旦转「死亡」即终态停摆，继续推进时钟绝不翻回「活」、绝不新建连接（L-3）。
/// 本结构体**无**任何重试 / 退避 / 重连字段（雷区：无退避器 / 无重试计数器）。
pub struct Keepalive<C: Clock, B: KeepaliveBackend> {
    clock: C,
    backend: B,
    health: HealthWriter,
    /// 是否长连接型：`false` 则状态机立即空转、永不保活（F-3，§3 第 3 项）。
    persistent: bool,
    /// 当前到期点：建立时由后端给出，续约成功后按远端新到期点刷新（**非本域常量**）。
    expiry: Instant,
    /// 续约提前量：续约触发点 = `expiry − skew`（宁可早续不可晚续，§3.3）。
    skew: Duration,
    /// 心跳节律：相邻两次心跳探测的固定间隔（§3.3）。
    heartbeat_interval: Duration,
    /// 上次成功续约的逻辑时刻（建立时为 `None`）。续约本身即证明活性，故紧随
    /// 续约之后的那一个心跳节律点冗余、不再探测——本字段用于钉死该「续约后一拍
    /// 跳过心跳」，避免在续约点附近重复探测。**不是**任何重试 / 退避计数。
    last_renew_at: Option<Instant>,
}

impl<C: Clock, B: KeepaliveBackend> Keepalive<C, B> {
    /// 新建一条**长连接型**保活状态机（`persistent == true`），以「活」为初态。
    ///
    /// `expiry` 为底层会话 / 租约建立时**由后端给出**的初始到期点（非本域常量，§3.3）；
    /// `skew` 为续约提前量（续约触发点 = `expiry − skew`）；`heartbeat_interval` 为心跳节律。
    pub fn persistent(
        clock: C,
        backend: B,
        health: HealthWriter,
        expiry: Instant,
        skew: Duration,
        heartbeat_interval: Duration,
    ) -> Self {
        Self {
            clock,
            backend,
            health,
            persistent: true,
            expiry,
            skew,
            heartbeat_interval,
            last_renew_at: None,
        }
    }

    /// 新建一条**非长连接型**保活状态机（`persistent == false`）：状态机立即空转，
    /// 永不发起心跳 / 续约（F-3，§3 第 3 项）。`expiry` / `skew` 仅占位、不被驱动。
    pub fn ephemeral(
        clock: C,
        backend: B,
        health: HealthWriter,
        expiry: Instant,
        skew: Duration,
        heartbeat_interval: Duration,
    ) -> Self {
        Self {
            clock,
            backend,
            health,
            persistent: false,
            expiry,
            skew,
            heartbeat_interval,
            last_renew_at: None,
        }
    }

    /// 当前续约触发点（§3.3）：`expiry − skew`。读取注入时钟到达 / 越过此点即触发续约。
    pub fn renew_at(&self) -> Instant {
        self.expiry.saturating_sub(self.skew)
    }

    /// 当前到期点（建立时 / 上次续约成功后由后端给出，**非本域常量**）。
    pub fn expiry(&self) -> Instant {
        self.expiry
    }

    /// 推进一次状态机（tick）：读注入时钟的「现在」，据续约阈值 / 心跳节律决定动作。
    ///
    /// 语义（§3.3）：
    /// - `persistent == false` → 立即返回 [`KeepaliveOutcome::Idle`]，**不**触端口（F-3）。
    /// - 已转「死亡」（健康视图非 `Alive`）→ 终态停摆，返回 [`KeepaliveOutcome::Died`]，
    ///   **不**触端口、**绝不**新建连接 / 翻回「活」（L-3）。
    /// - `now >= expiry − skew` → 经端口发起一次续约：成功则按返回的**远端新 `expiry`**
    ///   刷新本地到期点、返回 [`KeepaliveOutcome::Renewed`]（F-2）；失败则 [`HealthWriter`]
    ///   写「死亡」、返回 [`KeepaliveOutcome::Died`]（L-4），**绝不**重连。
    /// - 否则到心跳节律 → 经端口发起一次心跳：[`Heartbeat::Alive`] 返回
    ///   [`KeepaliveOutcome::Probed`]；[`Heartbeat::Dead`] 写「死亡」、返回 `Died`。
    /// - 未到任何阈值 → [`KeepaliveOutcome::Idle`]。
    ///
    /// **绝不** `.ok()` 吞错、**绝不** unwrap/expect/panic（fail-closed：晚续即死亡）。
    pub async fn tick(&mut self) -> KeepaliveOutcome {
        // 非长连接：状态机立即空转，永不触端口（F-3）。
        if !self.persistent {
            return KeepaliveOutcome::Idle;
        }

        // 已转「死亡」即终态停摆：不触端口、绝不新建连接 / 翻回「活」（L-3）。
        // 死活事实唯一真相在健康视图（单调），据此判定是否已终态。
        if self.health.reader().get() != Health::Alive {
            return KeepaliveOutcome::Died;
        }

        let now = self.clock.now();

        // ① 续约阈值优先（`now >= expiry − skew`）：宁可早续不可晚续（§3.3）。
        if now >= self.renew_at() {
            match self.backend.renew().await {
                // 续约成功：按远端给出的**新到期点**刷新本地 `expiry`，维持「活」。
                Ok(renewal) => {
                    self.expiry = renewal.new_expiry;
                    self.last_renew_at = Some(now);
                    KeepaliveOutcome::Renewed
                }
                // 续约失败：一次性写「死亡」、就地停摆，**绝不**新建连接（L-4）。
                Err(()) => {
                    self.health.mark_dead();
                    KeepaliveOutcome::Died
                }
            }
        }
        // ② 否则到心跳节律 → 发心跳探测（尽早发现僵死，§3.3）。
        else if self.heartbeat_due(now) {
            match self.backend.heartbeat().await {
                // 应答为活：维持「活」。
                Heartbeat::Alive => KeepaliveOutcome::Probed,
                // 判定僵死：一次性写「死亡」、就地停摆，**绝不**新建连接（L-4）。
                Heartbeat::Dead => {
                    self.health.mark_dead();
                    KeepaliveOutcome::Died
                }
            }
        }
        // ③ 未到任何阈值：空转，维持「活」。
        else {
            KeepaliveOutcome::Idle
        }
    }

    /// 是否到达心跳节律点：`now` 落在心跳网格（`heartbeat_interval` 的正整数倍）上。
    ///
    /// 续约本身即证明活性，故**紧随上次续约之后的那一个**网格点冗余、跳过——避免
    /// 在续约点附近重复探测。此判定纯由注入时钟的「现在」与固定节律决定，确定可复现。
    fn heartbeat_due(&self, now: Instant) -> bool {
        let interval = self.heartbeat_interval;
        let now_nanos = now.0.as_nanos();
        let interval_nanos = interval.as_nanos();
        // 节律为零或尚在原点：无网格点。
        if interval_nanos == 0 || now_nanos == 0 {
            return false;
        }
        // 不在网格点上（非整数倍）：不发心跳。
        if !now_nanos.is_multiple_of(interval_nanos) {
            return false;
        }
        // 在网格点上：除非该点恰为「上次续约 + 一个节律」（续约后冗余的一拍），否则发心跳。
        match self.last_renew_at {
            Some(last) => now != last.saturating_add(interval),
            None => true,
        }
    }
}
