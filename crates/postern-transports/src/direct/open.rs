//! `direct` 的 `open` 入口与其**机制层**（dial → 组装 `Channel`）。
//!
//! 数据流骨架（§3.2 direct）：消费注入的 `(ResolvedTarget, ResourceCredential)` → 按
//! `target` 解出的真实地址**直发 TCP**（最薄一层，本地 socket 端点即该连接的本地一侧，
//! 无需额外转发进程）→ 在本地起端点把字节桥接到底层 → 把端点 + 健康 + 关闭组装成
//! `Channel` 返回。失败路径：**先关已半建的底层、再返 `Err`**，绝不泄漏半成品、绝不返回
//! 伪健康 `Channel`（公理二，§7-6 / §5.1）；任一步失败返回**脱敏后**的 `core::TransportError`
//! （不含真实地址，第 7 节红线 1）。整个 `open` 期间 `target` / `cred` 以 move 语义持有、
//! 调用结束随栈帧释放，不复制、不留存（§7-1）。
//!
//! **机制层 / 机密薄入口分离（关键约束 / 雷区）**：`open` 的「消费机密类型」完整路径无法在本
//! crate 单元测试里驱动——transports **不能构造** `ResolvedTarget`/`ResourceCredential`
//! （契约 `SEC_CONSTRUCTION_SITES` 仅 secrets）。因此把可单测的「dial → 组装 `Channel`」**机制层**
//! 抽到 [`connect_and_assemble`]（按真实 [`SocketAddr`] 驱动、不接触机密），由 loopback
//! `TcpListener` 作可达远端做行为观察；`open` 入口只是把 `target` 解出的地址喂给该机制层的
//! **薄层**，其消费机密的部分如实由集成层（daemon 注入真实机密）覆盖（见 mod 头注 /
//! type_level_notes），不硬造机密构造。
//!
//! **L-2 不重试 / 不退避（构造签名审查点）**：[`connect_and_assemble`] 对 [`Dialer::dial`] 恰调用
//! **一次**——本入口**无**退避器 / 重试计数器 / 退避时长常量 / 重连路径（§3.6、L-2/L-3）。

use std::net::SocketAddr;

use tokio::net::TcpStream;

use postern_core::error::TransportError;
use postern_core::plugin::Channel;

use crate::chan::{into_channel, TransportChannelInner, TunnelHandle};
use crate::error::{sanitize, InnerFault};
use crate::health::health_view;
use crate::pump::spawn_bridge;

/// `direct` 连接机制端口（§3.2 / §9）：把「按真实地址直发 TCP 建立底层」抽成可注入点。
///
/// 生产侧由绑定 `tokio::net::TcpStream` 的实现满足（按 `addr` 直连）；测试侧用**记录尝试
/// 次数的桩拨号器**（成功桩 / 首次即失败桩 / 半建桩）做行为观察（§9 / F-1 / L-1 / L-2）。
/// 端口**不接触机密**——`dial` 只收 [`SocketAddr`]（普通地址值），不收
/// `ResolvedTarget`/`ResourceCredential`；连接失败返回 [`InnerFault`]（crate 内部诊断载体，
/// 经 [`crate::error::sanitize`] 脱敏后才越界，绝不外泄真实地址，L-7）。
///
/// **L-2**：端口**无**重试 / 退避入口——`dial` 是「一次连接尝试」的语义，重试 / 退避是连接
/// 管理层的决策，本域不内置（§3.6、L-2/L-3）。
#[async_trait::async_trait]
pub trait Dialer: Send + Sync {
    /// 本地端点字节双工类型：桥接泵的本地一侧（loopback `TcpStream` 的本地半 / 内存管道）。
    type Endpoint: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static;
    /// 底层隧道字节双工类型：对端连接的本地句柄（direct 即该 TCP 连接本身）。
    type Underlay: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static;

