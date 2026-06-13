//! http 执行：经 `Channel` 忠实转发到目标端点（骨架占位，§3.4）。
//!
//! 把 `Intent` 的 `(method, path, headers, body)` 经 `Channel` **转发**到目标 HTTP 端点，
//! 回传未脱敏的状态码 / 头 / 体。转发是**忠实搬运**——不在适配器侧改写请求语义（路径 /
//! 方法已在 `classify` + `check_constraint` 处被白名单约束）；凭据由连接管理层在建连边界
//! 注入、适配器不经手（`engine_enforced=false`，归类+细则是该请求合法性的唯一保证，执行
//! 阶段不再有第二道判别，§3.4）。

use postern_core::error::ExecError;
use postern_core::plugin::{Channel, RawResponse};
use postern_core::request::Intent;

use super::intent::HttpRequest;

/// 步骤[8] 执行（§3.4）：经 `Channel` 把请求忠实转发到目标端点，回未脱敏响应。
///
/// 负载先解码为 [`HttpRequest`]（印证其确为一份可忠实搬运的 HTTP 请求原文）；解码失败即
/// `Err(ExecError::ProtocolViolation)`（fail-closed 短路，不伪造响应，公理二）。转发是**忠实
/// 搬运**——不在适配器侧改写 `(method, path, headers, body)` 语义（合法性已由 `classify` +
/// `check_constraint` 的白名单约束）；凭据由连接管理层在建连边界注入、适配器不经手、不构造。
///
/// 线协议转发客户端（直连目标端点、流式回传未脱敏状态码 / 头 / 体）由 http 实现波次在集成
/// 测下填实（§3.4）；在此之前对任何输入 **fail-closed**——不 panic、不伪造响应字节、不自建
/// 缓冲、不 spawn 后台任务（§3.7）。失败唯一表达是 `Err(ExecError)`。
pub async fn execute(ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError> {
    let _ = ch;

    // 解码即印证：负载是一份可被忠实搬运的 HTTP 请求原文（method/path/headers/body）。
    // 解码失败即 fail-closed，不伪造响应（公理二）。
    let _request =
        HttpRequest::decode(intent.payload()).map_err(|_| ExecError::ProtocolViolation)?;

    // 线协议转发未接，fail-closed 不发任何请求、不回伪造字节（公理二）。
    Err(ExecError::ExecutionFailed)
}
