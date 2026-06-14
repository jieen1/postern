//! bindings 域 handler（`GET /v1/bindings` 列读 + `POST /v1/bindings` 写）。
//!
//! 与 roles 域同形态（提取器 → [`endpoints`](crate::control::endpoints) → 响应装配）：读经
//! [`endpoints::list`](crate::control::endpoints::list) 取 `Page` 信封（`items`，F-6）→ 200 /
//! 500；写经 [`endpoints::write`](crate::control::endpoints::write) 三联动 → 200 + WriteAck /
//! 409 / 500。响应装配辅助（[`resolve_origin`](super::roles::resolve_origin) /
//! [`write_response`](super::roles::write_response) / [`read_failed`](super::roles::read_failed)）
//! 复用 [`super::roles`]（同属 GreenAuth roles-bindings 域）。
//!
//! `*_id`（principal_id / role_id）入线为雪花**字符串**（id 一律 string，>2^53 在 JS 端丢
//! 精度）——透传进 [`WriteIntent::fields`] 仍以字符串承载，store 写解构层再解析为雪花原始值。
//! 来源以 [`Origin`] 别名读注入值，绝不构造字面 `ConnOrigin::` 变体（SEC_CONSTRUCTION_SITES）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use postern_core::id::SnowflakeId;
use postern_core::request::ConnOrigin as Origin;

use super::roles::{read_failed, resolve_actor, resolve_origin, write_response};
use super::PageParams;
use crate::control::dto::{id_to_string, BindingDto, CreateBindingReq};
use crate::control::endpoints;
use crate::control::{Actor, ControlState, WriteIntent};

/// `GET /v1/bindings`：经 [`endpoints::list`] 取 `Page<serde_json::Value>` 信封（强制分页，F-6）
/// → 200 Json；store 读失败 → 500 + 错误信封。
pub async fn list_bindings(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    match endpoints::list(&*state.policy, "bindings", page.to_query()).await {
        Ok(env) => (StatusCode::OK, Json(env)).into_response(),
        // 全量 bindings 读模型未接通（store 无对应 API）⇒ 501 + 稳定码（能力未接通，非 500）。
        Err(crate::error::DaemonError::NotImplemented) => not_implemented(),
        Err(_) => read_failed(),
    }
}

/// 能力未接通 ⇒ 501 + 稳定机读码（[`NOT_IMPLEMENTED_CODE`]）——能力未接通而非内部失败。
fn not_implemented() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(crate::control::dto::ApiErrorBody::new(
            crate::error::NOT_IMPLEMENTED_CODE,
            "control capability is not enabled yet",
        )),
    )
        .into_response()
}

/// `POST /v1/bindings`：DTO（principal_id/role_id 雪花字符串）→ [`WriteIntent`] → 三联动 → 响应。
///
/// 新增绑定 ⇒ `expected_version = None`（无前驱版本）。`*_id` 以字符串承载进 `fields`
/// （id 一律 string）；store 写解构层再解析为雪花原始值。
pub async fn create_binding(
    State(state): State<ControlState>,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    Json(body): Json<CreateBindingReq>,
) -> Response {
    let origin = match resolve_origin(origin) {
        Ok(o) => o,
        Err(resp) => return *resp,
    };
    let actor = resolve_actor(actor);
    let intent = WriteIntent {
        entity: "bindings",
        fields: serde_json::json!({
            "principal_id": body.principal_id,
            "role_id": body.role_id,
        }),
        expected_version: None,
    };
    write_response(endpoints::write(&*state.policy, &state.audit, origin, &actor, &intent).await)
}

/// store bindings 读模型行 → 出线 [`BindingDto`] 投影锚点（id / principal_id / role_id 一律字符串）。
///
/// 本波次只投基础列（id/principal_id/role_id/version）。**缺口**（见 notes）：types.ts `Binding`
/// 含 `principal`/`role`（名）/`scope_kind`/`scope_spec`/`expanded_resources`（绑定作用域展开），
/// 须从权威快照物化——store `list_bindings_of` 读模型行不含名/作用域展开，且 [`BindingDto`] 共享
/// struct 当前无这些字段，故本投影暂不产出。
#[allow(dead_code)]
pub(crate) fn binding_dto(
    id: SnowflakeId,
    principal_id: SnowflakeId,
    role_id: SnowflakeId,
    version: i64,
) -> BindingDto {
    BindingDto {
        id: id_to_string(id),
        principal_id: id_to_string(principal_id),
        role_id: id_to_string(role_id),
        version,
    }
}
