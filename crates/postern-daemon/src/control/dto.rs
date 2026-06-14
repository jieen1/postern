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
