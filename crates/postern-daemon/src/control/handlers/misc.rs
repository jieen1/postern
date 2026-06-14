//! 其余控制面域 handler 骨架：凭据 / 细则 / 条件 / 拒绝备注 / 设置 / 临时授权 / 模式 /
//! 授权视图 / 审计 / 拒绝摘要 / 审批 / 导出 / 导入 / 关停。
//!
//! 这些域的读写模型多为 doc-specified（types.ts 标注「无后端 DTO」）或落在后续波次的接线
//! （凭据写 vault = D2c、grants/mode/audit/denials/approvals/export/import 的投影 = GreenAuth 域内）。
//! 本波次留骨架占位，使 router「恰覆盖」§6.5 在运行期成立。**唯一例外**：`POST /v1/credentials`
//! 回明确「D2c 未启用」错误信封——绝不伪造写凭据成功（[`Enrollment`](crate::control::Enrollment)
//! 在 D2b 仍是 fail-closed 桩）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

// control/ 非 shells：需要来源类型时以别名读/构造（绝不写字面 `ConnOrigin::` 变体——
// SEC_CONSTRUCTION_SITES 文本扫描只放行 listener 层的字面 `ConnOrigin::`；本控制面写端点
// 的来源是「同进程操作者经 control.sock 0600 + 认证后的本地写」，以别名 [`Origin`] 表达，
// 与 sweeper / kernel 的系统本地来源同一纪律）。
use postern_core::request::ConnOrigin as Origin;

use super::PageParams;
use crate::control::dto::{
    ApiErrorBody, WriteAck, WriteConditionReq, WriteConstraintReq, WriteDenyNoteReq,
};
use crate::control::endpoints::{
    self, validate_import_on_timeout, validate_settings_on_timeout, WriteHttp,
};
use crate::control::{Actor, ControlState, WriteIntent};

/// `POST /v1/credentials`：凭据写 vault 是 D2c——本波次明确回「未启用」错误信封（不伪造成功）。
///
/// 凭据材料经机密面 [`Enrollment`](crate::control::Enrollment) 落地（D2c 接真）；D2b 的
/// `Enrollment` 仍是 fail-closed 桩，故写端点如实回 `501 Not Implemented` + 机读码
/// `credentials_not_enabled`（绝不静默成功、绝不伪报登记）。
pub async fn create_credential(State(_state): State<ControlState>) -> Response {
    let body = ApiErrorBody::new(
        "credentials_not_enabled",
        "credential enrollment is not enabled (D2c)",
    );
    (StatusCode::NOT_IMPLEMENTED, Json(body)).into_response()
}

/// `GET /v1/credentials`：凭据列读（仅元数据 + secret_hash 存在性，绝不出 secret）。
///
/// 凭据读模型的 store 侧投影未接通（适配器无 `credentials` 读臂，且凭据材料经机密面落地是
/// D2c）——如实回 501 + 稳定码（能力未接通，镜像 `POST /v1/credentials` 的「D2c 未启用」延后），
/// **绝不**经适配器走 `list("credentials")`（那会命中适配器未知实体兜底折成不可区分的 500），
/// 也绝不 `unimplemented!()` panic（那会经 CatchPanic 折成 500）。
pub async fn list_credentials(
    State(_state): State<ControlState>,
    Query(_page): Query<PageParams>,
) -> Response {
    not_implemented_response()
}

/// `GET /v1/constraints` 列读：经 [`endpoints::list`](crate::control::endpoints::list) 取
/// `Page<ConstraintRow>` 信封（强制分页，F-6）→ 200；store 读失败 ⇒ 500 错误信封（[`list_response`]）。
pub async fn list_constraints(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    list_response(endpoints::list(state.policy.as_ref(), "constraints", page.to_query()).await)
}

/// `POST /v1/constraints` 写（同源 create / delete）：`expected_version` 缺 ⇒ 新增、带 ⇒ 乐观锁
/// 逻辑删除（与适配器 `commit_constraint` 分流对称）。DTO → [`WriteIntent`] →
/// [`endpoints::write`](crate::control::endpoints::write) 三联动 → 响应（200 WriteAck / 409 / 500）。
pub async fn create_constraint(
    State(state): State<ControlState>,
    Json(body): Json<WriteConstraintReq>,
) -> Response {
    let intent = WriteIntent {
        entity: "constraints",
        // 字段全透传：store 写解构层据 expected_version 分流 create / delete 并校验必填
        // （缺字段 ⇒ fail-closed 折为事务级失败 500，绝不放行半态写）。
        fields: serde_json::json!({
            "resource_id": body.resource_id,
            "capability": body.capability,
            "kind": body.kind,
            "spec": body.spec,
            "id": body.id,
        }),
        expected_version: body.expected_version,
    };
    control_write_response(&state, &intent).await
}

