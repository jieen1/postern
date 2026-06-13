//! docker_logs `Intent` 负载结构与动词工具 `request` schema（承诺级签名，§3.6）。
//!
//! 负载是一个**封闭枚举的只读取数请求**：容器选择符 + `since` / `tail` / `follow` 等
//! **只读取数参数**，形态本身不含任何写表达——没有「执行命令」「重启容器」这类变体可被
//! 构造（只读性下沉到 schema 层，§3.1）。负载须可序列化往返且逐字段稳定（F-12），故
//! 派生 `serde`。
//!
//! **恒 Observe 靠类型而非运行期判别**：本枚举**唯一**变体 [`DockerLogsRequest::Logs`]
//! 是取容器日志（只读取数）；无 `Exec` / `Restart` / `Stop` 之类写 / 控制变体可表达——
//! 「危险无从表达」而非「危险被识别」（与 SQL 白名单形态同源，§3.1）。

use serde::{Deserialize, Serialize};

/// docker_logs 协议 `Intent` 负载 = MCP `postern_observe` 动词工具的 `request` schema
/// （§3.6）。**封闭枚举，结构上无写变体可表达**——只读性下沉到类型层（§3.1）。
///
/// `#[serde(tag = "action")]` 与场景 04 §4.1 Trace ③ 的
/// `request={action:"logs", container:"app-order", tail:200}` 对齐：`action` 是 MCP
/// 动词工具暴露给 Agent 的判别标签，唯一合法值 `"logs"`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum DockerLogsRequest {
    /// 取容器日志（只读取数）——本枚举唯一变体。容器选择符 + 只读取数范围参数。
    Logs(LogsRequest),
}

/// 取容器日志的只读取数参数（容器选择符 + `since` / `tail` / `follow`）。
///
/// **全部字段都是只读取数维度**——无任何写 / 控制字段。`container` 是容器选择符，经
/// `classify` 规范化为 `container:<名>` 对象（§3.1）并供 `container_prefix` 细则判定
/// （§3.2）。`since` / `tail` / `follow` 只约束取数范围 / 流式，不改变只读性。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsRequest {
    /// 容器选择符（容器名）——经 `classify` 规范化为 `container:<名>`（§3.1）。
    pub container: String,
    /// 取数起点（如相对时间 / 时间戳）——只读取数范围，缺省取全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// 取尾部行数上界——只读取数范围，缺省取尾部默认窗口。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<u64>,
    /// 是否流式跟随——`follow=true` 产**流式** `RawResponse`（§3.4），仍是只读取数。
    #[serde(default, skip_serializing_if = "is_false")]
    pub follow: bool,
}

/// serde `skip_serializing_if` 助手：`follow=false`（缺省）不入序列化形态。
fn is_false(b: &bool) -> bool {
    !*b
}

impl DockerLogsRequest {
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
