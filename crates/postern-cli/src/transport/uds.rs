//! hyperlocal 连 `control.sock`：一次性 HTTP-over-UDS 往返（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.2，F-3）：以 `hyperlocal` 的 UDS 约定把目标 `control.sock`
//! 路径包装成 hyper 可用的 `Uri`（UDS 路径以 hyperlocal 约定编码进 URI host 段，真实
//! 请求行仍用 6.5 的 `/v1/...` 路径），发起一次往返：建连 → 发请求 → 读完整响应 → 关闭。
//!
//! 关键纪律（§3.2/§3.9）：不持久、不池化、不开后台保活任务；可设连接 / 读取超时，超时按
//! daemon 不可达类报错；**无客户端重试**（含 `409` 不自动重试，§3.5）。一次性短命请求是
//! 唯一形态——观测到 0 次或 ≥2 次连接（隐式重试 / 保活）即违反 F-3。
//!
//! 雷区（本 unit 概要）：**不**用 `hyper_util::client::legacy::Client`（连接池 / 复用 /
//! 保活默认会触发 ≥2 次连接的误判，与 F-3 冲突）——每次调用新建一个 `tokio::net::UnixStream`
//! 并做一次 `hyper::client::conn::http1::handshake`，请求收完即落连接。连接 / 读 / 超时失败
//! 一律映射到 [`CliError::DaemonUnreachable`]（fail-closed，公理二；**无** store/secrets 回退
//! 路径，L-2）。本 unit 不依赖 store / secrets 任一 crate（架构禁止边 cli ↛ store/secrets，
//! B-1），亦**不**构造任何机密族类型（结构上无该路径，B-3）。

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;

use crate::error::CliError;
use crate::reqspec::RequestSpec;

/// daemon 控制面响应的 CLI 侧最小载体（§3.2/§3.3）：HTTP 状态码 + 完整响应体字节。
///
/// 传输层只负责"把一次往返的状态与体字节交还"——**不**在此判信封类别、**不**反序列化、
/// **不**渲染（那在 `render` 面，§3.3）。`status` 原样保留以供上层据 `409`/4xx/5xx 分流
/// （L-5/L-7）；`body` 是读完的完整字节，交渲染面按信封三分支处置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP 状态码（如 `200`/`409`/`500`）。上层据此分流：`409` → 冲突呈现 + 提示重读
    /// `version`、绝不自动重试覆盖（L-5）；写端点 5xx → 如实呈现失败、不本地补偿（L-7）。
    pub status: u16,
    /// 完整响应体字节（已读尽）。传输层不解释其内容，交渲染面按信封类别处置。
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// `409 Conflict`——乐观锁版本不匹配（§3.5/L-5）。上层据此原样呈现冲突并提示重读最新
    /// `version`，**绝不**自动重试覆盖。判定只看状态码，不在传输层做任何重试决策。
    pub const STATUS_CONFLICT: u16 = 409;
}

/// 一次性 HTTP-over-UDS 客户端（§3.2，F-3）：包装目标 `control.sock` 路径与超时，
/// 每次 [`UdsTransport::round_trip`] 新建连接、发一次、读完整、关闭。
///
/// **无连接池、无保活、无重试**（取舍：CLI 是一次性短命进程，连接池只增复杂度与"≥2 次
/// 连接"的误判风险，与 F-3"恰一次往返"冲突；瘦客户端的正确形态是无状态单发）。本类型
/// 不持任何跨命令复用的连接句柄——它只持路径与超时这两项纯配置。
//
#[derive(Debug, Clone)]
pub struct UdsTransport {
    /// 目标 `control.sock` 的文件系统路径（unix 本地 IPC 入口）。windows 上本地 IPC 走
    /// 127.0.0.1 TCP（连接地址取自 `POSTERN_CONTROL_PORT` 环境变量，见 [`control_tcp_addr`]），
    /// 此路径在 windows 不参与连接——保留字段使构造签名跨平台一致（main.rs 仍传路径）。
    #[cfg_attr(windows, allow(dead_code))]
    socket_path: std::path::PathBuf,
    /// 连接 / 读取超时。超时按 daemon 不可达类报错（§3.9）；**不**触发任何客户端重试。
    timeout: Duration,
}

