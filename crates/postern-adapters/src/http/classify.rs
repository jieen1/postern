//! http 归类：按声明的动词工具映射反查 `Capability`（承诺级签名，§3.1）。
//!
//! 依据该资源接入时声明的 `(method × path → Capability)` 表（随 `Intent` 负载搬运，
//! 见 [`super::intent`]），把进来的 `(method, path)` 反查到声明的 `Capability`——命中声明
//! 形态归相应动词，未落任何声明形态 → `Err`（白名单，未声明即不可归类）。归类档位**完全
//! 由声明决定**、不做任何启发式推断（`engine_enforced=false`，没有引擎账号兜底，误归不会
//! 被第二道防线拦下，故必须保守）。`objects` 取 `route:<path>`（§3.1）。

use postern_core::error::ClassifyError;
use postern_core::request::{ClassifiedIntent, Intent};

use crate::common::object;

use super::intent::{parse_capability, HttpRequest};

/// 步骤[2] 归类（§3.1）：命中声明动词工具形态归相应 `Capability`，否则 `Err`。
///
/// 负载解析失败即 `Err(ClassifyError::ParseFailed)`（fail-closed 短路，不吞错放行）。解析
/// 成功后，据该资源声明的 `(method × path → capability)` 映射做**精确白名单反查**：进来的
/// `(method, path)` 须整体命中某条声明项（method 与 path 皆逐字相等），命中即归该项声明的
/// `Capability`、`objects` 取 `route:<path>`。未落任何声明形态 → `Err(Unclassifiable)`
/// （白名单，未声明即不可归类）；声明的动词名无法解回已知 `Capability`（语料笔误 / 越界名）
/// 同样 → `Err`。**绝不做任何启发式推断**（如「GET 即只读」——`engine_enforced=false`，
/// 误归低危的写请求不会被第二道防线拦下，故只信声明，不信方法语义）。失败唯一表达是 `Err`。
pub fn classify(intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
    let request = HttpRequest::decode(intent.payload()).map_err(|_| ClassifyError::ParseFailed)?;

    // 精确反查：(method, path) 须整体命中某条声明项——method 与 path 皆逐字相等。
    // 只比 path 忽略 method 会让未声明方法（如 PUT）穿过同路径的读 / 写声明，是 fail-open。
    let declared = request
        .declared_routes
        .iter()
        .find(|r| r.method == request.method && r.path == request.path)
        .ok_or(ClassifyError::Unclassifiable)?;

    // 声明的动词名解回 Capability：未知名（笔误 / 越界）即不可可靠归类（fail-closed）。
    let capability = parse_capability(&declared.capability).ok_or(ClassifyError::Unclassifiable)?;

    Ok(ClassifiedIntent {
        capability,
        objects: object::dedup(vec![object::route_ref(&request.path)]),
    })
}
