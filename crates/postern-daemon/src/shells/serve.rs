//! axum-over-UDS 服务：把一个 axum `Router` 挂到一条已绑定的 `UnixListener` 上接受连接。
//!
//! 形态（模块文档 06 §8.12 / §3.8）：两平面 router（data.sock 数据面 / control.sock 控制面）
//! 各自经本缝在对应 `UnixListener` 上 serve——`accept` 循环逐条收 UDS 连接，把 axum `Router`
//! 转成 hyper service 喂给连接。每条连接经 `tokio::spawn` 独立处理，accept 循环不被单连接阻塞。
//!
//! 边界纪律：本缝只做「listener × router → serve」的搬运，**不**采集对端 (uid,gid)——来源采集
//! （SO_PEERCRED → `ConnOrigin`）唯一发生在 [`listener`](crate::shells::listener)，本文件不构造
//! `ConnOrigin`。零 SQL 标记、`anyhow` 禁用（仅 main.rs），失败一律 fail-closed 返
//! [`DaemonError::Listener`](crate::error::DaemonError::Listener)。
//!
//! panic 兜底（fail-closed）：router 在 serve 前 front 一层 [`CatchPanicLayer`]——单条请求
//! handler panic 被收成 500 响应而非外泄，accept 循环与进程绝不因单连接 panic 崩溃（公理二）。

use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use tower::ServiceExt;
use tower_http::catch_panic::CatchPanicLayer;

use crate::boot::sockets::TokioListener;
use crate::control::auth::{operator_of_peer, PeerUid};
use crate::error::Result;
use crate::shells::listener::origin_of;

/// 把 `router` 挂到已绑定的 `listener` 上 serve：accept 循环逐条接 UDS 连接、各自 spawn 处理。
///
/// 把 axum `Router` 转为 hyper service，循环 `listener.accept()` 收连接，每条连接经
/// `tokio::spawn` 独立 serve（accept 不被单连接阻塞，§3.8）。listener 已在 boot socket 绑定期
/// 完成 `bind → chmod/设属组 → listen`（L-1），本函数只在其上 serve。单条连接 accept / serve
/// 失败不拖垮 accept 循环——记下后继续接下一条（fail-open 仅限单连接，进程整体 fail-closed：
/// 进程不退出、不放行半装配状态）。handler panic 经 [`CatchPanicLayer`] 收成 500（不外泄）。
pub async fn serve_router_over_uds(listener: TokioListener, router: axum::Router) -> Result<()> {
    // 全 router front 一层 CatchPanic：单请求 handler panic → 500 响应（不外泄、不毒化连接任务）。
    let router = router.layer(CatchPanicLayer::new());

    loop {
        // accept 一条 UDS 连接。单条 accept 失败（对端瞬断 / EMFILE 等）不拖垮整个监听循环：
        // 跳过本条、继续接下一条（进程整体仍 serving，绝不因单连接错误整体退出）。
        let (stream, _addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        // 每条连接独立 spawn（accept 循环不被单连接阻塞，§3.8 multi-thread）。
        let router = router.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            // 把 axum router 转 hyper service：每条请求克隆 router 后 oneshot（Router<()> 的
            // Service::Error 为 Infallible，故 serve_connection 不因业务错误中断连接）。
            let service = hyper::service::service_fn(move |req| {
                let router = router.clone();
                async move { router.oneshot(req).await }
            });
            // http1 单连接 serve；连接级 IO 错误（对端断开等）落本任务、不外泄到 accept 循环。
            let _ = http1::Builder::new().serve_connection(io, service).await;
        });
    }
}

