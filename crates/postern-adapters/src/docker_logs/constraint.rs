//! docker_logs 细则语义：`container_prefix`（§3.2）。
//!
//! 本 crate 是 `container_prefix` 语义的属主。`check` 是**纯函数**：从 `ci.objects` 取出
//! 容器名维度，对照 `spec` 声明的前缀——目标容器名必须匹配该前缀（集合包含类，全称量化，
//! 任一越界即 `Ok(false)`，§3.2）。判定所需信息缺失即 `Err`，绝不放行（L-7）。

use serde::Deserialize;

use postern_core::domain::ConstraintSpec;
use postern_core::error::ConstraintError;
use postern_core::request::ClassifiedIntent;

/// 本 crate 拥有的细则 kind：容器名前缀匹配（§3.2 集合包含类）。
const KIND_CONTAINER_PREFIX: &str = "container_prefix";

/// `container:` 对象维度前缀（与 [`crate::common::object::container_ref`] 规范化一致）。
const OBJECT_PREFIX: &str = "container:";

/// `container_prefix` 的 spec 负载形态（适配器解释的 JSON，§3.2）。
#[derive(Deserialize)]
struct ContainerPrefixSpec {
    /// 容许的容器名前缀——目标容器名须以此为起点（非子串）匹配。
    prefix: String,
}

/// 步骤[4] 细则判定（§3.2）：容器名前缀匹配。
///
/// 分派语义：
/// - 非 `container_prefix` kind → `Err(UnknownKind)`（非本属主，绝不 `Ok(true)`，L-7）。
/// - spec 无法解释为 `{prefix}` → `Err(InvalidSpec)`。
/// - `ci.objects` 无 `container:<名>` 维度 → `Err(MissingObjects)`（判不了即拒，L-7）。
/// - 容器名以 `prefix` 起头 → `Ok(true)`；否则 `Ok(false)`（全称量化，越界即 false）。
pub fn check(spec: &ConstraintSpec, ci: &ClassifiedIntent) -> Result<bool, ConstraintError> {
    // 非属主 kind：拒绝解释，绝不放行（L-7）。
    if spec.kind != KIND_CONTAINER_PREFIX {
        return Err(ConstraintError::UnknownKind);
    }

    // spec 负载须为 `{prefix: <字符串>}`，否则非法（缺字段 / 类型不符均落此）。
    let parsed: ContainerPrefixSpec =
        serde_json::from_str(&spec.spec).map_err(|_| ConstraintError::InvalidSpec)?;

    // 从 ci.objects 取容器名维度（规范化为 `container:<名>`）。缺则信息不足，拒绝（L-7）。
    let container = ci
        .objects
        .iter()
        .find_map(|o| o.as_str().strip_prefix(OBJECT_PREFIX))
        .ok_or(ConstraintError::MissingObjects)?;

    // 前缀须从头匹配（非子串）：命中 → Ok(true)，越界 → Ok(false)。
    Ok(container.starts_with(&parsed.prefix))
}