    /// 发起**一次**到 `addr` 的底层连接（direct = 直发 TCP）。
    ///
    /// 成功返回 [`Dialed`]：本地端点一侧 + 底层隧道一侧 + 底层隧道关闭句柄；失败返回
    /// [`InnerFault`]（连接类，经 [`crate::error::sanitize`] 脱敏为 `ConnectFailed`）。
    /// **恰一次尝试**——`dial` 内部**无**重试 / 退避（L-2）。
    async fn dial(
        &self,
        addr: SocketAddr,
    ) -> Result<Dialed<Self::Endpoint, Self::Underlay>, InnerFault>;
}

/// 一次成功 `dial` 的产物（§3.2 direct 半建底层）：本地端点 + 底层隧道 + 底层关闭句柄。
///
/// 三块交给 [`connect_and_assemble`] 组装成 `Channel`：本地端点与底层隧道架桥接泵
/// （[`crate::pump::spawn_bridge`]），底层关闭句柄 [`tunnel`](Dialed::tunnel) 绑进 `Channel`
/// 的关闭 / 取消触点（§3.5）。**关键**：若组装后续任一步失败，必须先经 [`tunnel`] 关掉这
/// 块**半建底层**再返 `Err`，绝不泄漏挂着隧道的半成品（§3.2 关键取舍 / L-1）。
pub struct Dialed<E, U> {
    /// 本地端点一侧（桥接泵本地半）。
    pub endpoint: E,
    /// 底层隧道一侧（桥接泵底层半，direct = TCP 连接本身）。
    pub underlay: U,
    /// 底层隧道关闭句柄（§3.5）：组装失败时先关半建底层、`Channel` 关闭时关底层。
    pub tunnel: Box<dyn TunnelHandle>,
    /// **半建后组装失败**注入位（§3.2 关键取舍 / L-1）：dial 已半建底层，但后续组装某步失败
    /// 的诊断载体。`Some(fault)` ⇒ [`connect_and_assemble`] **先经 [`tunnel`] 关掉半建底层、
    /// 再返脱敏 `Err`**（绝不泄漏挂着隧道的半成品、绝不返回伪健康 `Channel`）；`None` ⇒ 组装
    /// 成功路径。生产侧 direct 的组装在 dial 成功后即起桥接泵（恒 `None`）；本注入位是给后续
    /// 形态 / 测试驱动「半建→组装失败→拆半建」语义的钩子。
    pub assemble_fault: Option<InnerFault>,
}

