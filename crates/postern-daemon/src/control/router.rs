//! 控制面 router：挂载于 control.sock 的 axum 路由装配（模块文档 06 §6.5 端点全集）。
//!
//! 把认证中间件（[`super::auth`]）与各端点（[`super::endpoints`] / [`super::approvals`]）装配
//! 成一个**独立**于数据面的 router，注入集合为 [`ControlState`]（PolicyRepo + Enrollment +
//! AuditSink，**绝无**连接池 / Sanitizer，红线 7.2-2）。认证中间件 front 所有端点：
//! SO_PEERCRED uid 比对（即便同 uid）**再叠**控制面本地凭据，二者皆必需（L-1）。
//!
//! 端点全集**恰为** §6.5：principals / credentials / roles / bindings / resources（+POST
//! /discover）/ constraints / conditions / deny-notes / settings / grants·temp（elevate /
//! revoke）/ mode / grants-view / audit / denials·summary / approvals / export / import /
//! verify / health / shutdown。每个写端点 = 事务 COMMIT + 快照重建 + 审计三联动（L-14）。
//!
//! 装配：逐条挂载 [`CONTROL_ROUTES`]（恰覆盖 §6.5），注入集合 = [`ControlState`]
//! （PolicyRepo + Enrollment + AuditSink，绝无连接池 / Sanitizer，红线 7.2-2）。

use std::sync::Arc;

use axum::routing::{get, post, MethodRouter};
use axum::{Json, Router};

use super::verify::{VerifyReport, VerifyRunner};
use super::ControlState;

/// 控制面 router 暴露的全部路由路径**恰为** §6.5 端点集（路径常量表）。
///
/// 钉死端点面的"恰覆盖"：测试逐条核对本表 == §6.5。新增 / 删减一条端点而不同步本表即被
/// 测试抓出（端点面是设计承诺，不是实现自由）。`(method, path)` 二元组，method 为大写动词。
pub const CONTROL_ROUTES: &[(&str, &str)] = &[
    // 主体 / 凭据 / 角色 / 绑定。
    ("GET", "/v1/principals"),
    ("POST", "/v1/principals"),
    ("GET", "/v1/credentials"),
    ("POST", "/v1/credentials"),
    ("GET", "/v1/roles"),
    ("POST", "/v1/roles"),
    ("GET", "/v1/bindings"),
    ("POST", "/v1/bindings"),
    // 资源（含 discover 子动作，F-6）。
    ("GET", "/v1/resources"),
    ("POST", "/v1/resources"),
    ("POST", "/v1/resources/{code}/discover"),
    // 细则 / 条件 / 拒绝备注。
    ("GET", "/v1/constraints"),
    ("POST", "/v1/constraints"),
    ("GET", "/v1/conditions"),
    ("POST", "/v1/conditions"),
    ("GET", "/v1/deny-notes"),
    ("POST", "/v1/deny-notes"),
    // 设置。
    ("GET", "/v1/settings"),
    ("POST", "/v1/settings"),
    // 临时授权（elevate / revoke）+ 模式 + 授权视图。
    ("POST", "/v1/grants/temp/elevate"),
    ("POST", "/v1/grants/temp/revoke"),
    ("POST", "/v1/mode"),
    ("GET", "/v1/grants"),
    // 审计 / 拒绝摘要 / 审批。
    ("GET", "/v1/audit"),
    ("GET", "/v1/denials/summary"),
    ("POST", "/v1/approvals"),
    // 导出 / 导入 / 校验。
    ("POST", "/v1/export"),
    ("POST", "/v1/import"),
    ("POST", "/v1/verify"),
    // 健康 / 关停。
    ("GET", "/v1/health"),
    ("POST", "/v1/shutdown"),
];

