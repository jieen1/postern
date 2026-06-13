//! 通路承载域：在 `core::Channel` 上填充三件套的本域行为（骨架占位，无逻辑）。
//!
//! `Channel` 类型本身定义于 `core`（与 `Adapter::execute` 共享），本域**不重定义
//! 类型**，只在各形态下填充其三件套的具体行为（§3.1）：①**本地端点句柄**——一个
//! 进程内可读写的字节双工端点（回环 TCP `127.0.0.1:<临时端口>` 或 `UnixStream` /
//! `socketpair` 的本地一端），适配器经 `Adapter::execute(ch: &mut Channel, ...)`
//! 在其上读写应用协议字节；②**健康事实视图**（见 [`crate::health`]）；③**关闭 /
//! 取消触点**（见 [`close`]）。把健康与关闭做成旁路控制面而非带内信令，使「读写业务
//! 字节」与「治理这条通路」正交（§3.1 必守不变量 §7-7）。
//!
//! 子模块：
//! - [`inner`]：本地端点句柄与底层隧道句柄、后台任务句柄的私有承载。
//! - [`close`]：幂等关闭 / 强制 abort 触点，绑定底层隧道取消句柄。
//!
//! **F-7 不重定义 `Channel`**：本模块**不**声明名为 `Channel` 的类型；只 `use` core 的
//! `Channel` 并经 [`into_channel`] 把 [`TransportChannelInner`] 装入其 `handle: Box<dyn Send+Sync>`
//! 不透明字段。daemon 经 `downcast` 触达 inner 的健康 / 关闭控制面（§3.1 关键裁决）。

pub mod close;
pub mod inner;

pub use inner::{KeepaliveHandle, TransportChannelInner, TunnelHandle};

use postern_core::plugin::Channel;

/// 把 **crate 内部** 的 [`TransportChannelInner`] 装入 `core::Channel` 的不透明
/// `handle: Box<dyn Send + Sync>`，产出一个对上层一致的 `core::Channel`（F-7 / §3.1）。
///
/// 本函数是「本域不重定义 `Channel`、只填充其 `handle`」的唯一装配点——`core::Channel`
/// 完全不暴露 health/close 方法（只 opaque handle），故本域健康 / 关闭控制面是 `handle`
/// 内部的 [`TransportChannelInner`]，由 daemon 经 `(&*ch.handle).downcast_ref` 触达（§3.1 裁决）。
/// inner 必须 `Send + Sync`（core 约束 `Box<dyn Send + Sync>`）。
pub fn into_channel(inner: TransportChannelInner) -> Channel {
    // 只填充 core 的 `Channel.handle` 不透明字段——不重定义 `Channel`（F-7）。inner 为
    // `Send + Sync`，满足 `Box<dyn Send + Sync>` 约束。
    Channel {
        handle: Box::new(inner),
    }
}
