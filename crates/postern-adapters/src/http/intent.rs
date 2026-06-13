//! http `Intent` 负载结构与动词工具 `request` schema（承诺级签名，§3.6）。
//!
//! 负载携 `(method, path, headers, body)`，既能被 `classify` 反查到声明的 `Capability`、
//! 又能被 `execute` 直接经 `Channel` 忠实转发（同一份原文，§3.4「忠实搬运」）。负载须可
//! 序列化往返且逐字段稳定（F-12），故派生 `serde`。
//!
//! **声明动词工具映射随负载搬运**：HTTP 没有 SQL 那样的协议级语义可解析，`classify` 据
//! 接入时为该资源声明的 `(method × path → Capability)` 表反查（§3.1）。`Adapter::classify`
//! 入参仅 `&Intent`，故该声明映射 [`declared_routes`](HttpRequest::declared_routes) 随负载
//! 一同搬运、由外壳层忠实装箱（§3.6「外壳只装箱、不解释」）；解释权唯一归适配器。

use serde::{Deserialize, Serialize};

use postern_core::domain::Capability;

/// http 协议 `Intent` 负载 = MCP 动词工具（`postern_query` / `postern_mutate` 等）的
/// `request` schema（§3.6）。携 `(method, path, headers, body)` 原文 + 该资源声明的动词
/// 工具映射；`classify` 据映射反查 `Capability`、`execute` 据原文忠实转发。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    /// HTTP 方法（`GET` / `POST` / `PUT` / `DELETE` …）——归类维度之一（§3.1）。
    pub method: String,
    /// 请求路径——经 `classify` 规范化为 `route:<path>` 对象（§3.1）、归类维度之一。
    pub path: String,
    /// 请求头（忠实转发，`execute` 不改写；凭据由连接管理层建连注入、不在此，§3.4）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    /// 请求体原始字节（忠实转发，§3.4）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<u8>,
    /// 该资源接入时声明的 `(method × path → Capability)` 动词工具映射（§3.1）。
    /// `classify` 据此白名单反查；未落任何声明形态 → `Err`（未声明即不可归类）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared_routes: Vec<RouteVerb>,
}

/// 一条声明的动词工具映射项：`(method, path) → capability`（§3.1）。
///
/// 归类档位**完全由声明决定**、不做任何启发式推断（`engine_enforced=false`，误归不会被
/// 第二道防线拦下，故必须保守）。`capability` 以其规范小写动词名（[`Capability::as_str`]）
/// 入序列化形态——`Capability` 本身只 `Serialize` 不 `Deserialize`，故 wire 形态用串、由
/// [`parse_capability`] 解回；未知动词名 → `None`（解析失败即不可归类，fail-closed）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteVerb {
    /// 声明的 HTTP 方法。
    pub method: String,
    /// 声明的请求路径。
    pub path: String,
    /// 命中该 `(method, path)` 形态时归入的动词（规范小写名，如 `"mutate"`）。
    pub capability: String,
}

/// `http_route` 细则的 `spec` 负载形态（§3.2）：路由白名单，可对读 / 写分别声明不同路径。
///
/// 白名单按动词键入——`classify` 已把 `method` 映射为 `ci.capability`，故白名单判定按
/// `(capability, path)` 全称量化（请求触达的每个路由都须落在对应动词的白名单内，§3.2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRouteSpec {
    /// 路由白名单项集合。请求 `(ci.capability, route:<path>)` 须命中其一（§3.2 全称量化）。
    pub routes: Vec<RoutePattern>,
}

/// 路由白名单的单条声明：`(capability, path)`（§3.2，读 / 写分路声明的落点）。
///
/// `capability` 以规范小写动词名入 wire 形态（理由同 [`RouteVerb`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutePattern {
    /// 该路由允许的动词（读 / 写分路：同路径可对不同动词声明；规范小写名）。
    pub capability: String,
    /// 该路由允许的请求路径。
    pub path: String,
}

impl HttpRequest {
    /// 把负载编码为 [`postern_core::request::Intent`] 的原始字节（外壳层装箱形态，§3.6）。
    ///
    /// `classify` / `execute` 看到的是同一份原始负载（§3.6「同一份原始负载」）；序列化
    /// 形态即 MCP 动词工具对外 `request` schema（F-12 往返基准）。
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// 从 [`postern_core::request::Intent`] 的原始字节解码负载（`classify` / `execute`
    /// 的入口，§3.6）。解码失败由调用方翻译为 `ClassifyError::ParseFailed`（§3.1）。
    pub fn decode(payload: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(payload)
    }
}

impl HttpRouteSpec {
    /// 从 `ConstraintSpec.spec` 的原始 JSON 串解码白名单（§3.2）。解码失败由 `check`
    /// 翻译为 `ConstraintError::InvalidSpec`（畸形 spec 即拒，L-7）。
    pub fn decode(spec: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(spec)
    }
}

/// 把规范小写动词名（wire 形态）解回 [`Capability`]——[`Capability::as_str`] 的逆。
///
/// 未知动词名 → `None`：解析失败即不可可靠归类（fail-closed，公理二）。穷尽匹配六动词、
/// 无 `_ =>` 通配，新增动词未在此登记则编译失败（与 core 穷尽 `match` 同纪律）。
pub fn parse_capability(name: &str) -> Option<Capability> {
    match name {
        "observe" => Some(Capability::Observe),
        "query" => Some(Capability::Query),
        "mutate" => Some(Capability::Mutate),
        "execute" => Some(Capability::Execute),
        "manage" => Some(Capability::Manage),
        "destroy" => Some(Capability::Destroy),
        _ => None,
    }
}
