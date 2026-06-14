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
use tokio::net::UnixListener;
use tower::ServiceExt;
use tower_http::catch_panic::CatchPanicLayer;

use crate::error::Result;

/// 把 `router` 挂到已绑定的 `listener` 上 serve：accept 循环逐条接 UDS 连接、各自 spawn 处理。
///
/// 把 axum `Router` 转为 hyper service，循环 `listener.accept()` 收连接，每条连接经
/// `tokio::spawn` 独立 serve（accept 不被单连接阻塞，§3.8）。listener 已在 boot socket 绑定期
/// 完成 `bind → chmod/设属组 → listen`（L-1），本函数只在其上 serve。单条连接 accept / serve
/// 失败不拖垮 accept 循环——记下后继续接下一条（fail-open 仅限单连接，进程整体 fail-closed：
/// 进程不退出、不放行半装配状态）。handler panic 经 [`CatchPanicLayer`] 收成 500（不外泄）。
pub async fn serve_router_over_uds(listener: UnixListener, router: axum::Router) -> Result<()> {
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
