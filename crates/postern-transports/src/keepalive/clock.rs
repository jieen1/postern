//! 可注入时钟抽象。
//!
//! 把保活状态机对「当前时间 / 到期点 / 节律推进」的依赖收敛到一个可替换的时钟接口，
//! 使续约阈值（`expiry − skew`）触发、心跳节律均可在测试中由**注入时钟**确定性驱动
//! （§9：注入时钟 + 桩远端做行为观察，对齐 §8 的 F-2/F-3/L-4）。生产实现绑定
//! tokio time，测试实现可手推时间，二者对状态机等价。
//!
//! **时间模型（确定可复现，§9）**：本域用「自通路建立原点起经过的逻辑时长」
//! （[`Instant`] = 单调 `Duration`）表达「现在」，而非墙钟绝对时刻。到期点
//! `expiry` 同样落在这条逻辑时间轴上，由底层「保活后端」端口在建立 / 续约时给出
//! （§3.3：`expiry` 不是本域常量）。状态机只问时钟「现在是几」、并据后端给出的
//! `expiry` 与本域的 `skew` 算出续约触发点；时钟可注入即让测试**手推**逻辑时间
//! 越过 / 不越过阈值，确定性地驱动 F-2 / F-3 / L-4。
//!
//! **纪律**：本接口只读「现在」，**不**提供任何 `sleep` / 真实墙钟等待入口——
//! 测试绝不用 `tokio::time::sleep` 真实墙钟跑（否则不确定、慢）；推进节律靠
//! 注入时钟手推逻辑时间。生产实现（tokio time 绑定）由集成层提供，本骨架只钉
//! trait 与可手推的 [`FakeClock`]，二者对状态机等价。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 逻辑时间轴上的一个时刻：自通路建立原点起经过的单调时长（§9 时间模型）。
///
/// 用 `Duration`（而非 `std::time::Instant`）表达「现在 / 到期点」，使整条时间轴
/// 在测试中可被注入时钟手推、确定可复现。原点（建立时刻）记为 `Instant(0)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(pub Duration);

impl Instant {
    /// 逻辑时间轴原点（通路建立时刻），等价 `Instant(Duration::ZERO)`。
    pub const ORIGIN: Instant = Instant(Duration::ZERO);

    /// 在本时刻之上叠加一段时长，得到未来时刻（用于据 `expiry`/`skew` 算触发点）。
    pub fn saturating_add(self, delta: Duration) -> Instant {
        Instant(self.0.saturating_add(delta))
    }

    /// 在本时刻之上回退一段时长（饱和到原点），用于据 `expiry − skew` 算续约触发点。
    ///
    /// 「宁可早续不可晚续」：续约触发点 = `expiry − skew`，留足续约往返提前量
    /// （§3.3 提前量取舍）。`skew` 大于 `expiry` 时饱和到原点（即建立即到触发点）。
    pub fn saturating_sub(self, delta: Duration) -> Instant {
        Instant(self.0.saturating_sub(delta))
    }
}

/// 可注入时钟接口（§9）：状态机对「现在是几」的唯一依赖点。
///
/// 只读「现在」——**无** `sleep` / 墙钟等待入口（推进节律靠手推逻辑时间，测试
/// 绝不真实墙钟跑）。生产实现绑定 tokio time（由集成层提供），测试用 [`FakeClock`]
/// 手推逻辑时间；二者对状态机等价。
pub trait Clock: Send + Sync {
    /// 读取当前逻辑时刻（自通路建立原点起经过的单调时长）。
    fn now(&self) -> Instant;
}

/// 可手推的测试时钟（§9）：内部持自原点起经过的纳秒计数，测试调
/// [`FakeClock::advance`] 手推逻辑时间越过 / 不越过续约阈值，确定性驱动状态机。
///
/// 可 `Clone`（共享同一底座原子），便于状态机持一个句柄、测试侧持另一个句柄推进。
/// **绝不**绑定真实墙钟——`now()` 只回放手推累积量。
#[derive(Clone, Default)]
pub struct FakeClock {
    /// 自原点起经过的纳秒数；`advance` 累加、`now` 读取。`Arc<Atomic>` 跨句柄共享。
    elapsed_nanos: Arc<AtomicU64>,
}

impl FakeClock {
    /// 新建一个停在原点（`Instant::ORIGIN`）的测试时钟。
    pub fn new() -> Self {
        Self {
            elapsed_nanos: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 手推逻辑时间前进 `delta`（单调，绝不回退）。测试据此越过 / 不越过续约阈值。
    pub fn advance(&self, delta: Duration) {
        // 单调累加纳秒：饱和处理避免溢出（绝不回退、绝不读真实墙钟）。
        let delta_nanos = u64::try_from(delta.as_nanos()).unwrap_or(u64::MAX);
        self.elapsed_nanos.fetch_add(delta_nanos, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    /// 回放手推累积的逻辑时刻（自原点起经过的时长）。绝不读真实墙钟。
    fn now(&self) -> Instant {
        Instant(Duration::from_nanos(
            self.elapsed_nanos.load(Ordering::SeqCst),
        ))
    }
}
