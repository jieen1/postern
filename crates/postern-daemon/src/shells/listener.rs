//! data.sock 监听层：唯一的 ConnOrigin 构造点。
//!
//! 接受 UDS 连接后，经 SO_PEERCRED（tokio [`tokio::net::UnixStream::peer_cred`] 安全
//! API，无 unsafe）取对端 (uid,gid)，构造 [`ConnOrigin::UnixPeer`]——这是全进程唯一可
//! 信的来源事实，绝不采信请求自报字段。TCP 入口（如有）同理构造 [`ConnOrigin::Tcp`]。
//! 构造出的来源按值交给外壳，外壳再传入数据面内核。
//!
//! 契约 SEC_CONSTRUCTION_SITES 仅放行本目录出现字面 ConnOrigin 变体。
//!
//! 本波次为骨架：来源采集入口桩，零接受循环逻辑。

use postern_core::request::ConnOrigin;
use tokio::net::UnixStream;

/// 从一条已接受的 UDS 连接采集对端凭据并构造来源事实。
///
/// 经 `peer_cred` 取 (uid,gid)，构造唯一可信的 [`ConnOrigin::UnixPeer`]。
pub fn origin_of(_stream: &UnixStream) -> crate::error::Result<ConnOrigin> {
    // 唯一构造点：listener 层经 SO_PEERCRED 取 (uid,gid) 后构造。
    // 形如 ConnOrigin::UnixPeer { uid, gid }（本波次为桩，参数留待接受循环填充）。
    todo!()
}
