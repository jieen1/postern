//! postgres `Intent` 负载结构与 MCP 动词工具 `request` schema（骨架占位，§3.6 F-12）。
//!
//! 本模块定义 postgres 协议 `Intent` 负载的具体结构（承载语句原文 + 可选绑定参数），
//! 以及 `postern_query` / `postern_observe` / `postern_mutate` / `postern_destroy` 等动词
//! 工具向 Agent 暴露的 `request` 形态——四个动词共用同一份 postgres `request` schema
//! [`PgRequest`]，归类档由 [`super::classify`] 自语句树推定，而非由 Agent 选哪个工具决定。
//!
//! 负载是 classify / execute 两方法的共同入参：`classify` 阶段产出的语法树只用于定档、
//! **绝不改写**负载，`execute` 用的仍是负载原文（§3.4「绝不重新解析或改写」），杜绝
//! 「解析时与执行时看到不同请求」的二义。
//!
//! 负载须可序列化往返且逐字段稳定（F-12 判定基准：序列化 → 反序列化往返后逐字段相等）；
//! 故负载类型派生 `serde` 的 `Serialize`/`Deserialize`，并派生 `PartialEq` 供往返断言。
//!
//! 入口 [`PgRequest::from_payload`] 把 `core::Intent.payload()` 的不透明字节经 `serde_json`
//! 反序列化为本地负载；解析失败一律收敛为 [`ClassifyError::ParseFailed`]（公理二，
//! fail-closed），绝不吞错放行。
//!
//! 红线：本负载**绝不命名 / 构造任何机密类型**（`ResolvedTarget` / `ResourceCredential` /
//! `PresentedCredential`）、不承载凭据或真实地址——它只装 Agent 提交的请求原文与参数；
//! 凭据与通路由连接管理层在建连边界注入，适配器侧拿不到、也不构造（§3.6、§4 边界）。

use serde::{Deserialize, Serialize};

use postern_core::error::ClassifyError;

/// postgres 动词工具的 `request` 负载（§3.6 F-12）。
///
/// `postern_query` / `postern_observe` / `postern_mutate` / `postern_destroy` 四工具共用此
/// schema：承载一条语句原文 [`PgRequest::statement`] 与可选的绑定参数 [`PgRequest::params`]。
/// 归类（动词与对象）由 [`super::classify`] 解读语句树推定，**不**取决于 Agent 调了哪个
/// 动词工具——工具名只是分组入口，真正的危险度判定收敛在适配器一处（§3.1 不降级根因）。
///
/// 派生 `Serialize`/`Deserialize` 是 F-12 对外契约：经外壳层装箱、跨进程搬运后须能无损
/// 反序列化回适配器，序列化 → 反序列化往返后**逐字段相等**是其正确性底线。`PartialEq`
/// 供该往返断言；`Clone`/`Debug` 供调用方持有与诊断（`Debug` 仅在适配器内部，不外泄；
/// 跨边界回显由内核出口 `Sanitizer` 统一处置，§3.7 机密红线）。
///
/// 仅派生 `PartialEq`（不派生 `Eq`）：参数以 `serde_json::Value` 承载，其含 `f64` 故只满足
/// `PartialEq`；F-12 往返判定基准也只需逐字段 `==` 相等。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PgRequest {
    /// 待执行的单条语句原文（运行期字节，源码不含其字面量）。`classify` 解析它定档、
    /// `execute` 忠实回放它，二者看到同一份原文。
    pub statement: String,
    /// 可选绑定参数（位置参数序列）。缺省为空；F-12 往返须保持其顺序与值逐一稳定。
    /// 以 `serde_json::Value` 承载异构标量，序列化往返不丢类型。
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
}

impl PgRequest {
    /// 解析入口（§3.6）：把 `core::Intent.payload()` 的不透明字节反序列化为本地负载。
    ///
    /// 经 `serde_json::from_slice` 解析；任何解析失败（非法 JSON / 字段缺失 / 类型不符）
    /// 一律映射为 [`ClassifyError::ParseFailed`]——「解析不了」等价于「不可归类」，由内核
    /// 翻译为 fail-closed deny（公理二）。绝不 `.ok()` / `unwrap_or_default` 吞错放行。
    ///
    /// 经 `serde_json::from_slice` 整段解析（拒绝尾随垃圾）；任何 `Err` 收敛为
    /// [`ClassifyError::ParseFailed`]，原始解析错误被丢弃（不外泄、不分流到其他变体）。
    ///
    /// 负载 schema 恒为对象形态（`{statement, params}`）：先解析为 [`serde_json::Value`]
    /// 并要求其为对象，再按字段反序列化为 [`PgRequest`]。这排除 derive 默认接受的
    /// 序列（数组）形态——数组负载非本 schema，一律 fail-closed 为 `ParseFailed`。
    pub fn from_payload(payload: &[u8]) -> Result<PgRequest, ClassifyError> {
        let value: serde_json::Value =
            serde_json::from_slice(payload).map_err(|_| ClassifyError::ParseFailed)?;
        if !value.is_object() {
            return Err(ClassifyError::ParseFailed);
        }
        serde_json::from_value(value).map_err(|_| ClassifyError::ParseFailed)
    }
}