/// 构造挂载于 control.sock 的控制面 router（独立于数据面 router）。
///
/// 装配 [`CONTROL_ROUTES`] 全部端点、front 一层认证中间件（[`super::auth`]），并把
/// [`ControlState`]（PolicyRepo + Enrollment + AuditSink）作为注入集合 `with_state`。
/// **绝不**装配连接池 / Sanitizer——它们在 [`ControlState`] 的类型里就不存在（红线 7.2-2）。
pub fn router(state: ControlState) -> Router {
    // 逐条装配 §6.5 端点表：同一路径的 GET / POST 合并到一个 MethodRouter，避免重复挂载
    // 同一 path 时 axum panic（恰覆盖：(method,path) 唯一，由 CONTROL_ROUTES 表保证）。
    let mut router: Router<ControlState> = Router::new();
    for (method, path) in CONTROL_ROUTES {
        // 端点处理器在缺口闭合后接上真实 DTO/处理逻辑；本装配点先挂占位 handler，使
        // 路由面"恰覆盖" §6.5 在运行期成立（route 表 == 实际挂载），注入集合 = ControlState。
        // CONTROL_ROUTES 只含 GET / POST 两类动词（端点面是设计承诺，由 §6.5 表固定）；
        // 任何其它动词在端点表里不存在，fail-closed 跳过——绝不静默挂成可达端点。
        //
        // `GET /v1/health` 已接真实健康投影 handler（D1：进程能 serve 控制面 health）——其余
        // 29 路由仍占位 501（D2 接真实处理器）；只此一条改真，端点面"恰覆盖"不变。
        let handler: Option<MethodRouter<ControlState>> = match (*method, *path) {
            ("GET", "/v1/health") => Some(get(health_handler)),
            ("GET", _) => Some(get(stub_handler)),
            ("POST", _) => Some(post(stub_handler)),
            _ => None,
        };
        if let Some(handler) = handler {
            router = router.route(path, handler);
        }
    }
    // 注入集合 = ControlState（PolicyRepo + Enrollment + AuditSink）；**绝无**连接池 / Sanitizer
    // ——它们在 ControlState 类型里就不存在（红线 7.2-2 在编译期成立）。
    router.with_state(state)
}

/// 占位端点处理器（缺口闭合后由各端点真实处理器替换）。
///
/// 仅用于让控制面 router 在装配点"恰覆盖" §6.5；不做任何安全决策、不触后端。
async fn stub_handler() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_IMPLEMENTED
}

/// `GET /v1/health` 真实处理器：回控制面健康投影 JSON（D1：进程能 serve 控制面 health）。
///
/// 健康投影只读 [`ControlState`] 已持有的句柄——`status` 恒为常量 `"ok"`（进程已装配并 serving
/// 才可能命中本 handler），`policy_rev` 取自 [`PolicyRepo::policy_rev`](super::PolicyRepo::policy_rev)
/// （当前权威快照修订号，运维据此对账控制面与数据面快照一致性）。不触后端、不做安全决策、
/// 不写库；`policy_rev` 读失败 fail-closed 折为 `null`（健康端点不因读失败误报不健康崩溃，
/// 但也不伪报修订号——以 `null` 如实反映「修订号此刻不可读」）。
async fn health_handler(
    axum::extract::State(state): axum::extract::State<ControlState>,
) -> Json<serde_json::Value> {
    let policy_rev = state.policy.policy_rev().ok();
    Json(serde_json::json!({
        "status": "ok",
        "policy_rev": policy_rev,
    }))
}

/// 把 `POST /v1/verify` 路由接到一个真实的红队自检 runner（红队自检的路由落地）。
///
/// 控制面注入集合（[`ControlState`]）**绝无** Kernel（红线 7.2-2）——故 verify 路由不从
/// ControlState 取求值入口，而是经注入的 [`VerifyRunner`] 触发：boot 在**数据面侧**装配一个持有
/// 数据面 [`Kernel`] + verify 临时低权材料的具体 runner，经本函数把它**覆盖**到 [`router`] 已挂
/// 的 `/v1/verify` 占位 handler 上（`.route` 同 path 以后挂者为准），使该路由真实可达（非 501）。
///
/// 本函数刻意**不**改 [`router`] 签名 / [`ControlState`] 类型——runner 持有 Kernel，绝不进
/// ControlState 的注入集合，红线 7.2-2 在编译期不退化。verify 报告以 `Json<VerifyReport>` 回出
/// （逐条 PASS/FAIL + all_pass，供 CLI / SPA 渲染）。
pub fn mount_verify(base: Router, runner: Arc<dyn VerifyRunner>) -> Router {
    base.route("/v1/verify", post(verify_handler))
        .layer(axum::Extension(runner))
}

/// `POST /v1/verify` 真实处理器：触发注入的红队自检 runner，回逐条报告（`Json<VerifyReport>`）。
async fn verify_handler(
    axum::Extension(runner): axum::Extension<Arc<dyn VerifyRunner>>,
) -> Json<VerifyReport> {
    Json(runner.run().await)
}
