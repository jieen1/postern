//! data.sock 监听层：唯一的 ConnOrigin 构造点。
//!
//! 接受 UDS 连接后，经 SO_PEERCRED（tokio [`tokio::net::UnixStream::peer_cred`] 安全
//! API，无 unsafe）取对端 (uid,gid)，构造 [`ConnOrigin::UnixPeer`]——这是全进程唯一可
//! 信的来源事实，绝不采信请求自报字段。TCP 入口（如有）同理构造 [`ConnOrigin::Tcp`]。
//! 构造出的来源按值交给外壳，外壳再传入数据面内核。
//!
//! 契约 SEC_CONSTRUCTION_SITES 仅放行本目录出现字面 ConnOrigin 变体。

use postern_core::request::ConnOrigin;
#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

use crate::error::DaemonError;

/// 从一条已接受的 UDS 连接采集对端凭据并构造来源事实（unix）。
///
/// 经 `peer_cred`（SO_PEERCRED 安全 API，无 unsafe、无 libc 直调、不采信自报字段）取对端
/// (uid,gid)，构造唯一可信的 [`ConnOrigin::UnixPeer`]。这是全进程**唯一**字面构造 `ConnOrigin`
/// 的位置（契约 SEC_CONSTRUCTION_SITES 仅放行 `src/shells/` 下出现字面变体）。
///
/// `peer_cred` 失败（极罕见：对端瞬断 / 非 UDS）⇒ fail-closed 返
/// [`DaemonError::Listener`]——无可信来源时绝不放行（绝不退化为自报或匿名来源）。
#[cfg(unix)]
pub fn origin_of(stream: &UnixStream) -> crate::error::Result<ConnOrigin> {
    // 唯一构造点：listener 层经 SO_PEERCRED 取 (uid,gid) 后构造（无 unsafe / 不自报）。
    let cred = stream.peer_cred().map_err(|_| DaemonError::Listener)?;
    Ok(ConnOrigin::UnixPeer {
        uid: cred.uid(),
        gid: cred.gid(),
    })
}

/// 从一条已接受的本地回环 TCP 连接采集对端地址并构造来源事实（windows）。
///
/// 原生 Windows 无 UDS / SO_PEERCRED——无内核对端 (uid,gid) 可取。control/data 平面均为
/// 127.0.0.1 回环 TCP（仅本机回环可连），来源事实取对端 socket 地址构造 [`ConnOrigin::Tcp`]
/// （core::request 既有变体）。这与 unix `origin_of` 同样是 `src/shells/` 下的合规构造点
/// （契约 SEC_CONSTRUCTION_SITES 仅放行本目录字面变体）。
///
/// windows 安全模型降级：peer-uid 主门旁路，仅回环可连 + control-token 必验（认证中间件
/// 的 cfg(windows) 分支据此放行 uid 门、token 仍为唯一接入凭据）。`peer_addr` 失败（对端瞬断）
/// ⇒ fail-closed 返 [`DaemonError::Listener`]（无可信来源时绝不放行）。
#[cfg(windows)]
pub fn origin_of(stream: &TcpStream) -> crate::error::Result<ConnOrigin> {
    // 唯一构造点：listener 层取对端回环地址构造 ConnOrigin::Tcp（不采信自报字段）。
    let remote = stream.peer_addr().map_err(|_| DaemonError::Listener)?;
    Ok(ConnOrigin::Tcp { remote })
}

/// 控制面本地来源事实（control.sock 是 0600 同 uid 本地 socket）——经 SO_PEERCRED 自连对取
/// 本进程 (uid,gid) 构造 [`ConnOrigin::UnixPeer`]。
///
/// 控制面写端点的 `policy_change` 审计需要一个来源事实；生产路径由
/// [`serve_control_router_over_uds`](crate::shells::serve::serve_control_router_over_uds) 经
/// `Extension(Origin)` 逐连接透传真实对端来源，控制面 handler 优先读该注入值。本函数提供该
/// 注入**缺位**时（未经控制面 serve 路径，如 in-process router 装配）的本地来源回退：因
/// control.sock 仅同 uid 可连（0600），本进程 uid 即唯一合法对端 uid——以 `UnixStream::pair()`
/// 自连对一端 `peer_cred()` 取本进程 (uid,gid)（与 boot 自身 uid 探测同一安全 API，无 unsafe /
/// 无 libc 直调 / 无新增依赖）。
///
/// 这是控制面来源回退的**唯一字面构造点**（契约 SEC_CONSTRUCTION_SITES 仅放行 `src/shells/`
/// 下出现字面 `ConnOrigin` 变体——control/ 的 handler 只读注入值、绝不字面构造）。自连对 /
/// `peer_cred` 失败（极罕见）⇒ fail-closed 回 [`DaemonError::Listener`]（无可信本地 uid 时绝不
/// 伪造来源）。
#[cfg(unix)]
pub fn control_local_origin() -> crate::error::Result<ConnOrigin> {
    // SO_PEERCRED 安全 API：自连对一端的 peer_cred() 即本进程 (uid,gid)（无 unsafe / 不自报）。
    let (a, _b) = UnixStream::pair().map_err(|_| DaemonError::Listener)?;
    let cred = a.peer_cred().map_err(|_| DaemonError::Listener)?;
    Ok(ConnOrigin::UnixPeer {
        uid: cred.uid(),
        gid: cred.gid(),
    })
}

/// （windows）控制面本地来源事实回退：构造 127.0.0.1 回环 [`ConnOrigin::Tcp`]。
///
/// 原生 Windows 无 UDS / SO_PEERCRED——control 平面为 127.0.0.1 回环 TCP（仅本机可连）。注入
/// 缺位时（in-process router 装配，非 serve 路径）的本地来源回退取本机回环地址构造 Tcp 来源
/// （与 serve 路径的 [`origin_of`] 同样是 `src/shells/` 下的合规字面构造点）。地址取
/// `127.0.0.1:0`（端口 0 = 未指定本地对端口，仅承载「本机回环来源」事实，审计无机密）。
#[cfg(windows)]
pub fn control_local_origin() -> crate::error::Result<ConnOrigin> {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let remote = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    Ok(ConnOrigin::Tcp { remote })
}