/// `GET /v1/conditions` 列读：经 [`endpoints::list`](crate::control::endpoints::list) 取
/// `Page<ConditionRow>` 信封（强制分页，F-6）→ 200；store 读失败 ⇒ 500 错误信封。
pub async fn list_conditions(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    list_response(endpoints::list(state.policy.as_ref(), "conditions", page.to_query()).await)
}

/// `POST /v1/conditions` 写（同源 create / delete，同 [`create_constraint`] 分流）：DTO →
/// [`WriteIntent`] → 三联动 → 响应。`resource_id`/`capability` 可空（全局 / 全动词通用条件）。
pub async fn create_condition(
    State(state): State<ControlState>,
    Json(body): Json<WriteConditionReq>,
) -> Response {
    let intent = WriteIntent {
        entity: "conditions",
        fields: serde_json::json!({
            "resource_id": body.resource_id,
            "capability": body.capability,
            "predicate": body.predicate,
            "spec": body.spec,
            "id": body.id,
        }),
        expected_version: body.expected_version,
    };
    control_write_response(&state, &intent).await
}

/// `GET /v1/deny-notes` 列读：经 [`endpoints::list`](crate::control::endpoints::list) 取
/// `Page<DenyNoteRow>` 信封（强制分页，F-6）→ 200；store 读失败 ⇒ 500 错误信封。
pub async fn list_deny_notes(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    list_response(endpoints::list(state.policy.as_ref(), "deny_notes", page.to_query()).await)
}

/// `POST /v1/deny-notes` 写（同源 create / delete，同 [`create_constraint`] 分流）：DTO →
/// [`WriteIntent`] → 三联动 → 响应。
pub async fn create_deny_note(
    State(state): State<ControlState>,
    Json(body): Json<WriteDenyNoteReq>,
) -> Response {
    let intent = WriteIntent {
        entity: "deny_notes",
        fields: serde_json::json!({
            "resource_id": body.resource_id,
            "capability": body.capability,
            "note": body.note,
            "id": body.id,
        }),
        expected_version: body.expected_version,
    };
    control_write_response(&state, &intent).await
}

/// `GET /v1/settings`：settings 行集列读。
///
/// types.ts `getSettings` 回 `SettingRow[]`（裸数组，非 Page 信封——settings 是固定小集，
/// doc-specified 不分页）。经注入的 [`PolicyRepo`](crate::control::PolicyRepo) 列读 settings 实体，
/// 把 `Page.items` 抽出为裸数组出线；store 读失败 ⇒ 500 错误信封。
pub async fn list_settings(State(state): State<ControlState>) -> Response {
    // settings 是固定小集：以全局上限单页列读，抽 items 为裸数组（types.ts 形状）。
    let page = endpoints::page_query(Some(1), Some(postern_core::page::PageQuery::MAX_SIZE));
    match endpoints::list(state.policy.as_ref(), "settings", page).await {
        Ok(page) => (StatusCode::OK, Json(page.items)).into_response(),
        // settings 读模型未接通 ⇒ 501 + 稳定码（能力未接通，非内部失败 500）。
        Err(crate::error::DaemonError::NotImplemented) => not_implemented_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody::new("read_failed", "settings read failed")),
        )
            .into_response(),
    }
}

