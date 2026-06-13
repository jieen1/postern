//! HTTP 外壳服务端（模块文档 06 §8.12）。
//!
//! 把 axum router 挂在 data.sock 上：接受请求后采集 ConnOrigin（经 listener）、把协议原
//! 文装箱为 NormalizedRequest 交给数据面内核，再把内核的结构化结果回写为 HTTP 响应。
//! 外壳只搬运不解释，自身不做任何安全决策。CatchPanic 中间件保证 panic 不外泄
//! （fail-closed）：handler panic 折叠为脱敏 deny + kind=anomaly 审计（L-4）。
//!
//! 装箱收敛到共享入口 [`crate::shells::box_request`]：HTTP 与 MCP 同一逻辑请求装出**字节
//! 等价**的 `NormalizedRequest`（F-4）。请求体自报来源字段绝不被读取（B-2）。
//!
//! 本波次为骨架：外壳装配入口桩 + HTTP 提交 DTO 归一化桩，零路由逻辑。

use postern_core::request::{ConnOrigin, NormalizedRequest};

/// HTTP 提交请求的协议 DTO（请求体反序列化目标）。
///
/// **刻意无来源字段**：来源是网关侧观测事实（SO_PEERCRED），绝不取自请求体（B-2）。
/// 若客户端在体内塞入自报 uid/gid/origin，反序列化时被忽略——本 DTO 不为其留位。
#[derive(Debug, serde::Deserialize)]
pub struct HttpSubmit {
    /// 出示物种类（鉴权器选型键，步骤 [1] 据此选 Authenticator）。
    pub auth_kind: String,
    /// 出示物秘密字节（base64/原文由路由层解码；此处持已解码字节）。
    #[serde(default)]
    pub secret: Vec<u8>,
    /// 目标资源代号（始终是代号，绝非真实地址）。
    pub resource: String,
    /// 协议原文意图字节（原样装箱，外壳绝不预解析）。
    #[serde(default)]
    pub intent: Vec<u8>,
}

impl HttpSubmit {
    /// 把 HTTP DTO + listener 采集的来源归一化为 `NormalizedRequest`（步骤 [0]）。
    ///
    /// 收敛到共享 [`box_request`](crate::shells::box_request)：来源**按值**由 listener 传入
    /// （DTO 自身无来源字段，B-2）；intent 原样裹入，绝不预解析（公理七）。
    pub fn normalize(self, origin: ConnOrigin) -> NormalizedRequest {
        crate::shells::box_request(
            postern_core::domain::PresentedCredential::new(self.auth_kind, self.secret),
            origin,
            postern_core::domain::ResourceCode::new(self.resource),
            self.intent,
        )
    }
}

/// 构造挂载于 data.sock 的 HTTP 外壳 router（占位）。
pub fn router() -> axum::Router {
    todo!()
}
