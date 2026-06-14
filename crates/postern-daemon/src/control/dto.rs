//! 控制面线缆 DTO（请求/响应 struct）——前端契约权威 `web/src/api/types.ts` 的 Rust 对应。
//!
//! 形态纪律（types.ts §8，镜像 postern-core）：
//! - **雪花 id 一律 `String`** 过线缆（>2^53 在 JS 端丢精度）——store row 的
//!   [`SnowflakeId`](postern_core::id::SnowflakeId) `i64` 投影为十进制字符串，绝不以 number 出线。
//! - **写成功信封** [`WriteAck`]`{policy_rev: String}`（u64 同雪花纪律，字符串化）。
//! - **错误信封** [`ApiErrorBody`]`{error:{code,message}}`（统一错误形状，跨边界前已脱敏）。
//! - 分页信封字段恒为 `items`（非 `list`），随 core [`Page`](postern_core::page::Page) 序列化。
//!
//! 本文件是**纯数据形状 + 投影/映射辅助**，不做任何安全决策、不触后端、零 SQL 标记。
//! row→DTO 投影（store 读模型行 → 出线 DTO）与 DTO→[`WriteIntent`] 字段映射（入线 DTO →
//! 写意图业务字段 JSON）在此提供骨架，GreenAuth 域内填具体字段映射。

use serde::{Deserialize, Serialize};

use postern_core::id::SnowflakeId;

use super::WriteOutcome;

/// 标准写成功信封：新策略修订号（运维对账锚点）。`policy_rev` 为字符串（u64 同雪花纪律）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteAck {
    /// 重建后的策略修订号（[`WriteOutcome::policy_rev`]），字符串化出线。
    pub policy_rev: String,
}

impl WriteAck {
    /// 由写端点三联动结果 [`WriteOutcome`] 组装写成功信封（`policy_rev` 十进制字符串化）。
    pub fn from_outcome(outcome: &WriteOutcome) -> Self {
        Self {
            policy_rev: outcome.policy_rev.to_string(),
        }
    }
}

/// 标准错误体 `{ error: { code, message } }`（types.ts `ApiErrorBody`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// 错误明细（`code` 机读、`message` 人读；二者均已脱敏、无机密细节）。
    pub error: ApiError,
}

/// 错误明细：机读码 + 人读消息（跨边界前脱敏，绝不回显机密 / 库路径 / SQL 片段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// 机读错误码（如 `version_conflict` / `write_failed` / `not_enabled`）。
    pub code: String,
    /// 人读错误消息（脱敏文案）。
    pub message: String,
}

impl ApiErrorBody {
    /// 由码 + 消息组装错误信封。
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            error: ApiError {
                code: code.to_string(),
                message: message.to_string(),
            },
        }
    }
}

/// 雪花 id 投影为出线字符串（store row 的 `i64`/`SnowflakeId` → 十进制字符串，杜绝 JS 丢精度）。
///
/// 这是「id 一律 string」纪律在 daemon 侧的**唯一**投影点：所有 row→DTO 的 id 字段经此产出。
pub fn id_to_string(id: SnowflakeId) -> String {
    id.as_raw().to_string()
}

// ──────────────────────────────────────────────────────── 读模型 DTO（row → 出线）

/// 主体出线行（types.ts `PrincipalRow`）：id 字符串、业务字段、乐观锁 version。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 主体名。
    pub name: String,
    /// 主体类别（`agent`/`program`/`human`）。
    pub kind: String,
    /// 乐观锁版本（下一次写的期望 version）。
    pub version: i64,
}

/// 角色出线行（types.ts `Role` 的骨架投影：本波次只投基础列，effective/inherits 域内填）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 角色名。
    pub name: String,
    /// 角色描述（可空）。
    pub description: Option<String>,
    /// 乐观锁版本。
    pub version: i64,
}

/// 资源出线行（types.ts `ResourceRow` 的骨架投影：tiers/labels 域内填）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 资源代号（恒为代号，绝非真实地址）。
    pub code: String,
    /// 适配器标识。
    pub adapter: String,
    /// 传输标识。
    pub transport: String,
    /// 乐观锁版本。
    pub version: i64,
}

/// 绑定出线行（types.ts `Binding` 的骨架投影：principal/role 名与展开域内填）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 被绑定主体 id（字符串）。
    pub principal_id: String,
    /// 被绑定角色 id（字符串）。
    pub role_id: String,
    /// 乐观锁版本。
    pub version: i64,
}

/// 对象细则出线行（types.ts `ConstraintRow`）。store 读模型只承载 `resource_id`（雪花），
/// 无名称解析能力（store 不提供 codename 反查）——故 `resource` 投影为受约束资源的**雪花 id
/// 字符串**（恒 id、绝非真实地址，公理四不泄露）。`spec` 可空（store `Option<String>`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 受约束资源 id（雪花字符串；恒 id、绝非真实地址）。
    pub resource: String,
    /// 受约束动词。
    pub capability: String,
    /// 约束种类。
    pub kind: String,
    /// 约束规格（可空 JSON 文本）。
    pub spec: Option<String>,
    /// 乐观锁版本。
    pub version: i64,
}