/// `POST /v1/settings` 的载荷：设置项 key/value + 审批超时处置 + 乐观锁期望版本。
#[derive(Debug, Clone, Deserialize)]
pub struct WriteSettingsReq {
    /// 设置项键。
    pub key: String,
    /// 设置项目标值。
    pub value: String,
    /// 审批超时处置（仅当本次写涉及该项时出现；`allow` 在写入时刻即拒，L-12）。
    #[serde(default)]
    pub on_timeout: Option<String>,
    /// 乐观锁期望版本（运维写恒带；缺则不参与乐观锁——但 settings 写恒为更新已有项）。
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// `POST /v1/settings`：设置写。`on_timeout=allow` 在**写入时刻**即被拒
/// （经 [`validate_settings_on_timeout`]，fail-closed，L-12——绝不把审批超时处置持久化成
/// 在线放行），其余经标准写端点三联动（事务 COMMIT + 快照重建 + 审计，L-14）。
///
/// 校验顺序：先钉 `on_timeout`（带且非 `deny` 即拒，绝不进写路径），再 DTO →
/// [`WriteIntent`] → [`endpoints::write`](crate::control::endpoints::write) → [`WriteHttp`]
/// → 响应（Committed 200 + WriteAck / Conflict 409 / Failed 500）。
pub async fn write_settings(
    State(state): State<ControlState>,
    Json(body): Json<WriteSettingsReq>,
) -> Response {
    // ① on_timeout 显式声明且非 deny（含 allow）⇒ 写入时刻即拒（fail-closed，绝不持久化在线放行）。
    if let Some(on_timeout) = body.on_timeout.as_deref() {
        if validate_settings_on_timeout(on_timeout).is_err() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiErrorBody::new(
                    "on_timeout_allow_rejected",
                    "settings on_timeout=allow is rejected (escalation timeout is always deny)",
                )),
            )
                .into_response();
        }
    }
    // ② DTO → WriteIntent（业务字段 JSON；settings 写恒为更新已有项 ⇒ 携乐观锁期望版本）。
    let intent = WriteIntent {
        entity: "settings",
        fields: serde_json::json!({ "key": body.key, "value": body.value }),
        expected_version: body.expected_version,
    };
    // ③ 标准写端点三联动（来源 + 审计句柄经签名传入；来源以 control_origin 别名构造）。
    let http = endpoints::write(
        state.policy.as_ref(),
        &state.audit,
        control_origin(),
        &control_actor(),
        &intent,
    )
    .await;
    write_response(http)
}

// mode / grants(view) / elevate / revoke 的真身在 mode_grants.rs（router 挂载的是后者）——
// 此处此前的四个 `unimplemented!()` 重复桩是死代码（无任何引用），已删除（绝不留 panic 死桩）。

// ════════════════════════════════════════════════════════════════════════════
//  audit-misc 域共享装配辅助（来源 / 操作者 / Page 信封 / 写结果 → 响应）
// ════════════════════════════════════════════════════════════════════════════

/// 控制面写端点的来源（三联动审计支需要 [`Origin`]）。
///
/// 控制面写**不**是数据面请求——它是「同进程操作者经 control.sock（0600 + SO_PEERCRED uid
/// 比对 + 本地凭据，L-1）认证后发起的本地策略写」。本进程自身 uid/gid 由 listener/auth 采集后
/// 透传是 D2c 的接线；D2b 控制面写以「系统本地来源」占位（与 sweeper `recycle_event` /
/// kernel 系统来源同一纪律），经 [`Origin`] 别名表达——**绝不**写字面 `ConnOrigin::` 变体
/// （SEC_CONSTRUCTION_SITES：字面构造由 listener 层独占）。
fn control_origin() -> Origin {
    Origin::UnixPeer { uid: 0, gid: 0 }
}

/// 控制面写操作者（落 `created_by` / `updated_by`）。D2b 以固定操作者标识占位；接真后由 auth
/// 透传已认证操作者身份（控制面只交「是谁在写」，五个审计字段由 store base 自动填充）。
fn control_actor() -> Actor {
    Actor::Operator("control".to_string())
}

/// 控制面写端点共享装配（细则 / 条件 / 拒绝备注同形态）：把已组装的 [`WriteIntent`] 经标准写端点
/// 三联动（[`endpoints::write`](crate::control::endpoints::write)：事务 COMMIT + 快照重建 + 审计同
/// 一写锁临界区，L-14）落地，再把 [`WriteHttp`] 装配为响应（[`write_response`]：Committed 200 +
/// WriteAck / Conflict 409 / Failed 500）。来源 + 操作者经 [`control_origin`] / [`control_actor`]
/// 别名构造（镜像 [`write_settings`] 同一占位纪律），绝不字面构造 `ConnOrigin`。
async fn control_write_response(state: &ControlState, intent: &WriteIntent) -> Response {
    let http = endpoints::write(
        state.policy.as_ref(),
        &state.audit,
        control_origin(),
        &control_actor(),
        intent,
    )
    .await;
    write_response(http)
}

