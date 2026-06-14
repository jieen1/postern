//! mode-grants 域 handler：模式（同源读写）+ 授权视图 + 临时授权（elevate / revoke）。
//!
//! 端点（router.rs 挂载、types.ts / endpoints.ts 权威）：
//! - `POST /v1/mode`：**同源读写**——`{op:"read"}` 投影当前 `mode_state` 行（`ModeStateRow[]`，
//!   无 `GET /v1/mode`）；`{op:"set", scope, mode, ttl_ms?, version}` 写一次模式覆盖，回
//!   `{rows, policy_rev}`（写后回投影 + WriteAck）。
//! - `GET /v1/grants`：授权视图 `GrantsView{your_grants, temp_grants}`（`your_grants` 为
//!   resource→capability[] 映射，`temp_grants` 为临时授权行）。
//! - `POST /v1/grants/temp/elevate`：发一条临时升权（必带正 `ttl_ms`，绝不发永久升权）。
//! - `POST /v1/grants/temp/revoke`：撤销一条临时授权（乐观锁 `version`）。
//!
//! 形态纪律（types.ts §8）：所有 id / 雪花一律 `String` 出线（[`id_to_string`](super::super::dto::id_to_string)
//! 是 daemon 侧唯一 i64→string 投影点）；写成功回 [`WriteAck`](super::super::dto::WriteAck)
//! （`policy_rev` 字符串）；错误回 [`ApiErrorBody`](super::super::dto::ApiErrorBody)`{error:{code,message}}`
//! （已脱敏，绝不回显真实地址 / 存在性）。
//!
//! 三联动写经 [`endpoints::write`](super::super::endpoints::write)（COMMIT + 重建 + 审计同一写锁
//! 临界区，L-14）；乐观锁 stale ⇒ 409（[`WriteHttp::Conflict`](super::super::endpoints::WriteHttp)）。
//! 写端点的审计支需 `origin` + `actor`：control/ 非 shells，故以别名 [`Origin`] 读、经 axum
//! `Extension` 提取（控制面 listener 经 SO_PEERCRED 采集后透传），**绝不**构造字面 `ConnOrigin::`
//! 变体（SEC_CONSTRUCTION_SITES）。`Extension` 缺失（生产未透传 / 未认证）⇒ fail-closed 拒绝。
//!
//! 同步 store / audit 调用经 [`endpoints`](super::super::endpoints) 内部 spawn_blocking 边界（§5）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;

use postern_core::domain::Mode;
// control/ 非 shells：需来源类型时以别名读、经 Extension 提取，绝不写字面 `ConnOrigin::` 变体。
use postern_core::request::ConnOrigin as Origin;

use super::super::dto::{id_to_string, ApiErrorBody, WriteAck};
use super::super::endpoints::{self, WriteHttp};
use super::super::{Actor, ControlState, WriteIntent};
use super::PageParams;
use crate::error::DaemonError;

// ════════════════════════════════════════════════════════════════════════════
//  入线 DTO（请求体；types.ts ModeSetRequest / ElevateRequest / RevokeRequest）
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/mode` 请求体：同源读写靠 `op` 判别（`read` 投影 / `set` 写）。
///
/// 入线一次解析为统一体，`op` 决定走读还是写分支（fail-closed：未知 `op` ⇒ 400，绝不静默当读/写）。
#[derive(Debug, Clone, Deserialize)]
pub struct ModeRequest {
    /// 判别词：`"read"`（投影当前模式行）/ `"set"`（写一次模式覆盖）。
    op: String,
    /// `set`：受治辖域，`null` = 全局。
    #[serde(default)]
    scope: Option<String>,
    /// `set`：目标模式（`normal`/`observe`/`maintain`/`freeze`）。
    #[serde(default)]
    mode: Option<String>,
    /// `set`：TTL 毫秒（可缺/`null` = 无到期）。
    #[serde(default)]
    ttl_ms: Option<i64>,
    /// `set`：乐观锁期望版本。
    #[serde(default)]
    version: Option<i64>,
}

/// `POST /v1/grants/temp/elevate` 请求体（types.ts `ElevateRequest`）。
#[derive(Debug, Clone, Deserialize)]
pub struct ElevateRequest {
    /// 受升权主体（雪花字符串入线）。
    principal: String,
    /// 目标资源代号（恒为代号）。
    resource: String,
    /// 升权能力动词。
    capability: String,
    /// TTL 毫秒——必带正值（临时授权绝不发永久升权）。
    ttl_ms: i64,
}

/// `POST /v1/grants/temp/revoke` 请求体（types.ts `RevokeRequest`）。
#[derive(Debug, Clone, Deserialize)]
pub struct RevokeRequest {
    /// 被撤销的临时授权 id（雪花字符串入线）。
    id: String,
    /// 乐观锁期望版本。
    version: i64,
}

// ════════════════════════════════════════════════════════════════════════════
//  POST /v1/mode：同源读写
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/mode`：`op:read` 投影当前模式行 / `op:set` 写一次模式覆盖（无 `GET /v1/mode`）。
///
/// 写支需 `origin` + `actor`（审计三联动支），经 [`Extension`] 提取（控制面 listener 透传）；
/// 读支不需。未知 `op` ⇒ 400（fail-closed）。
pub async fn mode(
    State(state): State<ControlState>,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    Json(body): Json<ModeRequest>,
) -> Response {
    match body.op.as_str() {
        "read" => mode_read(&state).await,
        "set" => mode_set(&state, origin, actor, body).await,
        // fail-closed：既非 read 也非 set，绝不静默当读/写。
        _ => error_response(StatusCode::BAD_REQUEST, "bad_request", "unknown mode op"),
    }
}

