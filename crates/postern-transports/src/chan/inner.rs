//! `Channel` 私有内里：本地端点 + 底层隧道 + 后台任务句柄（设计承诺级桩，函数体 `todo!()`）。
//!
//! 承载 `core::Channel::handle` 背后的不透明 payload：本地字节端点的一侧、底层隧道
//! 句柄、以及最多两类后台 tokio 任务句柄（桥接泵、长连接型保活）。任务边界即
//! `Channel` 生命周期边界——`Channel` 关闭即其全部后台任务随之终止，不留孤儿任务
//! （§3.7）。本域不持全局共享可变状态、无跨 `Channel` 协调（无池、无表，§7-4）。
//!
//! **F-7 不重定义 `Channel`**：本模块**绝不**声明名为 `Channel` 的类型；[`TransportChannelInner`]
//! 是 **crate 内部** 的具体通路状态类型，经 `Box<dyn Send + Sync>` 装入 `core::Channel.handle`
//! 的不透明 `handle` 字段——因 `core::Channel` 完全不暴露 health/close 方法（只 opaque handle），
//! 本域的健康 / 关闭控制面是 `handle` 内部的具体类型，由 daemon 经 downcast 触达（§3.1 关键裁决）。
//!
//! **L-9 差异不外溢**：本类型的关闭 / 健康接口**不含**长 / 非长（`persistent`）分支——
//! ssh/ssm/direct 三形态装配出的 inner 用法一致，persistent 差异只由 `Transport::persistent()`
//! 承载、不进入 `Channel` 装配（§3.1 / §5.2 / §7-7）。

use std::sync::Arc;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::health::{HealthReader, HealthWriter};
use crate::pump::PumpHandle;

/// 底层隧道关闭端口（§3.5）：`Channel` 关闭 / 取消触点背后绑定的「关底层隧道」抽象。
///
/// 生产侧由各形态满足（direct 关 TCP / ssh 关会话 / ssm 中止云侧会话）；测试侧用**记录
/// close/cancel 调用次数的桩隧道**做行为观察（§9 / F-5 / L-6）。端口**不接触机密**——
/// 方法不收 `ResolvedTarget`/`ResourceCredential`，关闭报错经 [`crate::error::sanitize`]
/// 脱敏为 `CloseFailed`，**绝不**外泄真实地址（§3.5 / L-7）。
///
/// 两条语义（§3.5）：[`TunnelHandle::close`] 优雅释放（有序拆除底层隧道）；
/// [`TunnelHandle::cancel`] 强制 abort（立即取消底层在途操作 / 中止云侧会话，不等在途）。
pub trait TunnelHandle: Send + Sync {
    /// 优雅关底层隧道（§3.5 优雅释放路径的末步）。返回 `Err(())` 表示底层关闭报错——
    /// 上层据此经 [`crate::error::sanitize`] 脱敏为 `CloseFailed`（绝不外泄原始地址）。
    ///
    /// 错误**故意无载体**（`()`）：底层关闭报错的真实地址 / 原始错误串绝不随此返回越界
    /// （L-7），脱敏判别由上层 [`crate::error::sanitize`] 统一收口——故无自定义错误类型。
    #[allow(clippy::result_unit_err)]
    fn close(&self) -> Result<(), ()>;

    /// 强制取消底层在途操作 / 中止云侧会话（§3.5 强制 abort 路径）：立即砍底层在飞 I/O，
    /// **不**等待其优雅返回（L-6）。
    fn cancel(&self);
}

/// 长连接型保活后台任务的取消 / 收口句柄（§3.3 / §3.7）。
///
/// 与 [`crate::pump::PumpHandle`] 对称：保活任务以 [`JoinHandle`] 承载，`cancel()` 经协作取消
/// 信号让其在节律点退出、`abort()` 走 [`JoinHandle::abort`] 硬中止。任务边界即 `Channel`
/// 生命周期边界——关闭即随之终止，不留孤儿任务。非长连接型 `Channel` 装配时此句柄为 `None`
/// （F-3：非长连接无保活任务），但**装配 / 关闭代码路径对二者一致**（L-9 差异不外溢）。
pub struct KeepaliveHandle {
    /// 后台保活任务句柄——`abort()` 走它硬中止（§3.5 强制 abort 路径）。
    task: JoinHandle<()>,
    /// 协作取消信号——`cancel()` 触发它，保活任务在节律点立即退出（§3.5 / L-6）。
    cancel: Arc<Notify>,
}

impl KeepaliveHandle {
    /// 用一个已 spawn 的保活任务与其协作取消信号组装收口句柄（§3.3 / §3.7）。
    pub fn new(task: JoinHandle<()>, cancel: Arc<Notify>) -> Self {
        Self { task, cancel }
    }

    /// 协作取消：通知保活任务在节律点立即退出，**不**等待在途续约 / 心跳返回（§3.5 / L-6）。
    pub fn cancel(&self) {
        // 唤醒保活任务在 `cancel.notified()` 上等待的分支：任务在其节律点收口退出
        // （砍在飞，不 await 在途续约 / 心跳自然返回，L-6）。
        self.cancel.notify_one();
    }

