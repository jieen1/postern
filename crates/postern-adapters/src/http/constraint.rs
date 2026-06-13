//! http 细则语义：`http_route`（承诺级签名，§3.2）。
//!
//! 本 crate 是 `http_route` 语义的属主。`check` 是**纯函数**：请求 `(method, path)`（method
//! 已由 `classify` 映射为 `ci.capability`，path 取自 `ci.objects` 的 `route:<path>`）必须落在
//! `spec` 路由白名单内（接口维度限制——可对读 / 写分别声明不同路径）。集合包含类，全称量化，
//! 任一越界即 `Ok(false)`（§3.2）。判定所需信息缺失即 `Err`，绝不放行（L-7）。

use postern_core::domain::ConstraintSpec;
use postern_core::error::ConstraintError;
use postern_core::request::ClassifiedIntent;

use super::intent::HttpRouteSpec;

/// http 属主的细则 `kind`（§3.2）：http 仅定义 `http_route` 语义。
pub const KIND_HTTP_ROUTE: &str = "http_route";

/// `route:<path>` 对象前缀（§3.1 `route_ref` 规范化形态）。
const ROUTE_PREFIX: &str = "route:";

/// 步骤[4] 细则判定（§3.2）：`(ci.capability, route:<path>)` 落在路由白名单内。
///
/// fail-closed 短路：非 `http_route` kind → `Err(UnknownKind)`；spec 串畸形 →
/// `Err(InvalidSpec)`；`ci.objects` 无任一 `route:<path>` → `Err(MissingObjects)`（「判不了」=
/// 「不通过」，绝不 `Ok(true)`，L-7）。
///
/// 通过短路后做**白名单全称量化**：请求触达的**每个** `route:<path>` 对象，其
/// `(ci.capability, path)` 都须命中白名单某条 `(capability, path)` 声明（capability 按规范
/// 小写名比对、path 逐字相等）。任一路由对象越界——路径未声明**或**动词未声明（路径在白名单
/// 内但 capability 未列）——即 `Ok(false)`，绝不 `Ok(true)`。只比 path 忽略动词是 fail-open
/// （越权写穿过读 / 写子集白名单），故动词维度必须保留（§3.2 读 / 写分路）。全部命中 →
/// `Ok(true)`。
pub fn check(spec: &ConstraintSpec, ci: &ClassifiedIntent) -> Result<bool, ConstraintError> {
    if spec.kind != KIND_HTTP_ROUTE {
        return Err(ConstraintError::UnknownKind);
    }
    let whitelist = HttpRouteSpec::decode(&spec.spec).map_err(|_| ConstraintError::InvalidSpec)?;

    // 从 ci.objects 取出所有 route:<path> 维度（http_route 判定的对象视图）。
    let routes: Vec<&str> = ci
        .objects
        .iter()
        .filter_map(|o| o.as_str().strip_prefix(ROUTE_PREFIX))
        .collect();

    // 无任一 route 对象 → 信息不足，判不了即拒（L-7），绝不放行。
    if routes.is_empty() {
        return Err(ConstraintError::MissingObjects);
    }

    // 请求动词的规范小写名——白名单按 (capability, path) 键入，动词维度逐名比对。
    let verb = ci.capability.as_str();

    // 全称量化：每个 route 对象的 (capability, path) 都须命中白名单某条声明，
    // 否则任一越界即 Ok(false)（路径越界或动词越界皆不放行，fail-closed）。
    let all_in = routes.iter().all(|path| {
        whitelist
            .routes
            .iter()
            .any(|allow| allow.capability == verb && allow.path == *path)
    });

    Ok(all_in)
}
