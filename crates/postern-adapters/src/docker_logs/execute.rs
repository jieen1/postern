//! docker_logs 执行：经只读日志端点 / 探针取数（§3.4）。
//!
//! 把 `Intent` 的取数参数（容器选择 + `since` / `tail` / `follow`）翻译为对**只读日志
//! 端点**的请求——两种取数形态：①直连远端已暴露的只读 API；②经远端只读探针取数（探针
//! 把只读下沉到远端进程边界，能力面恒不含写动词）。无论哪种形态，适配器侧**只发取数、
//! 不发任何写 / 控制动作**（封闭枚举无写变体可表达，无写路径可走）。`follow` 形态产
//! **流式** `RawResponse`，由内核出口按流式模型脱敏（§3.4）；适配器不自建无界缓冲、不
//! spawn 后台任务，背压交还内核（§3.7）。

use postern_core::error::ExecError;
use postern_core::plugin::{Channel, RawResponse};
use postern_core::request::Intent;

use crate::docker_logs::intent::DockerLogsRequest;

/// 步骤[8] 执行（§3.4）：经 `Channel` 向只读端点 / 探针发取数请求，回未脱敏字节流。
///
/// 负载先解码为封闭枚举（结构上只能是取容器日志的只读取数请求，无写变体可表达），故
/// 翻译产物**只可能**是取数请求、无任何写 / 控制动作。线协议客户端语义（直连只读 API /
/// 远端探针、`follow` 流式回传）由 docker_logs 实现波次在容器集成测下填实（§3.4 / F-9）；
/// 在此之前对任何输入 **fail-closed**（公理二）——不 panic、不伪造日志字节、不自建缓冲。
pub async fn execute(ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError> {
    let _ = ch;

    // 解码即印证：负载唯一变体是只读取数请求，无写 / 控制动作可被翻译下发。
    let DockerLogsRequest::Logs(_logs) =
        DockerLogsRequest::decode(intent.payload()).map_err(|_| ExecError::ProtocolViolation)?;

    // 线协议取数 / 流式回传未接，fail-closed 不发任何请求、不回伪造字节（公理二）。
    Err(ExecError::ExecutionFailed)
}
