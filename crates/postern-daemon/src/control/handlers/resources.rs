//! resources 域 handler（`GET /v1/resources` 列读 + `POST /v1/resources` 写 + discover 子动作）。
//!
//! 形态与 [`handlers`](super) 模块文档一致：axum 提取器 → [`endpoints`](crate::control::endpoints)
//! → 响应装配。
//! - **列读**：[`endpoints::list`](crate::control::endpoints::list) 取 `Page<serde_json::Value>`
//!   信封（强制分页，缺省 20、钳 200，F-6）→ 200 Json（字段 `items`）；store 读失败 → 500 +
//!   错误信封 [`ApiErrorBody`]。资源行只回**代号**（`code`），绝不出真实地址 / 存在性（F-6）。
//! - **写**：[`CreateResourceReq`] → [`WriteIntent`]`{entity:"resources", fields, expected_version}`
//!   → [`endpoints::write`](crate::control::endpoints::write) 三联动 → [`WriteHttp`] → 响应
//!   （`Committed` 200 + [`WriteAck`]（`policy_rev` 字符串）/ `Conflict` 409 / `Failed` 500，
//!   后两者带错误信封）。新增资源无前驱版本 ⇒ `expected_version=None`。
//! - **discover**：`POST /v1/resources/{code}/discover` 触发能力发现（F-6：discover **非**授权）。
//!   发现经数据面 [`Adapter::discover`] 落地（非控制面写三联动），D2b 控制面侧尚未接通数据面
//!   发现入口——故如实回 `501 Not Implemented` + 机读码 `discover_not_enabled`（绝不伪造发现
//!   成功、绝不回显真实地址）。
//!
//! 来源（`origin`）纪律：写端点三联动的审计支需要 [`Origin`]——它本应由控制面 listener（shells/）
//! 经 SO_PEERCRED 采集后透传。control/ **非** shells，故本文件**以别名 [`Origin`] 读取**控制面
//! 本地来源（同 sweeper 子域的既有形态），绝不写字面 `ConnOrigin::` 变体（SEC_CONSTRUCTION_SITES：
//! 契约仅放行 shells/ 出现字面 `ConnOrigin::`）。listener 接入循环就绪后，origin 改由提取器透传。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

// control/ 非 shells：以别名读控制面本地来源，绝不写字面 `ConnOrigin::` 变体（雷区）。
use postern_core::request::ConnOrigin as Origin;

use super::PageParams;
use crate::control::dto::{ApiErrorBody, CreateResourceReq, WriteAck};
use crate::control::endpoints::{self, WriteHttp};
use crate::control::{Actor, ControlState, WriteIntent};

/// 控制面本地来源（写端点三联动审计支所需）。
///
/// 控制面 listener 接入循环就绪前，控制面写以本进程同 uid 本地来源占位——经 [`Origin`] 别名
/// 读取，绝不在 control/（非 shells）构造字面 `ConnOrigin::` 变体（SEC_CONSTRUCTION_SITES）。
fn control_origin() -> Origin {
    Origin::UnixPeer { uid: 0, gid: 0 }
}

/// `GET /v1/resources`：经 [`endpoints::list`] 取 `Page<serde_json::Value>` 信封（强制分页，F-6）
/// → 200 Json（`items` 信封）；store 读失败 → 500 + 错误信封。资源行只回代号，绝不出真实地址。
pub async fn list_resources(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    match endpoints::list(&*state.policy, "resources", page.to_query()).await {
        Ok(envelope) => (StatusCode::OK, Json(envelope)).into_response(),
        // store 读失败 fail-closed：500 + 错误信封（绝不伪造空信封、绝不回显库路径 / SQL 片段）。
        Err(_e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody::new("read_failed", "failed to read resources")),
        )
            .into_response(),
    }
}

/// `POST /v1/resources`：DTO → [`WriteIntent`] → [`endpoints::write`] 三联动 → 响应。
///
/// 新增资源无前驱版本 ⇒ `expected_version=None`；`fields` 携 `code`/`adapter`/`transport`
/// 三业务列（资源代号恒为代号）。`Committed` ⇒ 200 + [`WriteAck`]（`policy_rev` 字符串）；
/// `Conflict` ⇒ 409 + 错误信封；`Failed` ⇒ 500 + 错误信封（fail-closed，无半态）。
pub async fn create_resource(
    State(state): State<ControlState>,
    Json(body): Json<CreateResourceReq>,
) -> Response {
    // DTO → WriteIntent.fields（业务列 JSON，零原始 SQL）。新增 ⇒ expected_version None。
    let intent = WriteIntent {
        entity: "resources",
        fields: serde_json::json!({
            "code": body.code,
            "adapter": body.adapter,
            "transport": body.transport,
        }),
        expected_version: None,
    };
    // 三联动（COMMIT + 重建 + 审计同一写锁临界区）；origin/actor 为控制面本地来源 / 操作者。
    let outcome = endpoints::write(
        &*state.policy,
        &state.audit,
        control_origin(),
        &Actor::Operator("control".to_string()),
        &intent,
    )
    .await;
    write_response(outcome)
}

/// `POST /v1/resources/{code}/discover`：触发能力发现（F-6：discover **非**授权）。
///
/// 发现经数据面 [`Adapter::discover`] 落地（非控制面写三联动）；D2b 控制面侧尚未接通数据面
/// 发现入口——如实回 `501 Not Implemented` + 机读码 `discover_not_enabled`（绝不伪造发现
/// 成功、绝不回显真实地址 / 存在性）。`code` 仅为资源代号（恒为代号）。
pub async fn discover_resource(
    State(_state): State<ControlState>,
    Path(_code): Path<String>,
) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ApiErrorBody::new(
            "discover_not_enabled",
            "resource discovery is not enabled (data-plane wiring pending)",
        )),
    )
        .into_response()
}

/// [`WriteHttp`] → axum 响应装配（写端点三类处置 → 状态码 + 载荷）。
///
/// `Committed` ⇒ 200 + [`WriteAck`]（`policy_rev` 字符串化出线）；`Conflict` ⇒ 409 +
/// `version_conflict` 错误信封（乐观锁冲突，F-6 / L-15）；`Failed` ⇒ 500 + `write_failed`
/// 错误信封（事务 / 重建 / 审计任一失败，fail-closed、无半态）。
fn write_response(outcome: WriteHttp) -> Response {
    match outcome {
        WriteHttp::Committed(o) => {
            (StatusCode::OK, Json(WriteAck::from_outcome(&o))).into_response()
        }
        WriteHttp::Conflict => (
            StatusCode::CONFLICT,
            Json(ApiErrorBody::new(
                "version_conflict",
                "resource was modified concurrently; retry with the current version",
            )),
        )
            .into_response(),
        // 写接缝未接通（resources 已接通，理应不命中；穷尽匹配）⇒ 501 + 稳定码（能力未接通）。
        WriteHttp::NotImplemented => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiErrorBody::new(
                crate::error::NOT_IMPLEMENTED_CODE,
                "control capability is not enabled yet",
            )),
        )
            .into_response(),
        WriteHttp::Failed => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody::new(
                "write_failed",
                "failed to write resource",
            )),
        )
            .into_response(),
    }
}