/// `op:read`：投影当前 `mode_state` 行集为 `ModeStateRow[]`（顶层数组，含 `effective_mode`）。
async fn mode_read(state: &ControlState) -> Response {
    match endpoints::list(&*state.policy, "mode", PageParams::default().to_query()).await {
        Ok(page) => {
            let policy_rev = current_rev(state);
            let rows = project_mode_rows(&page.items, &policy_rev);
            (StatusCode::OK, Json(rows)).into_response()
        }
        Err(e) => list_error(&e),
    }
}

/// `op:set`：写一次模式覆盖 → 三联动 → Committed 回 `{rows, policy_rev}` / Conflict 409 / Failed 500。
async fn mode_set(
    state: &ControlState,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    body: ModeRequest,
) -> Response {
    // set 必带 mode + version（乐观锁）；缺即 400。
    let (Some(mode), Some(version)) = (body.mode.as_deref(), body.version) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "mode set requires mode and version",
        );
    };
    // 写支须有 origin + actor（审计三联动支）——缺失即 fail-closed 拒（生产未透传 / 未认证）。
    let (Some(origin), Some(actor)) = (origin, actor) else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "control write context missing",
        );
    };
    let intent = WriteIntent {
        entity: "mode",
        fields: serde_json::json!({
            "scope": body.scope,
            "mode": mode,
            "ttl_ms": body.ttl_ms,
        }),
        // 控制面操作者写：走乐观锁（期望版本必传）。
        expected_version: Some(version),
    };
    match endpoints::write(&*state.policy, &state.audit, origin.0, &actor.0, &intent).await {
        WriteHttp::Committed(outcome) => {
            // 写后回投影：再读一次当前模式行 + WriteAck（`{rows, policy_rev}`，endpoints.ts setMode）。
            let policy_rev = outcome.policy_rev.to_string();
            let rows =
                match endpoints::list(&*state.policy, "mode", PageParams::default().to_query())
                    .await
                {
                    Ok(page) => project_mode_rows(&page.items, &policy_rev),
                    // 写已 COMMIT；回投影读失败不改判定（仍 200），rows 留空数组、policy_rev 如实回。
                    Err(_) => Vec::new(),
                };
            let payload = serde_json::json!({ "rows": rows, "policy_rev": policy_rev });
            (StatusCode::OK, Json(payload)).into_response()
        }
        WriteHttp::Conflict => conflict_response(),
        WriteHttp::NotImplemented => not_implemented_response(),
        WriteHttp::Failed => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "policy write failed",
        ),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  GET /v1/grants：授权视图
// ════════════════════════════════════════════════════════════════════════════

