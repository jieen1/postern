//! MCP 外壳服务端（模块文档 06 §8.12）。
//!
//! 把固定动词工具面（MCP）挂在 data.sock 上：每个工具调用同样采集 ConnOrigin、装箱为
//! NormalizedRequest 交给数据面内核，回写结构化结果。工具面是**编译期固定**动词集合
//! （[`crate::shells::MCP_TOOLS`]），与授权无关、不随授权动态增减（F-4）；外壳只搬运不
//! 解释、自身不做安全决策。`postern_surface` 只读快照投影、绝不 `Adapter::discover`（F-5）。
//!
//! 装箱收敛到共享入口 [`crate::shells::box_request`]：MCP 与 HTTP 同一逻辑请求装出**字节
//! 等价**的 `NormalizedRequest`（F-4）。工具调用参数里的自报来源字段绝不被读取（B-2）。
//!
//! 本波次为骨架：外壳装配入口桩 + MCP 工具调用归一化桩，零工具逻辑。

use postern_core::request::{ConnOrigin, NormalizedRequest};

/// MCP 工具调用的协议 DTO（工具入参反序列化目标）。
///
/// `tool` 必为 [`crate::shells::MCP_TOOLS`] 之一（固定动词面）。**刻意无来源字段**：来源
/// 是网关侧观测事实（SO_PEERCRED），绝不取自工具入参（B-2）。
#[derive(Debug, serde::Deserialize)]
pub struct McpToolCall {
    /// 被调用的固定动词工具名（须属 `MCP_TOOLS`）。
    pub tool: String,
    /// 出示物种类（鉴权器选型键）。
    pub auth_kind: String,
    /// 出示物秘密字节。
    #[serde(default)]
    pub secret: Vec<u8>,
    /// 目标资源代号。
    pub resource: String,
    /// 协议原文意图字节（原样装箱，外壳绝不预解析）。
    #[serde(default)]
    pub intent: Vec<u8>,
}

impl McpToolCall {
    /// 把 MCP 工具调用 + listener 采集的来源归一化为 `NormalizedRequest`（步骤 [0]）。
    ///
    /// 收敛到共享 [`box_request`](crate::shells::box_request)：与 HTTP 路径**同一**装箱入口，
    /// 故同一逻辑请求经 HTTP / MCP 装出字节等价的 `NormalizedRequest`（F-4）。来源按值由
    /// listener 传入（DTO 无来源字段，B-2）；intent 原样裹入，绝不预解析（公理七）。
    pub fn normalize(self, origin: ConnOrigin) -> NormalizedRequest {
        crate::shells::box_request(
            postern_core::domain::PresentedCredential::new(self.auth_kind, self.secret),
            origin,
            postern_core::domain::ResourceCode::new(self.resource),
            self.intent,
        )
    }
}

/// MCP 工具面：编译期固定的八个动词工具名（与授权无关，F-4）。
///
/// 直接交还 [`crate::shells::MCP_TOOLS`]——工具集合不随 principal 授权变化（鉴权在 submit
/// 之后的内核求值，而非工具面裁剪）。
pub fn tools() -> &'static [&'static str] {
    &crate::shells::MCP_TOOLS
}

/// 启动挂载于 data.sock 的 MCP 外壳服务（占位）。
pub fn serve() -> crate::error::Result<()> {
    todo!()
}
