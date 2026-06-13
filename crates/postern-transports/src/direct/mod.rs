//! `direct` 形态：非隧道直连（tokio TcpStream）。
//!
//! 最薄的一层 `Transport` 实现（§3.2）：按 `ResolvedTarget` 解出的真实地址直接
//! 发起到远端的 TCP 连接，本地 socket 端点即该连接的本地一侧（无需额外转发进程）。
//! `kind()` 取 `"direct"`；`persistent()` 取**编译期固定常量布尔**——direct 是非隧道直连、
//! 用毕即释放，本刀定为非长连接型 `false`（§3.2 / §5.1，取舍见 type_level_notes）。其余两
//! 形态（ssh/ssm）可视为在 direct 之上套一层隧道。
//!
//! 机密消费边界（关键约束）：`open(target: ResolvedTarget, cred: ResourceCredential)`
//! 按值消费机密类型，但 transports **不能构造**它们（契约 `SEC_CONSTRUCTION_SITES`
//! 仅 secrets）——故 `open` 的完整路径无法在本 crate 单元测试里用真实机密直接驱动。
//! 可测的「通路机制」（[`open::connect_and_assemble`] dial → 组装、[`crate::chan`] 三件套、
//! [`crate::pump`] 双向桥接、[`crate::keepalive`] 保活、[`crate::health`] / `persistent` 上报）
//! 用 loopback / 内存管道充分测试；`open` 入口消费机密的薄层如实标注为集成层（daemon 注入
//! 真实机密）覆盖，不硬造机密构造（见 type_level_notes）。
//!
//! 子模块：
//! - [`open`]：机制层 [`open::connect_and_assemble`]（按真实地址 dial → 组装 `Channel`）+
//!   消费机密的 `open` 薄入口（集成层覆盖）。

pub mod open;

use async_trait::async_trait;

use postern_core::domain::{ResolvedTarget, ResourceCredential};
use postern_core::error::TransportError;
use postern_core::plugin::{Channel, Transport};

/// `direct` 形态的注册键常量（§5.1 / F-8）：`kind()` 恒返回此值，用于传输注册表选型。
pub const KIND: &str = "direct";

/// `direct` 是否长连接型（§3.2 / §5.1 / F-6）——**编译期固定常量布尔**。
///
/// direct 是非隧道直连、用毕即释放，本刀定为**非长连接型** `false`（连接管理层据此**不**池化、
/// 用毕即销，§3 第 3 项 / F-3）。`persistent()` 读此常量，**不**读配置 / 通路状态（F-6 构造签名
/// 审查点：返回值为该实现固定的常量布尔）。
pub const PERSISTENT: bool = false;

/// `direct` 形态的 [`Transport`] 实现：最薄的一层非隧道直连（§3.2 / §5.1）。
///
/// 无字段——`kind()`/`persistent()` 取**编译期常量**（[`KIND`]/[`PERSISTENT`]，不读配置 / 通路
/// 状态，F-6）；`open` 消费注入的 `(target, cred)`（move 语义，调用结束即释放，L-5）经机制层
/// [`open::connect_and_assemble`] 建直连并组装 `Channel`（F-1）。
///
/// **不持凭据、不池化、不做通路间生命周期决策**（那是 daemon 连接管理层，§1）；本类型**无**
/// 退避器 / 重试计数器 / 重连路径（L-2/L-3）。
#[derive(Debug, Default, Clone, Copy)]
pub struct DirectTransport;

impl DirectTransport {
    /// 构造一个 `direct` 传输实例（无状态、无配置）。
    pub fn new() -> Self {
        DirectTransport
    }
}

#[async_trait]
impl Transport for DirectTransport {
    /// 传输注册表选型键：恒为 `"direct"`（§5.1 / F-8）。
    fn kind(&self) -> &'static str {
        KIND
    }

    /// 是否长连接型：恒为**编译期常量** [`PERSISTENT`]（`false`，非隧道直连用毕即释放，
    /// §3.2 / F-6）——不读配置 / 通路状态，同一实例多次调用返回值恒等。
    fn persistent(&self) -> bool {
        PERSISTENT
    }

    /// 建立到远端的 direct 通路（§3.2 / F-1）：消费注入的 `(target, cred)`（move，调用结束即
    /// 释放，L-5）→ 按 `target` 解出真实地址 → 经机制层 [`open::connect_and_assemble`] 直发
    /// TCP 并组装 `Channel`。失败先关半建底层再返脱敏 `Err`（§3.2 / L-1）。
    ///
    /// **机密薄入口（集成层覆盖）**：`target`/`cred` 是机密类型，transports **不能构造**
    /// （`SEC_CONSTRUCTION_SITES` 仅 secrets），故本入口「按 `target` 解地址 + 消费 `cred`」的
    /// 完整路径由集成层（daemon 注入真实机密）覆盖（type_level_notes）；本 crate 单测经
    /// [`open::connect_and_assemble`] 机制层用 loopback 地址驱动 dial→组装与失败语义。
    async fn open(
        &self,
        target: ResolvedTarget,
        cred: ResourceCredential,
    ) -> Result<Channel, TransportError> {
        // move 语义持有 target/cred，调用结束随栈帧释放（L-5）；机密薄入口（解地址 + 消费
        // cred）由集成层覆盖（daemon 注入真实机密驱动）。机制层 dial→组装在
        // `open::connect_and_assemble`（按真实地址驱动）；失败先关半建底层再返脱敏 Err（§3.2/L-1）。
        open::open_direct(target, cred, PERSISTENT).await
    }
}
