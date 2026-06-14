//! data.sock 监听层：唯一的 ConnOrigin 构造点。
//!
//! 接受 UDS 连接后，经 SO_PEERCRED（tokio [`tokio::net::UnixStream::peer_cred`] 安全
//! API，无 unsafe）取对端 (uid,gid)，构造 [`ConnOrigin::UnixPeer`]——这是全进程唯一可
//! 信的来源事实，绝不采信请求自报字段。TCP 入口（如有）同理构造 [`ConnOrigin::Tcp`]。
//! 构造出的来源按值交给外壳，外壳再传入数据面内核。
//!
//! 契约 SEC_CONSTRUCTION_SITES 仅放行本目录出现字面 ConnOrigin 变体。

use postern_core::request::ConnOrigin;
use tokio::net::UnixStream;

use crate::error::DaemonError;

/// 从一条已接受的 UDS 连接采集对端凭据并构造来源事实。
///
/// 经 `peer_cred`（SO_PEERCRED 安全 API，无 unsafe、无 libc 直调、不采信自报字段）取对端
/// (uid,gid)，构造唯一可信的 [`ConnOrigin::UnixPeer`]。这是全进程**唯一**字面构造 `ConnOrigin`
/// 的位置（契约 SEC_CONSTRUCTION_SITES 仅放行 `src/shells/` 下出现字面变体）。
///
/// `peer_cred` 失败（极罕见：对端瞬断 / 非 UDS）⇒ fail-closed 返
/// [`DaemonError::Listener`]——无可信来源时绝不放行（绝不退化为自报或匿名来源）。
pub fn origin_of(stream: &UnixStream) -> crate::error::Result<ConnOrigin> {
    // 唯一构造点：listener 层经 SO_PEERCRED 取 (uid,gid) 后构造（无 unsafe / 不自报）。
    let cred = stream.peer_cred().map_err(|_| DaemonError::Listener)?;
    Ok(ConnOrigin::UnixPeer {
        uid: cred.uid(),
        gid: cred.gid(),
    })
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
pub fn control_local_origin() -> crate::error::Result<ConnOrigin> {
    // SO_PEERCRED 安全 API：自连对一端的 peer_cred() 即本进程 (uid,gid)（无 unsafe / 不自报）。
    let (a, _b) = UnixStream::pair().map_err(|_| DaemonError::Listener)?;
    let cred = a.peer_cred().map_err(|_| DaemonError::Listener)?;
    Ok(ConnOrigin::UnixPeer {
        uid: cred.uid(),
        gid: cred.gid(),
    })
}
