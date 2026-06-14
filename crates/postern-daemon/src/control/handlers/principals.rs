//! principals 域 handler（`GET /v1/principals` 列读 + `POST /v1/principals` 写）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

// control/ 非 shells：需要来源类型时以别名读注入值，绝不写字面 `ConnOrigin::` 变体
// （SEC_CONSTRUCTION_SITES：ConnOrigin 字面只许在 shells/ 出现）。
use postern_core::request::ConnOrigin as Origin;

use super::PageParams;
use crate::control::dto::{ApiErrorBody, CreatePrincipalReq, WriteAck};
use crate::control::endpoints::{self, WriteHttp};
use crate::control::{Actor, ControlState, WriteIntent};
use crate::shells::listener::control_local_origin;

/// `GET /v1/principals`：经 [`endpoints::list`](crate::control::endpoints::list) 取
/// `Page<serde_json::Value>` 信封（强制分页，F-6）→ 200 Json；store 读失败 → 500 + 错误信封。
pub async fn list_principals(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    // 分页缺省填充 + 钳制（缺省 20、钳 200）委托唯一钳制点；集合读经 store scope/scan 分页层。
    match endpoints::list(&*state.policy, "principals", page.to_query()).await {
        // 回 Page<T> 信封（字段恒为 items，随 core Page 序列化，F-6）。
        Ok(env) => (StatusCode::OK, Json(env)).into_response(),
        // store 读失败 fail-closed → 500 + 脱敏错误信封（不伪造空信封、不回显库细节）。
        Err(_) => list_failed().into_response(),
    }
}

/// `POST /v1/principals`：DTO → [`WriteIntent`] → [`endpoints::write`] 三联动 → WriteHttp→响应
/// （Committed 200+WriteAck / Conflict 409 / Failed 500，均带 WriteAck / 错误信封）。
///
/// 来源 / 操作者经请求扩展读入：生产由控制面 serve 路径
/// （[`serve_control_router_over_uds`](crate::shells::serve::serve_control_router_over_uds)）逐连接
/// 经 `Extension(Origin)` / `Extension(Actor)` 透传（SO_PEERCRED 采集）；in-process 装配（注入
/// 缺位）时来源回退到本地 control.sock 来源（[`control_local_origin`]，shells/ 合规构造）、操作者
/// 回退到未具名控制面操作者。本 handler 只**读**注入的来源类型，绝不字面构造 `ConnOrigin`。
pub async fn create_principal(
    State(state): State<ControlState>,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    Json(body): Json<CreatePrincipalReq>,
) -> Response {
    // 来源：优先读控制面 serve 注入的对端来源；缺位 ⇒ 本地 control.sock 来源回退（shells/ 合规）。
    // 回退失败（无可信本地 uid，极罕见）⇒ fail-closed 500（绝不以伪造来源放行写 + 审计）。
    let origin = match origin {
        Some(Extension(o)) => o,
        None => match control_local_origin() {
            Ok(o) => o,
            Err(_) => return write_failed().into_response(),
        },
    };
    // 操作者：读控制面 serve 路径经 SO_PEERCRED 注入的已认证操作者（`uid:<peer>`）。生产路径
    // 恒经认证门 + serve 注入真实对端操作者；注入缺位（未经 serve 注入，如 in-process 装配）⇒
    // 回退固定控制面操作者（control，镜像 resources/misc 域 control_actor 同一占位纪律）。
    let actor = match actor {
        Some(Extension(a)) => a,
        None => Actor::Operator("control".to_string()),
    };

    // DTO → WriteIntent.fields（业务字段 JSON，零原始 SQL）。新增 ⇒ expected_version None
    // （无前驱版本可校验；store 据 None 走 create 分支）。
    let intent = WriteIntent {
        entity: "principals",
        fields: serde_json::json!({ "name": body.name, "kind": body.kind }),
        expected_version: None,
    };

    // 写端点三联动（事务 COMMIT + 快照重建 + 审计同一写锁临界区，L-14）。
    match endpoints::write(&*state.policy, &state.audit, origin, &actor, &intent).await {
        // 成功 ⇒ 200 + WriteAck（policy_rev 字符串化，雪花/u64 同纪律）。
        WriteHttp::Committed(outcome) => {
            (StatusCode::OK, Json(WriteAck::from_outcome(&outcome))).into_response()
        }
        // 乐观锁版本冲突 ⇒ 409 + 错误信封（F-6 / L-15）。
        WriteHttp::Conflict => (
            StatusCode::CONFLICT,
            Json(ApiErrorBody::new(
                "version_conflict",
                "the resource was modified concurrently; reload and retry",
            )),
        )
            .into_response(),
        // 写接缝未接通（principals 已接通，理应不命中；穷尽匹配）⇒ 501 + 稳定码（能力未接通）。
        WriteHttp::NotImplemented => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiErrorBody::new(
                crate::error::NOT_IMPLEMENTED_CODE,
                "control capability is not enabled yet",
            )),
        )
            .into_response(),
        // 其余写失败（事务 / 快照重建 / 审计）⇒ 500 + 错误信封（fail-closed，无半态）。
        WriteHttp::Failed => write_failed().into_response(),
    }
}

/// 列读失败的脱敏响应（500 + 错误信封；不回显库路径 / SQL 片段）。
fn list_failed() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody::new(
            "list_failed",
            "failed to read principals",
        )),
    )
        .into_response()
}

/// 写失败的脱敏响应（500 + 错误信封；fail-closed，无半态、无机密细节）。
fn write_failed() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiErrorBody::new(
            "write_failed",
            "failed to write principal",
        )),
    )
        .into_response()
}
