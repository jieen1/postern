//! roles 域 handler（`GET /v1/roles` 列读 + `POST /v1/roles` 写）。
//!
//! 提取器 → [`endpoints`](crate::control::endpoints) → 响应装配（handlers/mod.rs 形态）：
//! - **读**：[`endpoints::list`](crate::control::endpoints::list) 取 `Page<serde_json::Value>`
//!   信封（强制分页，缺省 20 钳 200）→ 200 Json（`items` 信封，F-6）；store 读失败 → 500 +
//!   错误信封 [`ApiErrorBody`](crate::control::dto::ApiErrorBody)。
//! - **写**：DTO → [`WriteIntent`](crate::control::WriteIntent) →
//!   [`endpoints::write`](crate::control::endpoints::write) 三联动 → [`WriteHttp`]→响应
//!   （`Committed` 200 + [`WriteAck`](crate::control::dto::WriteAck) / `Conflict` 409 /
//!   `Failed` 500，409·500 带错误信封）。
//!
//! 来源 / 操作者经请求扩展读入（镜像 principals 域）：生产由控制面 serve 路径逐连接经
//! `Extension(Origin)` / `Extension(Actor)` 透传（SO_PEERCRED 采集）；in-process 装配（注入缺位）
//! 时来源回退到本地 control.sock 来源（[`control_local_origin`]，shells/ 合规构造）、操作者回退到
//! 未具名控制面操作者。本目录**非** shells，故只**读**注入的来源类型、绝不字面构造 `ConnOrigin`
//! （SEC_CONSTRUCTION_SITES）。
//!
//! 本域两个公共出口（[`write_response`] / [`resolve_origin`]）落本文件，bindings 域以
//! `super::roles::` 复用（同属 GreenAuth roles-bindings 域，零跨域共享文件改动）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use postern_core::id::SnowflakeId;
use postern_core::request::ConnOrigin as Origin;

use super::PageParams;
use crate::control::dto::{id_to_string, ApiErrorBody, CreateRoleReq, RoleDto, WriteAck};
use crate::control::endpoints::{self, WriteHttp};
use crate::control::{Actor, ControlState, WriteIntent};
use crate::shells::listener::control_local_origin;

/// 控制面来源解析（写 handler 公共入口）：优先读控制面 serve 注入的对端来源；缺位 ⇒ 本地
/// control.sock 来源回退（[`control_local_origin`]，shells/ 合规构造）。回退失败（无可信本地
/// uid，极罕见）⇒ `Err` ⇒ 调用方 fail-closed 500（绝不以伪造来源放行写 + 审计）。
pub(super) fn resolve_origin(origin: Option<Extension<Origin>>) -> Result<Origin, Box<Response>> {
    match origin {
        Some(Extension(o)) => Ok(o),
        None => control_local_origin().map_err(|_| Box::new(write_failed())),
    }
}

/// 控制面操作者解析（写 handler 公共入口）：优先读控制面 serve 路径经 SO_PEERCRED 注入的已认证
/// 操作者（`uid:<peer>`，[`operator_of_peer`](crate::control::auth::operator_of_peer)）；注入缺位
/// （未经 serve 注入，如 in-process router 装配）⇒ 回退到固定控制面操作者
/// [`Actor::Operator("control")`]（镜像 resources/misc 域 `control_actor` 同一占位纪律）。生产路径
/// 恒经认证门 + serve 注入真实对端操作者，故 created_by/updated_by 审计归属如实追溯「谁写了」。
pub(super) fn resolve_actor(actor: Option<Extension<Actor>>) -> Actor {
    match actor {
        Some(Extension(a)) => a,
        None => Actor::Operator("control".to_string()),
    }
}

