//! 本 crate 传输面错误词汇基底 —— L-7 红线 7.2-1（跨边界错误先脱敏）的唯一收口。
//!
//! `open` / 保活 / 关闭任一步失败一律收敛为脱敏后的 `core::TransportError`
//! （ConnectFailed / HandshakeFailed / ChannelClosed / CloseFailed），跨 crate
//! 边界抛出**前**必须已脱敏为不含真实地址 / 凭据的错误码（§5.1，第 7 节红线 1）。
//!
//! 设计纪律（雷区收口）：
//! - 本 crate **不**重定义任何跨边界 thiserror 错误枚举与 `core::TransportError`
//!   竞争 —— `core::TransportError` 是唯一对外错误类型（每 crate 一个 thiserror
//!   枚举的纪律由 core 承载）。本模块只定义一个 **crate 内部、不越边界** 的低层
//!   失败种类载体 [`InnerFault`]，并提供 [`sanitize`] 把它映射为 `TransportError`。
//! - [`InnerFault`] 是诊断载体：它可在 crate 内部携带真实地址明文（白名单字段，仅
//!   供本地 tracing 用），但**绝不**出现在任何 `pub` 对外签名的**返回**位置。它只
//!   作为 [`sanitize`] 的**入参**存在，经此函数被丢弃，决不随返回值越出边界。
//! - 承接 tracing 的地址字段**不实现 / 不派生 `Serialize`**；其 `Debug` 也**绝不**
//!   在 [`sanitize`] 路径上被拼进 `TransportError`（脱敏输出只承载常量化错误码判别）。

use postern_core::error::TransportError;

/// crate 内部低层失败的**种类**判别（§3.6 错误处理与传播）。
///
/// 四类与 `core::TransportError` 四变体一一对应的失败语义：连接 / 握手 / 通路死亡 /
/// 关闭。这是 [`sanitize`] 的输入域，**不**是对外错误类型，**绝不**出现在任何 `pub`
/// 对外签名的返回位置。
///
/// 映射完整性（§8 L-7）：连接类 → `ConnectFailed`、握手类 → `HandshakeFailed`、
/// 通路死亡类 → `ChannelClosed`、关闭类 → `CloseFailed`；逐类有对应分支、无 `_ =>`
/// 吞配的丢失语义。
// NB: 故意**不**派生 `Serialize` —— 诊断载体不得被序列化越界（§7-2 红线）。
#[derive(Debug)]
pub enum FaultKind {
    /// 连接类底层失败（如 `connection refused`、不可达）→ 映射为 `ConnectFailed`。
    Connect,
    /// 握手 / 会话协商类底层失败 → 映射为 `HandshakeFailed`。
    Handshake,
    /// 已建通路死亡类（保活判定僵死、桥接泵退出、对端 RST）→ 映射为 `ChannelClosed`。
    Channel,
    /// 关闭 / 释放底层隧道时报错 → 映射为 `CloseFailed`。
    Close,
}

/// crate 内部诊断载体 —— 携带白名单诊断细节供**本地 tracing**，**不越边界**。
///
/// `detail` 可承载底层 IO 错误串 / 真实地址明文（如 `connection refused to
/// 10.0.3.17`），仅用于本地 tracing 的白名单字段诊断；[`sanitize`] 的输出**绝不**
/// 内嵌该字段。本类型只作 [`sanitize`] 入参，**绝不**作任何 `pub` 对外返回。
// NB: `detail` 是诊断明文 —— 全类型故意**不**派生 / 实现 `Serialize`（§7-2）。
#[derive(Debug)]
pub struct InnerFault {
    /// 失败种类判别（决定 [`sanitize`] 映射到的 `TransportError` 变体）。
    pub kind: FaultKind,
    /// 白名单诊断明文（可含真实地址 / 底层错误串），仅供本地 tracing；脱敏时丢弃。
    pub detail: String,
}

impl InnerFault {
    /// 连接类底层失败，携带白名单诊断明文。
    pub fn connect(detail: impl Into<String>) -> Self {
        Self {
            kind: FaultKind::Connect,
            detail: detail.into(),
        }
    }

    /// 握手 / 会话协商类底层失败，携带白名单诊断明文。
    pub fn handshake(detail: impl Into<String>) -> Self {
        Self {
            kind: FaultKind::Handshake,
            detail: detail.into(),
        }
    }

    /// 已建通路死亡类底层失败，携带白名单诊断明文。
    pub fn channel(detail: impl Into<String>) -> Self {
        Self {
            kind: FaultKind::Channel,
            detail: detail.into(),
        }
    }

    /// 关闭 / 释放类底层失败，携带白名单诊断明文。
    pub fn close(detail: impl Into<String>) -> Self {
        Self {
            kind: FaultKind::Close,
            detail: detail.into(),
        }
    }
}

/// L-7 红线 7.2-1 的唯一脱敏收口：把 crate 内部 [`InnerFault`] 映射为 core 的
/// `TransportError`，跨边界呈现前真实地址 / 凭据 / 原始底层错误串一律丢弃。
///
/// 映射逐类显式、无 `_ =>` 吞配；输出仅承载常量化错误码判别，**绝不**插值 `detail`、
/// **绝不**拼接 `InnerFault` 的 `Debug`。
pub fn sanitize(inner: InnerFault) -> TransportError {
    // 仅取**种类**判别；`detail`（真实地址 / 凭据 / 原始底层错误串）在此丢弃，
    // 决不内插、决不拼 `InnerFault` 的 `Debug` —— 输出仅承载常量化错误码判别。
    match inner.kind {
        FaultKind::Connect => TransportError::ConnectFailed,
        FaultKind::Handshake => TransportError::HandshakeFailed,
        FaultKind::Channel => TransportError::ChannelClosed,
        FaultKind::Close => TransportError::CloseFailed,
    }
}
