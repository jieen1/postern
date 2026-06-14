//! 外壳服务端子域（模块文档 06 §8.12）。
//!
//! 把 axum（HTTP）/ MCP 外壳挂在 data.sock 上：在 listener 层经 SO_PEERCRED
//! （tokio UnixStream::peer_cred 安全 API）采集对端 (uid,gid) 构造 ConnOrigin，并把请求
//! 装箱为 NormalizedRequest 交给数据面内核。外壳只搬运不解释、自身不做安全决策。
//!
//! ConnOrigin 的字面构造点唯一在本子域（契约 SEC_CONSTRUCTION_SITES 仅放行
//! crates/postern-daemon/src/shells/ 下的字面变体）；其余模块经 Origin 别名读/解构。
//!
//! 两个 Router（HTTP / MCP）共挂 data.sock、共用**同一注入集**（[`DataPlane`]）与**同一
//! 装箱/提交入口**（[`box_request`]→`Kernel::submit`）。注入集刻意不含 PolicyRepo / vault
//! 句柄（L-2 / B-2：控制面写句柄与机密句柄绝不进数据面注入集合）。MCP 暴露**编译期固定**
//! 的动词工具面（[`MCP_TOOLS`]），与授权无关。`postern_surface` 只读当前快照的授权投影
//! （[`surface`]），**绝不** `Adapter::discover`、绝不触后端资源（F-5）。
//!
//! 本波次为骨架：listener 与 http/mcp 外壳子模块声明 + 共享装箱/工具面/投影/非法出口桩。

pub mod http;
pub mod listener;
pub mod mcp;
pub mod serve;

use std::sync::Arc;

use postern_core::domain::{
    Capability, GrantAction, PolicySnapshot, PresentedCredential, PrincipalId, ResourceCode,
};
use postern_core::plugin::sanitize::{SanitizedResponse, Sanitizer};
use postern_core::plugin::RawResponse;
use postern_core::request::{ConnOrigin, Intent, NormalizedRequest};

use crate::kernel::Kernel;

/// 数据面注入集（两个 Router 共挂 data.sock 时共享的唯一句柄束）。
///
/// 刻意只承载数据面所需句柄：求值内核（已内含求值器/适配器/建连缝/审计/脱敏）、当前策略
/// 快照的只读投影源（供 `postern_surface`）、出口脱敏器（供协议非法请求的安全出口）。
///
/// **红线 7.2-2 / L-2 / B-2**：PolicyRepo 写句柄与 vault/机密句柄**绝不**出现在本结构——
/// 控制面写句柄独立持有（control 子域），机密句柄只在 secrets 面/connpool 建连缝内一次性
/// 物化。本结构无任何 PolicyRepo / UnlockedVault / CredentialProvider 字段（编译期事实）。
pub struct DataPlane {
    /// 数据面求值内核（唯一提交入口 `submit` 的持有者）。
    kernel: Arc<Kernel>,
    /// 当前策略快照的只读投影源——`postern_surface` 只读此投影，绝不触后端（F-5）。
    snapshot: Arc<PolicySnapshot>,
    /// 出口脱敏器：协议非法 4xx 的安全出口字节亦过同一 Sanitizer（F-10 / L-4）。
    sanitizer: Arc<dyn Sanitizer>,
}

impl DataPlane {
    /// 由 boot 装配点注入数据面句柄束（绝不接受 PolicyRepo / vault 句柄）。
    pub fn new(
        kernel: Arc<Kernel>,
        snapshot: Arc<PolicySnapshot>,
        sanitizer: Arc<dyn Sanitizer>,
    ) -> Self {
        Self {
            kernel,
            snapshot,
            sanitizer,
        }
    }

    /// 数据面提交：把已采集来源 + 协议原文装箱为 `NormalizedRequest` 交内核 `submit`。
    ///
    /// 两个 Router 的 submit 路径都收敛到此处——同一装箱 + 同一内核入口。`origin` 由
    /// listener 层经 SO_PEERCRED 采集后**按值**传入（绝不采信请求体自报字段，B-2）。
    pub async fn submit(
        &self,
        presented: PresentedCredential,
        origin: ConnOrigin,
        resource: ResourceCode,
        intent_bytes: Vec<u8>,
    ) -> Result<SanitizedResponse, postern_core::decision::DenyResponse> {
        let req = box_request(presented, origin, resource, intent_bytes);
        self.kernel.submit(req).await
    }

    /// `postern_surface`：当前快照里该 principal 已授权对象的子集投影。
    ///
    /// 只读快照投影（[`surface`]）；**不** `Adapter::discover`、不建连、不触后端（F-5）。
    pub fn surface(&self, principal: PrincipalId) -> Vec<SurfaceEntry> {
        surface(&self.snapshot, principal)
    }