impl UdsTransport {
    /// 以目标 `control.sock` 路径与超时构造一次性客户端（§3.2）。仅持纯配置，不建任何连接、
    /// 不起任何后台任务——连接在每次 [`round_trip`](Self::round_trip) 内即建即关。
    pub fn new(socket_path: impl Into<std::path::PathBuf>, timeout: Duration) -> Self {
        UdsTransport {
            socket_path: socket_path.into(),
            timeout,
        }
    }

    /// 发起**恰好一次** HTTP-over-UDS 往返（§3.2，F-3）：新建 `UnixStream` 连 `control.sock`
    /// → 据 `spec` 装配请求行 / 查询串 / 体并发出 → 读完整响应 → 关闭连接。
    ///
    /// 形态纪律（F-3）：单连接、单请求、读完即关——**无**连接复用、**无**保活、**无**重试
    /// （含 `409` 不自动重试，§3.5/L-5）。观测到 0 次或 ≥2 次连接即违反 F-3。
    ///
    /// 失败语义（fail-closed，公理二，L-2/L-7）：
    /// - 连接失败（`control.sock` 缺失 / 无权连 / daemon 未监听）/ 连接或读取超时 / 协议
    ///   读失败 → 一律映射 [`CliError::DaemonUnreachable`]，**绝不**回退本地策略 / 缓存
    ///   （结构上无 store/secrets 依赖即无可回退路径，L-2）。
    /// - daemon 应答（含 `409`/4xx/5xx）→ 原样以 [`HttpResponse`]（状态 + 体）交还，由上层
    ///   据状态分流呈现；传输层**不**在此自动重试覆盖（L-5）、**不**本地补偿写失败（L-7）。
    pub async fn round_trip(&self, spec: &RequestSpec) -> Result<HttpResponse, CliError> {
        // 装配本次往返的请求（方法 / origin-form 路径+查询 / 体）。任何装配失败一律 fail-closed
        // 为 daemon 不可达——绝不静默成功、绝不本地回退（结构上无 store/secrets 回退路径，L-2）。
        let request = build_request(spec)?;

        // 每次往返新建一条一次性短命连接——**无**连接池 / 复用（绕开
        // `hyper_util::client::legacy::Client` 的池化保活，避免 ≥2 次连接误判，F-3）。连接阶段
        // 失败（入口缺失 / 无权连 / daemon 未监听）/ 连接超时 → DaemonUnreachable。
        //
        // 本地 IPC 传输按平台分流（行为等价、仅底层 socket 不同）：
        // - unix：连 `control.sock`（UDS），权限边界为 0600 + SO_PEERCRED（部署前置）。
        // - windows：连 127.0.0.1 本地回环 TCP（原生 Windows 无 UDS/SO_PEERCRED，安全模型
        //   降级为 token-only + 仅回环可连，端口取自 `POSTERN_CONTROL_PORT`，缺省 127.0.0.1:7878）。
        #[cfg(unix)]
        let (mut sender, conn) = {
            let stream = match with_timeout(
                self.timeout,
                tokio::net::UnixStream::connect(&self.socket_path),
            )
            .await
            {
                Some(Ok(stream)) => stream,
                _ => return Err(CliError::DaemonUnreachable),
            };
            match with_timeout(
                self.timeout,
                hyper::client::conn::http1::handshake(TokioIo::new(stream)),
            )
            .await
            {
                Some(Ok(pair)) => pair,
                _ => return Err(CliError::DaemonUnreachable),
            }
        };
        #[cfg(windows)]
        let (mut sender, conn) = {
            let stream = match with_timeout(
                self.timeout,
                tokio::net::TcpStream::connect(control_tcp_addr()),
            )
            .await
            {
                Some(Ok(stream)) => stream,
                _ => return Err(CliError::DaemonUnreachable),
            };
            match with_timeout(
                self.timeout,
                hyper::client::conn::http1::handshake(TokioIo::new(stream)),
            )
            .await
            {
                Some(Ok(pair)) => pair,
                _ => return Err(CliError::DaemonUnreachable),
            }
        };

        // 连接驱动任务：推进这条连接的 I/O。读完整响应、sender 落地后它自然完成；不保活、不重连。
        let driver = tokio::spawn(async move {
            let _ = conn.await;
        });

        // 发出**恰好一次**请求并读取完整响应——无重试（含 409 不自动重试，L-5）、无补偿（写
        // 5xx 不本地补写 / 回滚 / 重试，L-7）。发送 / 读取失败或超时 → DaemonUnreachable。
        let result = round_trip_once(&mut sender, &self.timeout, request).await;

        // 落 sender → 连接驱动结束 → 连接关闭（命令结束即关，不留后台保活连接，F-3）。
        drop(sender);
        driver.abort();

        result
    }
}