/// 求值条件出线行（types.ts `ConditionRow`）。`resource`（雪花 id 字符串）/ `capability` 可空
/// （资源级 / 全动词通用条件，对齐 types.ts `string | null`）。`spec` 可空。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 受约束资源 id（雪花字符串，可空：空 = 全局通用条件）。
    pub resource: Option<String>,
    /// 受约束动词（可空：空 = 资源全动词通用）。
    pub capability: Option<String>,
    /// 求值谓词。
    pub predicate: String,
    /// 条件规格（可空 JSON 文本）。
    pub spec: Option<String>,
    /// 乐观锁版本。
    pub version: i64,
}

/// 拒绝说明出线行（types.ts `DenyNoteRow`）。`resource` 投影为受约束资源的雪花 id 字符串
/// （恒 id、绝非真实地址）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenyNoteDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 受约束资源 id（雪花字符串；恒 id、绝非真实地址）。
    pub resource: String,
    /// 受约束动词。
    pub capability: String,
    /// 拒绝说明（人亲笔预写）。
    pub note: String,
    /// 乐观锁版本。
    pub version: i64,
}

/// 设置项出线行（types.ts `SettingRow`）。store 只承载 `key`/`value`/`version`；元数据
/// （`default`/`writable`/`kind`）由 daemon 按已知 key 定义、不入库，故本投影只出 store 持有列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingDto {
    /// 业务键。
    pub key: String,
    /// 当前值。
    pub value: String,
    /// 乐观锁版本。
    pub version: i64,
}

/// 模式行出线行（types.ts `ModeStateRow` 的 store 持有列投影；`effective_mode` 由 mode 域 handler
/// 二次投影，本 list 投影只出 store 行列）。`scope` 投影为辖区资源雪花 id 字符串（`None` = 全局）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeRowDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 受治辖区资源 id（雪花字符串，可空：空 = 全局模式）。
    pub scope: Option<String>,
    /// 模式文本（`normal`/`observe`/`maintain`/`freeze`）。
    pub mode: String,
    /// 过期墙钟文本（可空：空 = 不过期）。
    pub expires_at: Option<String>,
    /// 乐观锁版本。
    pub version: i64,
}

/// 临时授权出线行（types.ts `TempGrantRow` 的 store 持有列投影）。`resource`/`principal` 投影为
/// 雪花 id 字符串（恒 id、绝非真实地址）。终态字段 `ended_at`/`end_reason` 可空（活跃时为空）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TempGrantDto {
    /// 雪花 id（字符串）。
    pub id: String,
    /// 被授权主体 id（雪花字符串）。
    pub principal: String,
    /// 被授权资源 id（雪花字符串；恒 id、绝非真实地址）。
    pub resource: String,
    /// 被授权动词。
    pub capability: String,
    /// 授予墙钟。
    pub granted_at: String,
    /// 过期墙钟。
    pub expires_at: String,
    /// 终态墙钟（可空：活跃时为空）。
    pub ended_at: Option<String>,
    /// 终态原因（可空：`expired`/`revoked`）。
    pub end_reason: Option<String>,
    /// 乐观锁版本。
    pub version: i64,
}

/// 审计事件出线行（types.ts `AuditEvent` 的 core carrier 投影；doc-specified 信封字段无后端列、
/// 此处不伪造）。`origin` 投影为**已脱敏不透明文本**（store 本地 `OriginEnvelope` → 文本，**绝不**
/// 构造 `ConnOrigin`、**绝不**回显真实 TCP 地址语义——uid/gid 为本地信任域门，TCP 仅脱敏标记）。
/// `policy_rev` 字符串化（u64 同雪花纪律）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEventDto {
    /// 事件 id（雪花字符串）。
    pub id: String,
    /// 信封 schema 版本。
    pub v: u32,
    /// 事件 kind（`request`/`policy_change`/`deny`/...）。
    pub kind: String,
    /// 固定宽度 UTC 时间戳。
    pub ts: String,
    /// shell 入口（`mcp`/`http`/`control`）。
    pub entry: String,
    /// 已脱敏不透明来源文本（绝不构造 `ConnOrigin`、绝不回显真实地址）。
    pub origin: String,
    /// 决策词（`allow`/`deny`/`escalate_denied`）。
    pub decision: String,
    /// 目标资源代号（恒代号、绝非真实地址）。
    pub resource: String,
    /// 决策时刻策略修订号（字符串化，对账锚点）。
    pub policy_rev: String,
}