/// `GET /v1/grants`：授权视图 `GrantsView{your_grants, temp_grants}`。
///
/// `your_grants` 为该主体自身世界的 resource→capability[] 映射；`temp_grants` 为临时授权行
/// （经 store `grants` 读模型投影，id 一律 string）。读端点不需 origin/actor。
pub async fn grants_view(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    match endpoints::list(&*state.policy, "grants", page.to_query()).await {
        Ok(page) => {
            let temp_grants = project_temp_grants(&page.items);
            let view = serde_json::json!({
                // your_grants 为主体自身世界投影；本读模型行集只承载 temp 行，自身授权映射空对象
                // （RBAC 世界投影在专属读模型，本端点的 list 行即 temp grants，不混淆）。
                "your_grants": serde_json::Map::new(),
                "temp_grants": temp_grants,
            });
            (StatusCode::OK, Json(view)).into_response()
        }
        Err(e) => list_error(&e),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  POST /v1/grants/temp/elevate · revoke：临时授权写
// ════════════════════════════════════════════════════════════════════════════

/// `POST /v1/grants/temp/elevate`：发一条临时升权（必带正 `ttl_ms`）→ 三联动 → WriteAck / 409 / 500。
pub async fn elevate_grant(
    State(state): State<ControlState>,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    Json(body): Json<ElevateRequest>,
) -> Response {
    // 临时授权必带正 TTL：绝不发永久升权（ttl_ms<=0 ⇒ 400）。
    if body.ttl_ms <= 0 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "elevate requires positive ttl_ms",
        );
    }
    let intent = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({
            "op": "elevate",
            "principal": body.principal,
            "resource": body.resource,
            "capability": body.capability,
            "ttl_ms": body.ttl_ms,
        }),
        // 新增临时授权：无既有行版本可比（系统不参与；操作者发起但行不存在）⇒ 不走乐观锁。
        expected_version: None,
    };
    commit(&state, origin, actor, &intent).await
}

/// `POST /v1/grants/temp/revoke`：撤销一条临时授权（乐观锁 `version`）→ 三联动 → WriteAck / 409 / 500。
pub async fn revoke_grant(
    State(state): State<ControlState>,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    Json(body): Json<RevokeRequest>,
) -> Response {
    let intent = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({
            "op": "revoke",
            "id": body.id,
        }),
        // 撤销既有行：走乐观锁（期望版本必传）。
        expected_version: Some(body.version),
    };
    commit(&state, origin, actor, &intent).await
}

// ════════════════════════════════════════════════════════════════════════════
//  共享：三联动提交 + 投影 / 错误装配
// ════════════════════════════════════════════════════════════════════════════

/// 三联动提交 + 标准 WriteAck 装配（Committed 200+WriteAck / Conflict 409 / Failed 500）。
///
/// 写支须有 origin + actor（审计支）——缺失即 fail-closed 拒（生产未透传 / 未认证）。
async fn commit(
    state: &ControlState,
    origin: Option<Extension<Origin>>,
    actor: Option<Extension<Actor>>,
    intent: &WriteIntent,
) -> Response {
    let (Some(origin), Some(actor)) = (origin, actor) else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "control write context missing",
        );
    };
    match endpoints::write(&*state.policy, &state.audit, origin.0, &actor.0, intent).await {
        WriteHttp::Committed(outcome) => {
            let ack = WriteAck::from_outcome(&outcome);
            (StatusCode::OK, Json(ack)).into_response()
        }
        WriteHttp::Conflict => conflict_response(),
        WriteHttp::NotImplemented => not_implemented_response(),
        WriteHttp::Failed => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "policy write failed",
        ),
    }
}