/// 把控制面 `router` 挂到 control.sock 的 `listener` 上 serve，**逐连接经 SO_PEERCRED 采集对端
/// 来源并注入请求扩展链**（控制面专用，认证主门的来源事实在此采集，L-1 / B-2）。
///
/// 与 [`serve_router_over_uds`] 的唯一区别：每条连接 accept 后经
/// [`origin_of`](crate::shells::listener::origin_of)（SO_PEERCRED 安全 API，唯一 `ConnOrigin`
/// 构造点）取对端 `(uid,gid)`，把对端 uid 经 `Extension(PeerUid)`、来源经 `Extension(Origin)`
/// 注入该连接每条请求的扩展集——控制面认证中间件（[`control_auth`](crate::control::auth::control_auth)）
/// 据此 `PeerUid` 比对 `self_uid`（主门），审计端点据 `Origin` 留痕。`origin_of` 失败（无可信
/// 来源事实）⇒ fail-closed 跳过本条连接（绝不在来源缺失时放行：无注入 ⇒ 中间件 peer 门必拒）。
///
/// router 在 serve 前已由 boot 装配期 front 认证中间件（[`with_control_auth`](crate::control::router::with_control_auth)）；
/// 本函数只负责「每连接来源采集 + 注入」与 accept 循环，单连接 panic 经 [`CatchPanicLayer`] 收 500。
pub async fn serve_control_router_over_uds(
    listener: TokioListener,
    router: axum::Router,
) -> Result<()> {
    // 全 router front 一层 CatchPanic（与数据面同纪律：单请求 handler panic → 500，不外泄）。
    let router = router.layer(CatchPanicLayer::new());

    loop {
        // accept 一条 control.sock 连接（单条 accept 失败不拖垮监听循环，跳过续接下一条）。
        let (stream, _addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        // 唯一来源采集点：经 SO_PEERCRED 取对端来源事实（origin_of 在 shells/，合规构造）。
        // 失败 ⇒ 无可信对端事实 ⇒ fail-closed 跳过本条连接（绝不放行无来源连接）。
        let origin = match origin_of(&stream) {
            Ok(o) => o,
            Err(_) => continue,
        };
        // 从来源事实析出对端 uid（主门比对值）；control/ 的中间件只读此 u32、不持来源类型。
        // - unix：control.sock 是 UDS，来源恒 UnixPeer，取其 SO_PEERCRED uid 作主门比对值。
        // - windows：control 是 127.0.0.1 回环 TCP，来源恒 Tcp，无内核对端 uid——peer-uid 门在
        //   windows 被旁路（token-only），注入哨兵 uid 使中间件 uid 比对恒成立（见 boot::real
        //   WINDOWS_SENTINEL_UID 与 control::auth 的 cfg(windows) 分支）。非预期变体 fail-closed 跳过。
        #[cfg(unix)]
        let peer_uid = match &origin {
            postern_core::request::ConnOrigin::UnixPeer { uid, .. } => *uid,
            _ => continue,
        };
        #[cfg(windows)]
        let peer_uid = match &origin {
            postern_core::request::ConnOrigin::Tcp { .. } => {
                crate::boot::real::WINDOWS_SENTINEL_UID
            }
            _ => continue,
        };
        // 已认证操作者身份：由对端 SO_PEERCRED uid 派生（control.sock 0600 + uid 主门 + token，
        // 故对端 uid 即「是谁在写」的可信标识）。注入 Extension(Actor) 让写 handler 据此填
        // created_by/updated_by 审计字段（生产路径绝不再退化为空串 / 常量操作者）。
        let actor = operator_of_peer(peer_uid);

        let router = router.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = hyper::service::service_fn(move |mut req| {
                let router = router.clone();
                let origin = origin.clone();
                let actor = actor.clone();
                async move {
                    // 把本连接对端 uid + 来源 + 已认证操作者经请求扩展注入 handler 链（认证中间件 /
                    // 写 handler 审计支据此）。自报字段绝不被读取——注入值恒为 listener 采集者（B-2）。
                    req.extensions_mut().insert(PeerUid(peer_uid));
                    req.extensions_mut().insert(origin);
                    req.extensions_mut().insert(actor);
                    router.oneshot(req).await
                }
            });
            let _ = http1::Builder::new().serve_connection(io, service).await;
        });
    }
}