    /// 硬中止后台保活任务（[`JoinHandle::abort`]）：立即丢弃在途、任务不再触达底层（§3.5/§3.7）。
    pub fn abort(&self) {
        self.task.abort();
    }

    /// 等待保活任务终止（供关闭 / abort 收口处观测「任务确已结束」，不留孤儿）。
    /// 返回任务是否因 [`Self::abort`] 被取消（`true` = 被 abort 取消，`false` = 协作退出）。
    pub async fn join(self) -> bool {
        // `Ok(())` = 任务自然 / 协作退出；`Err(e)` 且 `e.is_cancelled()` = 经 `abort()` 硬取消。
        // 任务体禁 panic，其余 `Err` 按「非 abort 取消」收口。
        match self.task.await {
            Ok(()) => false,
            Err(e) => e.is_cancelled(),
        }
    }
}

/// `core::Channel::handle` 背后的 **crate 内部** 具体通路状态（§3.1 三件套装配）。
///
/// 把「本地端点句柄 + 健康事实视图 + 关闭 / 取消触点」三块组装为一个具体类型，经
/// `Box<dyn Send + Sync>` 装入 `core::Channel.handle`（F-7：**不**重定义 `Channel`；本类型
/// 只是 `handle` 的不透明 payload）。装入 `handle` 的 inner 必须 `Send + Sync`（core 约束
/// `Box<dyn Send + Sync>`）。
///
/// 三件套（§3.1）：
/// - **健康事实视图**：[`HealthReader`]（被动读）+ [`HealthWriter`]（关闭时写 `Closed`），来自
///   [`crate::health::health_view`]，与 pump / keepalive 写半共享同一事实位。
/// - **关闭 / 取消触点**：见 [`crate::chan::close`]，背后绑定 [`PumpHandle`] / [`KeepaliveHandle`]
///   与底层 [`TunnelHandle`]。
/// - **本地端点句柄**：本地字节双工端点的一侧（loopback / 内存管道），适配器经
///   `Adapter::execute(ch: &mut Channel, ...)` 读写——本骨架不内联端点字段，端点的搬运由
///   [`PumpHandle`] 背后的桥接泵承载（§3.2）。
///
/// **L-9**：本类型**无** `persistent` 字段、关闭 / 健康接口**无**长 / 非长分支——保活任务有无
/// 仅体现为 `keepalive: Option<KeepaliveHandle>`，装配 / 关闭路径对二者一致。
pub struct TransportChannelInner {
    /// 桥接泵收口句柄（§3.2 / §3.5）：关闭时停泵、abort 时砍在飞。`take` 进 `join`
    /// 收口后置 `None`（once 守卫下不会被二次触达）。
    pub(super) pump: Option<PumpHandle>,
    /// 长连接型保活收口句柄；非长连接型装配即为 `None`（F-3）。装配 / 关闭路径对二者
    /// 一致（L-9）。`take` 进 `join` 收口后置 `None`。
    pub(super) keepalive: Option<KeepaliveHandle>,
    /// 底层隧道关闭端口（§3.5）：优雅 close 的末步关底层、强制 abort 的 cancel 触点。
    pub(super) tunnel: Box<dyn TunnelHandle>,
    /// 健康事实**写半**：关闭收尾时写 `Closed`（§3.5 优雅 close 末步 / abort 终态）。
    pub(super) health_w: HealthWriter,
    /// 健康事实**读半**：供 daemon（经 downcast 触达）被动读当前死活事实（§3.4）。
    pub(super) health_r: HealthReader,
    /// 幂等关闭标志（§3.5）：close / abort 经此 once 守卫——重复下达不二次关底层、不报错。
    pub(super) closed: bool,
}

impl TransportChannelInner {
    /// 把三件套装配为一个具体通路状态（§3.1）：本地端点（经桥接泵）+ 健康视图 + 关闭触点。
    ///
    /// `keepalive` 为 `None` 即非长连接型（F-3 无保活任务），为 `Some` 即长连接型；**两形态
    /// 装配路径一致**（L-9 差异不外溢——本签名无 `persistent` 布尔分支）。装入 `handle` 的
    /// inner 由本函数产出、`Send + Sync`（core 约束）。
    pub fn assemble(
        pump: PumpHandle,
        keepalive: Option<KeepaliveHandle>,
        tunnel: Box<dyn TunnelHandle>,
        health_w: HealthWriter,
        health_r: HealthReader,
    ) -> Self {
        Self {
            pump: Some(pump),
            keepalive,
            tunnel,
            health_w,
            health_r,
            closed: false,
        }
    }

    /// 被动读当前健康事实（§3.4）：daemon 经 downcast 拿到本 inner 后据此查通路死活。
    /// 同步、非回调——是「连接管理层来读」的被动事实位（§6.2）。
    pub fn health(&self) -> crate::health::Health {
        self.health_r.get()
    }
}
