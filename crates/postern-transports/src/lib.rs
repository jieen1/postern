//! Transport 插件：把「一种远端接入方式」抽象为「本地可用通路」的单通路域
//! （docs/modules/04-postern-transports.md；详细设计 4.4 / 8.7）。
//!
//! 本 crate 实现 `core` 中**定义**的 `#[async_trait] Transport` trait，并在
//! `core` 定义的 `Channel` 抽象上承载健康与关闭语义；只管一条通路从建立到关闭
//! 的协议级机制，不持有凭据、不池化、不做通路间生命周期决策（那是 daemon 连接
//! 管理层）。架构上仅依赖 `core`（契约 `ARCH_FORBIDDEN_EDGES`：transports ↛
//! store/secrets/adapters）。
//!
//! 子模块布局：
//! - [`error`]：本 crate 脱敏 `TransportError` 词汇基底（跨边界前已脱敏，§5.1）。
//! - [`health`]：通路健康事实视图与单调死活状态机（§3.4，被动呈现不主动推送）。
//! - [`pump`]：本地端点 ⇆ 底层隧道的双向字节桥接泵（§3.2，只搬字节不解析协议）。
//! - [`keepalive`]：长连接型保活——心跳 + 协议级续约状态机 + 注入时钟（§3.3）。
//! - [`chan`]：`Channel` 三件套的承载——本地端点句柄 + 关闭/取消触点（§3.1 / §3.5）。
//! - [`direct`]：非隧道直连（tokio TcpStream），最薄的一层（§3.2）。
//! - [`ssh`]：SSH 隧道形态（feature 门控，具体通路库后续引入）。
//! - [`ssm`]：SSM 端口转发形态（feature 门控，有时限会话续约的主要使用方）。

pub mod chan;
pub mod direct;
pub mod error;
pub mod health;
pub mod keepalive;
pub mod pump;

#[cfg(feature = "ssh")]
pub mod ssh;

#[cfg(feature = "ssm")]
pub mod ssm;
