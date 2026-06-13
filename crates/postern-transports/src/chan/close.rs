//! 关闭 / 取消触点（设计承诺级桩，函数体 `todo!()`）。
//!
//! 一个**幂等**的关闭入口，背后绑定底层隧道的取消句柄（§3.5），两条路径：优雅释放
//! （正常收尾、半关后排空）与强制 abort/cancel（紧急切断由连接管理层发起，本域执行
//! 底层隧道取消与后台任务 `JoinHandle::abort`）。关闭后不再触达底层；abort 砍在飞
//! 的搬运也不留孤儿任务。关闭决策在连接管理层、执行在本域（决策者-执行者分离）。
//!
//! **两条语义的差别 = 是否等待在途（§3.5 关键取舍）**：
//! - [`TransportChannelInner::close`] **优雅释放**：有序拆除——停保活任务 → 停桥接泵 →
//!   关底层隧道 → 健康视图转 `Closed`（以桩记录的调用次序断言，§3.5 / F-5）。**幂等**：
//!   重复下达不二次关底层、不报错（once 守卫，§3.5）。
//! - [`TransportChannelInner::abort`] **强制切断**：立即 cancel / abort 泵与保活、cancel 底层
//!   在途，**不**await 在途返回（L-6 砍在飞）；健康转 `Closed`，且在桩慢端点的在途操作返回
//!   **之前**完成（不等其优雅跑完，§3.5 / L-6）。
//!
//! **错误脱敏（§3.5 / L-7）**：优雅 close 执行中底层报错（[`TunnelHandle::close`] 返回 `Err`）
//! 经 [`crate::error::sanitize`] 脱敏为 `TransportError::CloseFailed`，**绝不**外泄原始地址串。
//! 失败显式转 `CloseFailed`、**不**吞错（无 `.ok()` / `unwrap_or` 放行）。

use postern_core::error::TransportError;

use crate::error::{sanitize, InnerFault};

use super::inner::TransportChannelInner;

impl TransportChannelInner {
    /// **优雅释放（close）**：按连接管理指令有序拆除本条通路（§3.5 / F-5）。
    ///
    /// 执行顺序（以桩记录的调用次序断言，§3.5）：停保活任务（`KeepaliveHandle::cancel` +
    /// `join` 收口），然后停桥接泵（`PumpHandle::cancel` + `join` 收口），然后关底层隧道
    /// （`TunnelHandle::close`），最后健康视图转 [`crate::health::Health::Closed`]。
    ///
    /// **幂等**（§3.5）：经 `closed` once 守卫——已 close 过则**直接返回 `Ok(())`**，不二次
    /// 调用桩 close、不报错。底层关闭报错 → 经 [`crate::error::sanitize`] 脱敏为
    /// `Err(TransportError::CloseFailed)`（绝不外泄原始地址，L-7），**不**吞错。
    ///
    /// 关闭后全部后台任务（泵 / 保活）的 `JoinHandle` 均已完成——不留孤儿任务、不再触达
    /// 底层（§3.7 无孤儿任务）。
    pub async fn close(&mut self) -> Result<(), TransportError> {
        // 幂等 once 守卫（§3.5）：已关闭过则直接返回 `Ok(())`——不二次关底层、不报错。
        if self.closed {
            return Ok(());
        }
        self.closed = true;

        // ① 停保活 task：协作 cancel + join 收口（任务确已退出，不留孤儿，§3.7）。
        if let Some(keepalive) = self.keepalive.take() {
            keepalive.cancel();
            keepalive.join().await;
        }

        // ② 停桥接泵：协作 cancel + join 收口（砍在飞，不 await 在途 I/O 自然返回，L-6）。
        if let Some(pump) = self.pump.take() {
            pump.cancel();
            pump.join().await;
        }

        // ③ 关底层隧道（优雅释放末步）。底层报错显式经 sanitize 脱敏为 `CloseFailed`
        //    （绝不外泄真实地址，L-7），**不**吞错。
        let underlay = self.tunnel.close();

        // ④ 健康视图转 `Closed`（关闭终态，§3.5 末步）。底层报错时同样推进终态——通路
        //    确已不可用，健康事实如实推进，错误另经返回值显式呈现（不吞错）。
        self.health_w.mark_closed();

        match underlay {
            Ok(()) => Ok(()),
            Err(()) => Err(sanitize(InnerFault::close("underlay tunnel close failed"))),
        }
    }

    /// **强制切断（abort/cancel）**：紧急切断在用通路，**不走优雅排空**（§3.5 / L-6）。
    ///
    /// 立即 `PumpHandle::cancel` / `KeepaliveHandle::cancel` 砍泵与保活在飞、`TunnelHandle::cancel`
    /// cancel 底层在途操作，**不**await 在途读写 / 续约返回——使在桩慢端点的在途操作在其优雅
    /// 完成**之前**被砍断（L-6）。健康视图转 [`crate::health::Health::Closed`]。
    ///
    /// **幂等**：经 `closed` once 守卫——已关闭则直接返回，不二次砍。abort 后泵 / 保活的
    /// `JoinHandle` 均已完成（经 `join` 收口）——不留孤儿任务（§3.7）。
    pub async fn abort(&mut self) {
        // 幂等 once 守卫（§3.5）：与 close 共用 `closed`——已关闭则直接返回，不二次砍底层。
        if self.closed {
            return;
        }
        self.closed = true;

        // 立即 cancel 保活与桥接泵在飞（取消点命中即丢弃在途 copy / 续约 future，**不**
        // await 其自然返回，L-6 砍在飞），随后 join 收口确认任务确已退出（不留孤儿，§3.7）。
        if let Some(keepalive) = self.keepalive.take() {
            keepalive.cancel();
            keepalive.join().await;
        }
        if let Some(pump) = self.pump.take() {
            pump.cancel();
            pump.join().await;
        }

        // 强制切断底层在途操作 / 中止云侧会话（非优雅 close，无返回值，§3.5 强制路径）。
        self.tunnel.cancel();

        // 健康视图转 `Closed`（abort 终态，§3.5）。
        self.health_w.mark_closed();
    }
}