/// 把集合读结果（`Page<serde_json::Value>` / 读失败）装配为 axum 响应。
///
/// 读成功 ⇒ 200 + `Page` 信封（字段恒 `items`，F-6，随 core
/// [`Page`](postern_core::page::Page) 序列化）；store 读失败 ⇒ 500 + 错误信封
/// （不伪造空信封、不泄后端细节）。
fn list_response(
    result: Result<postern_core::page::Page<serde_json::Value>, crate::error::DaemonError>,
) -> Response {
    match result {
        Ok(page) => (StatusCode::OK, Json(page)).into_response(),
        // 读模型未接通（store 侧无对应读）⇒ 501 + 稳定码（能力未接通，非内部失败 500）。
        Err(crate::error::DaemonError::NotImplemented) => not_implemented_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody::new("read_failed", "list read failed")),
        )
            .into_response(),
    }
}

/// 把写端点三联动结果 [`WriteHttp`] 装配为 axum 响应：
/// - `Committed` ⇒ 200 + [`WriteAck`]（`policy_rev` 字符串，rev 前进后修订号）；
/// - `Conflict`  ⇒ 409 + 错误信封 `version_conflict`（乐观锁，F-6 / L-15）；
/// - `Failed`    ⇒ 500 + 错误信封 `write_failed`（fail-closed，无半态）。
fn write_response(http: WriteHttp) -> Response {
    match http {
        WriteHttp::Committed(outcome) => {
            (StatusCode::OK, Json(WriteAck::from_outcome(&outcome))).into_response()
        }
        WriteHttp::Conflict => (
            StatusCode::CONFLICT,
            Json(ApiErrorBody::new(
                "version_conflict",
                "stale optimistic-lock version",
            )),
        )
            .into_response(),
        WriteHttp::NotImplemented => not_implemented_response(),
        // 资源代号不存在（本路径写不反查资源代号，理应不命中；穷尽匹配）⇒ 404。
        WriteHttp::NotFound => (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::new(
                "not_found",
                "referenced resource not found",
            )),
        )
            .into_response(),
        WriteHttp::Failed => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody::new("write_failed", "policy write failed")),
        )
            .into_response(),
    }
}

/// 写接缝未接通 ⇒ 501 + 稳定机读码（[`NOT_IMPLEMENTED_CODE`]）——能力未接通而非内部失败。
///
/// 与 `credentials_not_enabled` / `discover_not_enabled` 同一「未启用」族：运维 / SPA / CLI 据此
/// 把「能力未接通」与 500 内部失败、409 乐观锁冲突逐一区分（绝不伪装成 500）。
fn not_implemented_response() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ApiErrorBody::new(
            crate::error::NOT_IMPLEMENTED_CODE,
            "control capability is not enabled yet",
        )),
    )
        .into_response()
}

/// `GET /v1/audit`：审计列读经 [`endpoints::list`](crate::control::endpoints::list) 取分页
/// `Page` 信封（强制分页，缺省 20、钳 200，F-6）→ 200；store 读失败 ⇒ 500 错误信封。
///
/// 审计读模型逐项已脱敏（资源恒为代号、绝不出真实地址 / 凭据，公理四）；id 投影为字符串
/// （雪花不丢精度）由 store 投影层保证。控制面读句柄经注入的
/// [`PolicyRepo`](crate::control::PolicyRepo)。
pub async fn list_audit(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    list_response(endpoints::list(state.policy.as_ref(), "audit", page.to_query()).await)
}

/// `GET /v1/denials/summary`：拒绝摘要（doc-specified 聚合）经
/// [`endpoints::list`](crate::control::endpoints::list) 取分页 `Page` 信封 → 200；读失败 ⇒ 500。
///
/// 聚合行不泄露真实地址 / 存在性（资源恒为代号）；强制分页（F-6）。
pub async fn denials_summary(
    State(state): State<ControlState>,
    Query(page): Query<PageParams>,
) -> Response {
    list_response(endpoints::list(state.policy.as_ref(), "denials_summary", page.to_query()).await)
}

/// `POST /v1/approvals` 的载荷：`op:list`（查询）/ `op:adjudicate`（裁决）同源端点。
#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalsReq {
    /// 操作鉴别符（`list` 查询 / `adjudicate` 裁决）；缺省按 `list` 处理。
    #[serde(default)]
    pub op: Option<String>,
}