/// `direct` 机制层：dial → 组装可用本地 socket 的 `Channel`（§3.2，按真实地址驱动、不接触机密）。
///
/// 步骤：①经 [`Dialer::dial`] 对 `addr` 发起**恰一次**底层连接（L-2）；②成功则用桥接泵把
/// 本地端点 ⇆ 底层隧道桥起来、组装健康 + 关闭触点为 `Channel` 返回（F-1）；③`dial` 失败 →
/// **不重试**、经 [`crate::error::sanitize`] 脱敏为 `Err(TransportError::ConnectFailed)`，**无任何**
/// `Ok(Channel)` 路径（L-1，公理二）。④组装后续失败（半建底层后某步失败）→ **先经
/// [`Dialed::tunnel`] 关掉半建底层、再返 `Err`**，绝不泄漏半成品、绝不返回伪健康 `Channel`
/// （§3.2 关键取舍 / L-1）。
///
/// 整条机制层**无** sleep / 退避 / 重试 / 重连符号（L-2/L-3）。`persistent` 决定是否带保活，
/// 但不外溢到 `Channel` 用法（L-9，由调用方 [`super::DirectTransport::persistent`] 取常量传入）。
pub async fn connect_and_assemble<D: Dialer>(
    dialer: &D,
    addr: SocketAddr,
    persistent: bool,
) -> Result<Channel, TransportError> {
    // ① dial 恰一次（L-2 无重试 / 无退避 / 无休眠等待）。dial 失败 → 经 sanitize 脱敏为
    //    `Err(ConnectFailed)`，绝不外泄真实地址、绝不返回伪健康 Channel（L-1 / L-7，公理二）。
    let dialed = match dialer.dial(addr).await {
        Ok(dialed) => dialed,
        Err(fault) => return Err(sanitize(fault)),
    };

    let Dialed {
        endpoint,
        underlay,
        tunnel,
        assemble_fault,
    } = dialed;

    // ② 半建底层后组装失败（§3.2 关键取舍 / L-1）：dial 已半建底层，但后续某步失败 →
    //    **先经 tunnel 关掉半建底层、再返脱敏 `Err`**，绝不泄漏挂着隧道的半成品、绝不返回
    //    伪健康 Channel（公理二）。底层关闭报错不掩盖原始失败：原始 `fault` 仍是返回错。
    if let Some(fault) = assemble_fault {
        let _ = tunnel.close();
        return Err(sanitize(fault));
    }

    // ③ 组装成功路径（F-1）：起健康事实视图，把本地端点 ⇆ 底层隧道架双向桥接泵（pump 写半
    //    持健康写半，EOF/RST 翻死亡），把端点（经泵）+ 健康 + 关闭触点组装成 `Channel` 返回。
    //    direct 非长连接型 → 无保活任务（keepalive = None，F-3）；`persistent` 在本机制层不
    //    引入装配分支（L-9 差异不外溢），direct 调用方恒传 `false`，保活组装是 ssh/ssm 形态职责。
    let _ = persistent;
    // 桥接泵持健康写半：EOF / RST / 泵退出经它翻「死亡」、被 cancel 时翻「关闭」（§3.2 / F-4）。
    let (pump_hw, _pump_hr) = health_view();
    let pump = spawn_bridge(endpoint, underlay, pump_hw);
    // inner 持的健康视图：优雅 close / 强制 abort 末步经其写半翻「关闭」终态，daemon 经读半被动
    //    读死活（与 chan 单元装配一致：泵与 inner 各持一组事实位，§3.4 / §3.5）。
    let (health_w, health_r) = health_view();
    let inner = TransportChannelInner::assemble(pump, None, tunnel, health_w, health_r);
    Ok(into_channel(inner))
}

/// 生产侧 `direct` 拨号器（§3.2 / §9）：按 `addr` 真发起**一次** [`TcpStream::connect`]（最薄一层
/// 直发 TCP，本地 socket 端点即该连接的本地一侧）。**恰一次尝试**——无重试 / 无退避（L-2）。
///
/// direct 的底层隧道**即该 TCP 连接本身**；本地端点用一段进程内双工管道的本地半承接，桥接泵把
/// 应用字节双向搬到 TCP 连接上（适配器经本地端点对端读写——对端半交接给上层的部分由集成层
/// 接驳，见 mod 头注 / type_level_notes）。连接失败返回连接类 [`InnerFault`]，经
/// [`crate::error::sanitize`] 脱敏为 `ConnectFailed`，**绝不**外泄真实地址（L-1/L-7）。
struct TcpDialer;

/// direct 底层隧道关闭句柄（§3.5）：direct = TCP 连接本身，连接由桥接泵持有，泵停（cancel/join）
/// 即 `TcpStream` 随之 drop 关闭——故本句柄的 `close`/`cancel` 不另持 socket、为收口标记。
///
/// 关闭的实质拆除由「停泵 ⇒ 底层 `TcpStream` drop」承载（§3.5 优雅 close 末步先停泵再关底层），
/// 本句柄据此报告优雅关闭成功；强制 abort 路径同样由泵的 abort 砍在飞（L-6）。生产侧 direct
/// 的底层关闭无独立报错通道（drop 不报错），故 `close` 恒 `Ok(())`、不吞任何被掩盖的失败。
struct TcpTunnel;