/// 把 [`RequestSpec`] 装配成一次往返的 `hyper::Request`：方法、origin-form 请求目标
/// （`/v1/...?k=v`，真实请求行只含 6.5 路径 + 查询，**不**含 UDS host 段）、体（仅写端点）。
fn build_request(spec: &RequestSpec) -> Result<Request<Full<Bytes>>, CliError> {
    let target = request_target(spec);
    let body = request_body(spec)?;

    let mut builder = Request::builder().method(spec.method.as_str()).uri(target);
    if spec.body.is_some() {
        builder = builder.header(hyper::header::CONTENT_TYPE, "application/json");
    }

    builder.body(body).map_err(|_| CliError::DaemonUnreachable)
}

/// origin-form 请求目标：`path_template` 加查询串（缺键则不带，F-6）。键已 `BTreeMap` 排序，
/// 顺序稳定；消费侧按键比对（F-6 不要求顺序）。
fn request_target(spec: &RequestSpec) -> String {
    let pairs = spec.query.clone().into_pairs();
    if pairs.is_empty() {
        return spec.path_template.clone();
    }
    let query = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{}", spec.path_template, query)
}

/// 请求体：写端点把命令载荷 + 期望 `version`（仅 `Some` 时）序列化为 JSON；读端点空体。
/// `version` 只透传调用方供入值，传输层不自造（F-7）。
fn request_body(spec: &RequestSpec) -> Result<Full<Bytes>, CliError> {
    let write = match &spec.body {
        Some(write) => write,
        None => return Ok(Full::new(Bytes::new())),
    };

    let mut map = serde_json::Map::new();
    for (key, value) in &write.fields {
        map.insert(key.clone(), serde_json::Value::String(value.clone()));
    }
    if let Some(version) = write.version {
        map.insert("version".to_string(), serde_json::Value::from(version));
    }

    let bytes = serde_json::to_vec(&serde_json::Value::Object(map))
        .map_err(|_| CliError::DaemonUnreachable)?;
    Ok(Full::new(Bytes::from(bytes)))
}

/// 在已握手的连接上发**一次**请求、读**完整**响应体，返回状态 + 体字节。
/// 发送 / 读取失败或超时一律 fail-closed 为 [`CliError::DaemonUnreachable`]（公理二，L-2）。
async fn round_trip_once(
    sender: &mut hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    timeout: &Duration,
    request: Request<Full<Bytes>>,
) -> Result<HttpResponse, CliError> {
    let response = match with_timeout(*timeout, sender.send_request(request)).await {
        Some(Ok(response)) => response,
        _ => return Err(CliError::DaemonUnreachable),
    };

    let status = response.status().as_u16();

    let collected = match with_timeout(*timeout, response.into_body().collect()).await {
        Some(Ok(collected)) => collected,
        _ => return Err(CliError::DaemonUnreachable),
    };
    let body = collected.to_bytes().to_vec();

    Ok(HttpResponse { status, body })
}

/// `tokio::time::timeout` 的轻封装：超时返回 `None`，完成返回 `Some(inner)`。调用方把
/// `None`（超时）与 `Some(Err(..))`（I/O / 协议失败）统一映射 DaemonUnreachable（§3.9）。
async fn with_timeout<F: std::future::Future>(duration: Duration, future: F) -> Option<F::Output> {
    tokio::time::timeout(duration, future).await.ok()
}

/// （windows）控制面本地回环 TCP 连接地址：取自 `POSTERN_CONTROL_PORT`，缺省 `127.0.0.1:7878`。
///
/// 原生 Windows 无 UDS / SO_PEERCRED，本地 IPC 降级为 127.0.0.1 回环 TCP——daemon 的控制面
/// 监听 127.0.0.1:<port>，cli 据同一约定连接。安全模型为 token-only + 仅回环可连（与 daemon
/// 端 cfg(windows) 监听/认证分支一致）。环境变量解析失败 / 未设即落缺省（缺省与 daemon 端一致）。
#[cfg(windows)]
fn control_tcp_addr() -> String {
    std::env::var("POSTERN_CONTROL_PORT").unwrap_or_else(|_| "127.0.0.1:7878".to_string())
}
