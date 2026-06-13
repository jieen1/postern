//! docker_logs 归类：形态校验恒归 `Observe`（§3.1）。
//!
//! 不做语法树遍历——`Intent` 负载是封闭枚举的只读取数请求，无写变体可表达，故只做
//! 「负载结构合法即归 `Observe`、否则 `Err`」的形态校验。`objects` 取容器选择符规范化
//! 后的 `container:<名>`。把安全性建立在「危险无从表达」而非「危险被识别」上（与 SQL
//! 白名单形态同源，§3.1）。

use postern_core::domain::Capability;
use postern_core::error::ClassifyError;
use postern_core::request::{ClassifiedIntent, Intent};

use crate::common::object;
use crate::docker_logs::intent::DockerLogsRequest;

/// 步骤[2] 归类（§3.1）：合法只读取数负载恒归 `Observe`，否则 `Err`。
///
/// 失败唯一表达是 `Err(ClassifyError)`。负载解码失败（结构不合法）→
/// `ParseFailed`（公理二 fail-closed）；解码成功则**恒** `Observe`——封闭枚举无写变体
/// 可表达，故只校验负载结构、不做运行期危险识别。`objects` 取容器选择符规范化后的
/// 单个 `container:<名>`。
pub fn classify(intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
    // 负载结构合法即归 Observe（§3.1）：解码失败 → fail-closed deny（公理二）。
    let request =
        DockerLogsRequest::decode(intent.payload()).map_err(|_| ClassifyError::ParseFailed)?;

    // 唯一变体即取容器日志（只读取数）；无写变体可表达，恒 Observe。
    let DockerLogsRequest::Logs(logs) = request;

    Ok(ClassifiedIntent {
        capability: Capability::Observe,
        objects: object::dedup(vec![object::container_ref(&logs.container)]),
    })
}