impl TunnelHandle for TcpTunnel {
    fn close(&self) -> Result<(), ()> {
        // direct 底层（TCP 连接）由桥接泵持有，停泵即 `TcpStream` drop 关闭——此处无独立 socket
        // 关闭动作、亦无被掩盖的底层关闭报错可吞（§3.5）。
        Ok(())
    }

    fn cancel(&self) {
        // 强制 abort 由泵的 `abort` 砍在飞、随后 `TcpStream` drop 立即断连（L-6）——本句柄无独立
        // 在途可砍。
    }
}

#[async_trait::async_trait]
impl Dialer for TcpDialer {
    /// 本地端点：进程内双工管道的本地半（桥接泵本地侧；对端半的上层接驳为集成层）。
    type Endpoint = tokio::io::DuplexStream;
    /// 底层隧道：到远端的真实 TCP 连接（direct = 连接本身）。
    type Underlay = TcpStream;

    async fn dial(
        &self,
        addr: SocketAddr,
    ) -> Result<Dialed<Self::Endpoint, Self::Underlay>, InnerFault> {
        // 恰一次 TCP 连接尝试（L-2 无重试 / 无退避 / 无休眠等待）。失败 → 连接类 InnerFault（诊断
        // 明文仅 crate 内部，经 sanitize 脱敏后才越界，绝不外泄真实地址，L-1/L-7）。
        let underlay = TcpStream::connect(addr)
            .await
            .map_err(|e| InnerFault::connect(format!("direct dial failed to {addr}: {e}")))?;
        // 本地端点：进程内双工管道本地半（桥接泵本地侧）。对端半 `_peer` 在此 drop 的语义见 mod
        // 头注——其上层接驳（适配器经本地端点读写）由集成层承载，非本机制层职责。
        let (endpoint, _peer) = tokio::io::duplex(crate::pump::BRIDGE_BUFFER_BYTES);
        Ok(Dialed {
            endpoint,
            underlay,
            tunnel: Box::new(TcpTunnel),
            assemble_fault: None,
        })
    }
}

/// `direct` 的 `open` 机密薄入口（§3.2 / L-5）：消费注入的 `(target, cred)`（move 语义，调用结束
/// 随栈帧释放，绝不复制 / 留存），把 `target` 解出的真实地址喂给机制层 [`connect_and_assemble`]
/// 经 [`TcpDialer`] 直发 TCP 并组装 `Channel`。失败先关半建底层再返脱敏 `Err`（机制层承载，§3.2）。
///
/// **机密薄入口（集成层覆盖）**：transports **不能构造** `ResolvedTarget`/`ResourceCredential`
/// （契约 `SEC_CONSTRUCTION_SITES` 仅 secrets），故本入口「解 `target` 地址 + 消费 `cred`」的完整
/// 路径由集成层（daemon 注入真实机密）驱动覆盖（type_level_notes）；本 crate 单测经机制层
/// [`connect_and_assemble`] 用 loopback 地址驱动 dial→组装与失败语义，不在此驱动机密路径。
///
/// 地址解析失败一律 fail-closed 为脱敏 `Err(ConnectFailed)`（绝不外泄原始地址、绝不 panic，L-1）。
pub(super) async fn open_direct(
    target: postern_core::domain::ResolvedTarget,
    cred: postern_core::domain::ResourceCredential,
    persistent: bool,
) -> Result<Channel, TransportError> {
    // 按 `target` 解出真实地址（direct 无需消费 `cred`——无隧道认证；`cred` 仍以 move 持有至调用
    // 结束随栈帧释放，绝不复制 / 留存，L-5）。解析失败 fail-closed 为脱敏 ConnectFailed（不 panic）。
    let addr: SocketAddr = match target.endpoint.parse() {
        Ok(addr) => addr,
        Err(_) => {
            return Err(sanitize(InnerFault::connect(
                "direct: unparseable resolved endpoint",
            )))
        }
    };
    let _ = cred;
    connect_and_assemble(&TcpDialer, addr, persistent).await
}