/// 拒绝摘要出线行（types.ts `DenialSummaryRow` 的可投影子集；doc-specified 聚合）。
///
/// 由 deny 类审计聚合记录投影：`count` 为窗口内被折叠的 deny 条数（`None` 视同 1）；`resource`
/// 恒代号、绝非真实地址（公理四）；不构造 `ConnOrigin`。无后端列的 doc-specified 字段
/// （`principal_id`/`intent_digest`/`stage`）不伪造。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenialSummaryDto {
    /// 目标资源代号（恒代号、绝非真实地址）。
    pub resource: String,
    /// 窗口内被折叠的 deny 条数。
    pub count: u64,
    /// 聚合记录的决策时刻策略修订号（字符串化）。
    pub policy_rev: String,
}

// ──────────────────────────────────────────────────────── 写请求 DTO（入线 → WriteIntent.fields）

/// 新增主体请求（types.ts `postPrincipal` body）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePrincipalReq {
    /// 主体名。
    pub name: String,
    /// 主体类别（`agent`/`program`/`human`）。
    pub kind: String,
}

/// 新增角色请求（types.ts `postRole` body 的骨架）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoleReq {
    /// 角色名。
    pub name: String,
    /// 角色描述（可空）。
    pub description: Option<String>,
}

/// 新增资源请求（types.ts `postResource` body 的骨架）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateResourceReq {
    /// 资源代号。
    pub code: String,
    /// 适配器标识。
    pub adapter: String,
    /// 传输标识。
    pub transport: String,
}

/// 新增绑定请求（types.ts `postBinding` body 的骨架）。`*_id` 为雪花字符串（入线再解析）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBindingReq {
    /// 被绑定主体 id（字符串）。
    pub principal_id: String,
    /// 被绑定角色 id（字符串）。
    pub role_id: String,
}

/// 对象细则写请求（`POST /v1/constraints`，types.ts `postConstraint` body）。
///
/// **同源 create / delete**：`expected_version` 缺 ⇒ 新增（须带 `resource_id`/`capability`/`kind`、
/// 可空 `spec`）；带 ⇒ 乐观锁逻辑删除（须带 `id`，期望版本即 `expected_version`）——与适配器
/// `commit_constraint` 的 `expected_version` 分流对称。各字段透传进 [`WriteIntent::fields`]，store
/// 写解构层再校验 / 解析（缺字段 ⇒ fail-closed 折为事务级失败）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteConstraintReq {
    /// 受约束资源 id（雪花字符串；新增必带）。
    #[serde(default)]
    pub resource_id: Option<String>,
    /// 受约束动词（新增必带）。
    #[serde(default)]
    pub capability: Option<String>,
    /// 约束种类（新增必带）。
    #[serde(default)]
    pub kind: Option<String>,
    /// 约束规格（可空 JSON 文本）。
    #[serde(default)]
    pub spec: Option<String>,
    /// 既有行 id（逻辑删除必带，雪花字符串）。
    #[serde(default)]
    pub id: Option<String>,
    /// 乐观锁期望版本（带 ⇒ 走逻辑删除分支）。
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// 求值条件写请求（`POST /v1/conditions`，types.ts `postCondition` body）。
///
/// 同源 create / delete（同 [`WriteConstraintReq`] 的 `expected_version` 分流）：新增须带
/// `predicate`，`resource_id`/`capability` 可空（全局 / 全动词通用条件）；删除带 `id` + 期望版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteConditionReq {
    /// 受约束资源 id（雪花字符串，可空：空 = 全局通用条件）。
    #[serde(default)]
    pub resource_id: Option<String>,
    /// 受约束动词（可空：空 = 资源全动词通用）。
    #[serde(default)]
    pub capability: Option<String>,
    /// 求值谓词（新增必带）。
    #[serde(default)]
    pub predicate: Option<String>,
    /// 条件规格（可空 JSON 文本）。
    #[serde(default)]
    pub spec: Option<String>,
    /// 既有行 id（逻辑删除必带，雪花字符串）。
    #[serde(default)]
    pub id: Option<String>,
    /// 乐观锁期望版本（带 ⇒ 走逻辑删除分支）。
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// 拒绝说明写请求（`POST /v1/deny-notes`，types.ts `postDenyNote` body）。
///
/// 同源 create / delete（同 [`WriteConstraintReq`] 的 `expected_version` 分流）：新增须带
/// `resource_id`/`capability`/`note`；删除带 `id` + 期望版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteDenyNoteReq {
    /// 受约束资源 id（雪花字符串；新增必带）。
    #[serde(default)]
    pub resource_id: Option<String>,
    /// 受约束动词（新增必带）。
    #[serde(default)]
    pub capability: Option<String>,
    /// 拒绝说明（人亲笔预写；新增必带）。
    #[serde(default)]
    pub note: Option<String>,
    /// 既有行 id（逻辑删除必带，雪花字符串）。
    #[serde(default)]
    pub id: Option<String>,
    /// 乐观锁期望版本（带 ⇒ 走逻辑删除分支）。
    #[serde(default)]
    pub expected_version: Option<i64>,
}