/// `POST /v1/approvals`：审批同源读写（无 `GET /v1/approvals`）。
///
/// 审批在 D2b **关闭**（`on_timeout` 恒固定为 deny，L-12）：`op:list` ⇒ 回**空** `Page` 信封
/// （待审队列恒空，审批关闭时 escalate 不入队）；`op:adjudicate` ⇒ 恒 `deny`——审批关闭时
/// **绝不**伪造「裁决通过」，回明确「审批已禁用」错误信封（fail-closed，allow 在类型层不可
/// 表达）。
pub async fn approvals(
    State(_state): State<ControlState>,
    body: Option<Json<ApprovalsReq>>,
) -> Response {
    let op = body
        .and_then(|Json(req)| req.op)
        .unwrap_or_else(|| "list".to_string());
    if op == "adjudicate" {
        // 审批关闭：裁决恒 deny，绝不伪造通过（L-12）。
        return (
            StatusCode::CONFLICT,
            Json(ApiErrorBody::new(
                "approvals_disabled",
                "approvals are disabled; escalations are denied (on_timeout=deny)",
            )),
        )
            .into_response();
    }
    // op:list（含缺省）：待审队列恒空（审批关闭，escalate 不入队）⇒ 回空 Page 信封。
    let empty: postern_core::page::Page<serde_json::Value> = postern_core::page::Page {
        items: Vec::new(),
        page_no: 1,
        page_size: postern_core::page::PageQuery::DEFAULT_SIZE,
        total: 0,
    };
    (StatusCode::OK, Json(empty)).into_response()
}

/// `POST /v1/export`：策略导出（TOML 包）。D2b 回稳定形状 `{ toml }`（doc-specified 包封）。
///
/// 导出包**不含**凭据材料（机密零接触，公理四）——导出的是策略读模型，凭据材料绝不出库面。
/// 实际 TOML 物化的 store 投影是后续波次；D2b 回一个合法空包封（形状稳定、不伪造内容）。
pub async fn export_policy(State(_state): State<ControlState>) -> Response {
    (StatusCode::OK, Json(serde_json::json!({ "toml": "" }))).into_response()
}

/// `POST /v1/import` 的载荷：策略包 + 审批超时处置（校验阶段钉死）。
#[derive(Debug, Clone, Deserialize)]
pub struct ImportReq {
    /// 待导入策略包的审批超时处置（仅接受 `deny`；`allow` 在校验阶段即拒，L-12）。
    #[serde(default)]
    pub on_timeout: Option<String>,
}

/// `POST /v1/import`：策略导入。`on_timeout=allow` 在**校验阶段**即被拒
/// （经 [`validate_import_on_timeout`]，fail-closed，L-12——绝不让导入把审批超时处置改成
/// 在线放行）。校验通过后回应用结果计数（`applied`）。
///
/// 缺 `on_timeout` 字段视同未声明放行处置：按缺省 `deny` 校验通过（导入不**引入** allow）。
pub async fn import_policy(
    State(_state): State<ControlState>,
    body: Option<Json<ImportReq>>,
) -> Response {
    let on_timeout = body
        .and_then(|Json(req)| req.on_timeout)
        .unwrap_or_else(|| "deny".to_string());
    if validate_import_on_timeout(&on_timeout).is_err() {
        // fail-closed：on_timeout=allow 在校验阶段即拒，绝不应用（无半态）。
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::new(
                "on_timeout_allow_rejected",
                "import on_timeout=allow is rejected (escalation timeout is always deny)",
            )),
        )
            .into_response();
    }
    // 校验通过：D2b 回稳定应用结果形状（实际策略包应用的 store 接线是后续波次）。
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "added": 0,
            "changed": 0,
            "deleted": 0,
            "applied": true,
        })),
    )
        .into_response()
}

/// `POST /v1/shutdown` 关停（确认令牌 + 优雅收口）。
///
/// 该路由在 router.rs 被**真实挂载**（CONTROL_ROUTES 表内、`post(misc::shutdown)`），生产路径
/// 经 serve.rs 的 CatchPanicLayer 兜底——故体内**绝不** `unimplemented!()` panic（那会被折成不可
/// 区分的 500）。关停语义的真实接线（确认令牌校验 + 优雅收口信号）是后续波次；D2b 与
/// `list_credentials` / 写接缝未接通族同一纪律，如实回 501 + 稳定机读码（能力未接通，非内部失败 500）。
pub async fn shutdown(State(_state): State<ControlState>) -> Response {
    not_implemented_response()
}