/// 当前权威快照修订号（字符串化；读失败 fail-closed 回 `"0"`，不伪报）。
fn current_rev(state: &ControlState) -> String {
    state
        .policy
        .policy_rev()
        .map(|r| r.to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// 把 `mode_state` 读模型行集投影为 `ModeStateRow[]`（含 `effective_mode` = global.meet(scoped)）。
///
/// `effective_mode` 是 doc-specified 投影：每行的有效模式 = 全局模式 meet 本行模式（取更严者）；
/// 全局行（scope=null）的有效模式即其自身。`policy_rev` 字符串透传到每行（对账锚点）。
fn project_mode_rows(items: &[serde_json::Value], policy_rev: &str) -> Vec<serde_json::Value> {
    // 先取全局模式（scope=null 的行）作为 meet 基底；无全局行 ⇒ Normal（不施加额外限制）。
    let global_mode = items
        .iter()
        .find(|r| r.get("scope").map(|s| s.is_null()).unwrap_or(false))
        .and_then(|r| r.get("mode").and_then(|m| m.as_str()))
        .and_then(parse_mode)
        .unwrap_or(Mode::Normal);

    items
        .iter()
        .map(|row| {
            let scope = row.get("scope").cloned().unwrap_or(serde_json::Value::Null);
            let mode_str = row.get("mode").and_then(|m| m.as_str()).unwrap_or("normal");
            let row_mode = parse_mode(mode_str).unwrap_or(Mode::Normal);
            // 有效模式 = 全局 meet 本行（取更严者）；全局行自身 meet 即其自身。
            let effective = global_mode.meet(row_mode);
            serde_json::json!({
                "scope": scope,
                "mode": mode_str,
                "effective_mode": mode_to_str(effective),
                "expires_at": row.get("expires_at").cloned().unwrap_or(serde_json::Value::Null),
                "version": row.get("version").and_then(|v| v.as_i64()).unwrap_or(0),
                "updated_at": row.get("updated_at").cloned().unwrap_or(serde_json::Value::Null),
                "updated_by": row.get("updated_by").cloned().unwrap_or(serde_json::Value::Null),
                "policy_rev": policy_rev,
            })
        })
        .collect()
}

/// 把 `grants` 读模型行集投影为 `TempGrantRow[]`（id 一律 string）。
fn project_temp_grants(items: &[serde_json::Value]) -> Vec<serde_json::Value> {
    items
        .iter()
        .map(|row| {
            // id 纪律：store row 的 i64 雪花 → 十进制字符串（唯一投影点 id_to_string；此处行已是
            // serde_json，i64 直接字符串化，等价投影——杜绝 JS 端丢精度）。
            let id = row
                .get("id")
                .and_then(|v| v.as_i64())
                .map(|raw| id_to_string(postern_core::id::SnowflakeId::from_raw(raw as u64)))
                .unwrap_or_default();
            serde_json::json!({
                "id": id,
                "resource": row.get("resource").cloned().unwrap_or(serde_json::Value::Null),
                "capability": row.get("capability").cloned().unwrap_or(serde_json::Value::Null),
                "granted_at": row.get("granted_at").cloned().unwrap_or(serde_json::Value::Null),
                "expires_at": row.get("expires_at").cloned().unwrap_or(serde_json::Value::Null),
                "ended_at": row.get("ended_at").cloned().unwrap_or(serde_json::Value::Null),
                "end_reason": row.get("end_reason").cloned().unwrap_or(serde_json::Value::Null),
                "version": row.get("version").and_then(|v| v.as_i64()).unwrap_or(0),
            })
        })
        .collect()
}

/// `Mode` 文本解析（types.ts `Mode` 联合）；未知 ⇒ `None`（调用方 fail-closed 落 Normal/拒）。
fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "normal" => Some(Mode::Normal),
        "observe" => Some(Mode::Observe),
        "maintain" => Some(Mode::Maintain),
        "freeze" => Some(Mode::Freeze),
        _ => None,
    }
}

/// `Mode` → types.ts 文本（出线词，与 `parse_mode` 对称）。
fn mode_to_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "normal",
        Mode::Observe => "observe",
        Mode::Maintain => "maintain",
        Mode::Freeze => "freeze",
    }
}

/// 标准错误信封响应（`{error:{code,message}}`，已脱敏）。
fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(ApiErrorBody::new(code, message))).into_response()
}

/// 乐观锁冲突 ⇒ 409 + `version_conflict` 错误信封（F-6 / L-15）。
fn conflict_response() -> Response {
    error_response(StatusCode::CONFLICT, "version_conflict", "version conflict")
}

/// 能力未接通 ⇒ 501 + 稳定机读码（[`NOT_IMPLEMENTED_CODE`]）——能力未接通而非内部失败。
fn not_implemented_response() -> Response {
    error_response(
        StatusCode::NOT_IMPLEMENTED,
        crate::error::NOT_IMPLEMENTED_CODE,
        "control capability is not enabled yet",
    )
}

/// 列读失败 ⇒ 错误信封（脱敏：绝不回显真实地址 / 存在性 / 库路径）。读模型未接通 ⇒ 501 +
/// 稳定码（能力未接通，非内部失败 500）；其余 ⇒ 500 read_failed。
fn list_error(e: &DaemonError) -> Response {
    match e {
        DaemonError::NotImplemented => not_implemented_response(),
        _ => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "read_failed",
            "policy read failed",
        ),
    }
}