    /// 协议语法非法请求的安全出口（4xx）：常量安全文案的字节过同一 Sanitizer（F-10 / L-4）。
    pub fn invalid_request_egress(&self) -> SanitizedResponse {
        invalid_request_egress(self.sanitizer.as_ref())
    }
}

/// MCP 暴露的**编译期固定**动词工具面（§8 F-4）。
///
/// 与授权无关、不随授权动态增减：无论 principal 被授予多少能力，工具集合恒为这八个固定
/// 动词工具。鉴权发生在 `submit` 之后的内核求值，而非工具面裁剪。
pub const MCP_TOOLS: [&str; 8] = [
    "postern_grants",
    "postern_query",
    "observe",
    "mutate",
    "execute",
    "manage",
    "destroy",
    "postern_surface",
];

/// 协议语法非法请求的**常量安全文案**（§8 F-10 / L-4）。
///
/// 不随输入变化、不回显请求字节、不泄露内部细节；其字节跨边界前仍过同一 Sanitizer。
pub const INVALID_REQUEST_SAFE_MESSAGE: &str = "invalid request";

/// `postern_surface` 投影的一项：principal 已授权的 (资源, 动词) 坐标（授权能力投影，
/// 非后端探测结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceEntry {
    /// 已授权资源代号（始终是代号，绝非真实地址）。
    pub resource: ResourceCode,
    /// 该坐标上已授权的动词。
    pub capability: Capability,
}

/// 装箱（步骤 [0]）：把已采集来源 + 协议原文装为 `NormalizedRequest`。
///
/// 这是外壳层把「出示物 + 来源 + 资源代号 + 原始 Intent」装箱的**唯一**入口（HTTP / MCP
/// 两路共用）。`origin` 由 listener 采集后按值传入——请求体自报来源字段绝不在此被读取
/// （B-2）。装箱只搬运不解释：`intent_bytes` 原样裹进 `Intent`，绝不预解析 SQL/协议
/// （公理七）。
pub fn box_request(
    presented: PresentedCredential,
    origin: ConnOrigin,
    resource: ResourceCode,
    intent_bytes: Vec<u8>,
) -> NormalizedRequest {
    // 只搬运不解释：原始字节原样裹进 Intent，绝不预解析 SQL/协议（公理七）。来源恒为 listener
    // 传入者，请求体自报来源字段不在此读取（B-2）。
    NormalizedRequest {
        presented,
        origin,
        resource,
        intent: Intent::new(intent_bytes),
    }
}

/// `postern_surface` 的投影实现：从快照取该 principal 已授权对象的子集（F-5）。
///
/// 纯函数，只读 `snapshot.grants[principal]` 的 (资源, 动词) 坐标；确定性（BTreeMap 序）。
/// **绝不** `Adapter::discover`、绝不建连、绝不触后端资源——投影是「当前快照授权能力」的
/// 子集，不是「后端实有能力」的探测。
pub fn surface(snapshot: &PolicySnapshot, principal: PrincipalId) -> Vec<SurfaceEntry> {
    // 纯读快照：只取该 principal 自身授权世界里 action=Allow 的 (资源, 动词) 坐标。Escalate
    // 格在审批关闭时折叠为 deny，非已授权能力，故不入投影。无授权 principal → 空（不泄露存在
    // 性）。绝不 Adapter::discover、绝不建连、绝不触后端（F-5）。BTreeMap 迭代序稳定（确定性）。
    let per_principal = match snapshot.grants.get(&principal) {
        Some(cells) => cells,
        None => return Vec::new(),
    };
    per_principal
        .iter()
        .filter(|(_, cell)| cell.action == GrantAction::Allow)
        .map(|((resource, capability), _)| SurfaceEntry {
            resource: resource.clone(),
            capability: *capability,
        })
        .collect()
}

/// 协议语法非法请求的安全出口字节：常量安全文案过同一 Sanitizer（F-10 / L-4）。
///
/// 协议非法 4xx 不是数据面求值结果，但其出口字节仍须过与正常/ deny 出口**同一** Sanitizer
/// （统一出口不变量）；文案恒为 [`INVALID_REQUEST_SAFE_MESSAGE`]，不回显请求字节。
pub fn invalid_request_egress(sanitizer: &dyn Sanitizer) -> SanitizedResponse {
    // 协议非法 4xx 不旁路裸传：常量安全文案字节过与正常/deny 出口同一 Sanitizer（统一出口
    // 不变量，F-10 / L-4）。无字段级遮罩声明源，传空 mask 集——只保证「确实过了同一脱敏」。
    sanitizer.scrub(
        RawResponse {
            payload: INVALID_REQUEST_SAFE_MESSAGE.as_bytes().to_vec(),
        },
        &[],
    )
}