/// [`WriteHttp`] → axum 响应装配（写 handler 公共出口）：
/// - `Committed(outcome)` ⇒ 200 + [`WriteAck`]（`policy_rev` 字符串化）。
/// - `Conflict` ⇒ 409 + 错误信封 `version_conflict`（乐观锁冲突，F-6 / L-15）。
/// - `Failed` ⇒ 500 + 错误信封 `write_failed`（事务 / 重建 / 审计任一失败，fail-closed）。
pub(super) fn write_response(outcome: WriteHttp) -> Response {
    match outcome {
        WriteHttp::Committed(o) => {
            (StatusCode::OK, Json(WriteAck::from_outcome(&o))).into_response()
        }
        WriteHttp::Conflict => (
            StatusCode::CONFLICT,
            Json(ApiErrorBody::new(
                "version_conflict",
                "the resource was modified concurrently; reload and retry",
            )),
        )
            .into_response(),
        // 写接缝未接通（roles/bindings 已接通，理应不命中；穷尽匹配）⇒ 501 + 稳定码。
        WriteHttp::NotImplemented => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiErrorBody::new(
                crate::error::NOT_IMPLEMENTED_CODE,
                "control capability is not enabled yet",
            )),
        )
            .into_response(),
        // 资源代号不存在（roles/bindings 写不引用资源代号，理应不命中；穷尽匹配）⇒ 404。
        WriteHttp::NotFound => (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::new(
                "not_found",
                "referenced resource not found",
            )),
        )
            .into_response(),
        WriteHttp::Failed => write_failed(),
    }
}

/// 读失败的脱敏响应（500 + 错误信封；不回显库路径 / SQL 片段）。
pub(super) fn read_failed() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody::new("read_failed", "failed to read policy")),
    )
        .into_response()
}

/// 写失败的脱敏响应（500 + 错误信封；fail-closed，无半态、无机密细节）。
pub(super) fn write_failed() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody::new("write_failed", "failed to write policy")),
    )
        .into_response()
}

/// `GET /v1/roles`：经 [`endpoints::list`] 取 `Page<serde_json::Value>` 信封（强制分页，F-6）
/// → 200 Json；store 读失败 → 500 + 错误信封。
pub async fn list_roles(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    match endpoints::list(&*state.policy, "roles", page.to_query()).await {
        Ok(env) => (StatusCode::OK, Json(env)).into_response(),
        Err(_) => read_failed(),
    }
}

/// `POST /v1/roles`：DTO → [`WriteIntent`] → [`endpoints::write`] 三联动 → 响应。
///
/// 新增角色 ⇒ `expected_version = None`（无前驱版本）；`admin` 名（任意大小写/空白）由 store
/// schema `CHECK` 拒（`SEC_ADMIN_NOT_GRANTABLE`），表现为写失败 ⇒ 500（fail-closed）。
pub async fn create_role(
    State(state): State<ControlState>,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    Json(body): Json<CreateRoleReq>,
) -> Response {
    // 来源：注入优先、缺位回退本地来源；回退失败 ⇒ fail-closed 500（绝不伪造来源放行）。
    let origin = match resolve_origin(origin) {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    // 操作者：读 serve 注入的已认证操作者；缺位 ⇒ 回退固定控制面操作者（control）。
    let actor = resolve_actor(actor);
    // DTO → WriteIntent.fields（业务字段 JSON；新增无主键，故无 id 字段）。
    let intent = WriteIntent {
        entity: "roles",
        fields: serde_json::json!({
            "name": body.name,
            "description": body.description,
        }),
        expected_version: None,
    };
    write_response(endpoints::write(&*state.policy, &state.audit, origin, &actor, &intent).await)
}

/// store roles 读模型行 → 出线 [`RoleDto`] 投影锚点（id 一律字符串）。
///
/// 本波次只投基础列（id/name/description/version）。**缺口**（见 notes）：types.ts `Role` 含
/// `effective`/`direct`/`inherits_from`（继承展开），须从权威快照物化——store `list_roles`
/// 读模型行不含继承/能力展开，且 [`RoleDto`] 共享 struct 当前无这三字段，故本投影暂不产出。
#[allow(dead_code)]
pub(crate) fn role_dto(
    id: SnowflakeId,
    name: String,
    description: Option<String>,
    version: i64,
) -> RoleDto {
    RoleDto {
        id: id_to_string(id),
        name,
        description,
        version,
    }
}
