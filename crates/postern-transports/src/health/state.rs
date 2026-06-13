//! 健康死活事实位与单调状态机内核（设计承诺级桩，函数体 `todo!()`）。
//!
//! 以「活」为初态，可被推进到「僵死 / 已关闭（死亡）」；推进**单调不可逆**
//! （§3.4 末）。两个事实来源（保活判定、底层 I/O 信号）经此内核合流，任一触发
//! 即翻位。提供给连接管理层的是只读快照读取入口，无写回 / 无自愈 / 无重连边
//! （重建是连接管理层据健康事实另行决策，本域状态机无这条转移边，§7-3）。
//!
//! 结构形态（§3.1 健康事实视图 + §3.4 被动呈现）：写半 / 读半分离——pump /
//! keepalive / chan 单元持写半 [`HealthWriter`] 写入死亡 / 关闭事实；连接管理层
//! 持读半 [`HealthReader`] 同步查询当前事实。底座是可跨任务共享的同步原语
//! （`Arc<AtomicU8>` 包装），保持 Layer 0 无运行时任务依赖，供上层任务被动读取。
//!
//! **机密结构保证（§7-8 / L-8）**：[`Health`] 是无字段的纯状态判别枚举，
//! 健康视图类型亦不承载真实地址 / 凭据 / 拓扑标识——类型层即不可表达机密，
//! 不是运行期擦除。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// 通路健康事实位（§3.1 / §3.4）：单调推进的纯状态判别枚举。
///
/// 仅承载「活 / 僵死 / 已关闭」三态，**无任何字段**——不携带 `ResolvedTarget` /
/// `ResourceCredential` / 真实地址 / 拓扑标识（§7-8、L-8 的结构保证）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// 通路活着、可被复用（初态）。
    Alive,
    /// 通路僵死（心跳超时 / 续约失败 / 对端 RST / EOF / 桥接泵退出）。
    Dead,
    /// 通路已按指令关闭（优雅释放或强制 abort/cancel 终态）。
    Closed,
}

impl Health {
    /// 单调序数：`Alive < Dead < Closed`。推进只能非降序，绝不回退
    /// （§3.4 单调不可逆；L-3/L-4）。内核内部用于「绝不翻回活」的比较。
    fn rank(self) -> u8 {
        match self {
            Health::Alive => 0,
            Health::Dead => 1,
            Health::Closed => 2,
        }
    }

    /// 从底座原子的判别字节还原事实位。非法字节即编程错误（fail-closed）。
    fn from_repr(repr: u8) -> Health {
        match repr {
            0 => Health::Alive,
            1 => Health::Dead,
            2 => Health::Closed,
            // 底座字节只由本内核以 `rank()` 写入，越界即不变量被破坏：
            // fail-closed（公理二）——不静默吞掉。
            other => panic!("health repr byte out of range: {other}"),
        }
    }
}

/// 健康事实视图的**写半**（§3.4）：供 pump / keepalive / chan 单元写入死亡 /
/// 关闭事实。只暴露单调推进入口，**无任何「复活」接口**（无 `mark_alive`）——
/// 翻回活只能是连接管理层重建出的**新** `Channel`，与本视图无关（L-3/L-4）。
pub struct HealthWriter {
    cell: Arc<AtomicU8>,
}

/// 健康事实视图的**读半**（§3.4）：供连接管理层**被动读取**当前事实快照的
/// 同步查询入口。本单元内**无任何对 daemon 的调用、无回调注册入口**——是
/// 「连接管理层来读」的被动事实，不是「本域去报」的主动 push（§6.2）。
#[derive(Clone)]
pub struct HealthReader {
    cell: Arc<AtomicU8>,
}

/// 构造一组写半 / 读半分离的健康事实视图，初态为 [`Health::Alive`]（F-4）。
///
/// 底座是可跨任务共享的 `Arc<AtomicU8>`，写半与读半共享同一事实位；本单元
/// 不引入 tokio task/spawn，保持 Layer 0 无运行时任务依赖（供上层任务写入）。
pub fn health_view() -> (HealthWriter, HealthReader) {
    let cell = Arc::new(AtomicU8::new(Health::Alive.rank()));
    let writer = HealthWriter {
        cell: Arc::clone(&cell),
    };
    let reader = HealthReader { cell };
    (writer, reader)
}

impl HealthWriter {
    /// 推进到[僵死]（§3.4 来源 ①②任一触发）。**单调**：若当前已是
    /// [`Health::Closed`] 则不降级；绝不把 [`Health::Dead`] 翻回
    /// [`Health::Alive`]（L-3/L-4）。幂等：已是 `Dead` 再调无副作用。
    pub fn mark_dead(&self) {
        // 单调推进：`fetch_max` 取「当前 rank 与 Dead rank 的较大者」——
        // 已是 Closed（更高 rank）则不降级；已是 Dead 则幂等无副作用；
        // 绝不把 rank 拉回 Alive（L-3/L-4）。
        self.cell.fetch_max(Health::Dead.rank(), Ordering::SeqCst);
    }

    /// 推进到[已关闭]（§3.4 / F-5 关闭终态）。从任意非 `Closed` 态推进到
    /// 终态 [`Health::Closed`]；绝不翻回活（L-3/L-4）。幂等：已 `Closed` 再调
    /// 无副作用。
    pub fn mark_closed(&self) {
        // 终态推进：`fetch_max` 把 rank 拉到 Closed（最高 rank）——从任意
        // 非 Closed 态推进到终态；已是 Closed 则幂等无副作用（L-3/L-4）。
        self.cell.fetch_max(Health::Closed.rank(), Ordering::SeqCst);
    }

    /// 派生一个共享同一事实位的读半（写入侧也可被动读自身当前事实）。
    pub fn reader(&self) -> HealthReader {
        HealthReader {
            cell: Arc::clone(&self.cell),
        }
    }
}

impl HealthReader {
    /// **被动读取**当前健康事实快照（同步、非回调）。本调用不触达 daemon、
    /// 不注册回调——是连接管理层主动来读的事实位（§3.4 / §6.2）。
    pub fn get(&self) -> Health {
        Health::from_repr(self.cell.load(Ordering::SeqCst))
    }
}
