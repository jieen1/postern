//! 控制面 [`PolicyRepo`] 的真实 store 适配器（D2b 写接缝核心）。
//!
//! 把控制面缝 trait [`PolicyRepo`](super::PolicyRepo) 接到 postern-store 的
//! [`store::PolicyRepo`](postern_store::policy::PolicyRepo)：
//!
//! - **写**（[`commit_write`](StorePolicyRepo::commit_write)）：把
//!   [`WriteIntent`]`{entity, fields, expected_version}` 解构成具体 per-entity 调用，经 store 的
//!   `*_and_rebuild`（实体写 + `bump_policy_rev` + COMMIT + `build_snapshot` + `SnapshotView::replace`
//!   同一写锁临界区原子完成，§8 L-14）。store 的 `(new_version, new_rev)` 包装为
//!   [`WriteOutcome`]。乐观锁冲突（store `VersionConflict`）→ [`WriteError::VersionConflict`]
//!   （整体 ROLLBACK，rev 不进、快照不换——全或无）。
//! - **读**（[`list`](StorePolicyRepo::list)）：按 `entity` 分流到对应 `list_*`，store 读模型行
//!   投影为 `serde_json::Value`（id 一律字符串，[`dto::id_to_string`](super::dto::id_to_string)）。
//! - **修订号**（[`policy_rev`](StorePolicyRepo::policy_rev)）：取自快照视图（健康端点对账锚点）。
//!
//! 纪律（雷区）：本文件零 SQL 标记（DB 写读全经 store API，契约 `DB_WRITE_PATH_CENTRALIZED` /
//! `DB_NO_RAW_SQL_OUTSIDE_STORE`）；`Actor` 经 store base::write::Actor 映射；不构造机密类型 /
//! `ConnOrigin`；`anyhow` 禁用，只用 [`DaemonError`]。同步 store 调用由控制面端点在 spawn_blocking
//! 边界驱动（§5）。
//!
//! **本波次为 D2b 骨架**：分流框架 + 写接缝形态已定，per-entity 字段解构 / row 投影体留
//! `unimplemented!()` 占位（GreenAuth 域内按 DTO 填），保证编译、RED 测试可挂。

use std::sync::Arc;

use postern_core::id::SnowflakeId;
use postern_core::page::{Page, PageQuery};

use postern_store::base::error::StoreError;
use postern_store::base::meta::read_policy_rev;
use postern_store::base::write::Actor as StoreActor;
use postern_store::policy::PolicyRepo as StorePolicyRepo;

use super::dto::{
    BindingDto, ConditionDto, ConstraintDto, DenyNoteDto, ModeRowDto, PrincipalDto, ResourceDto,
    RoleDto, SettingDto, TempGrantDto,
};
use super::{Actor, AuditRead, PolicyRepo, WriteError, WriteIntent, WriteOutcome};
use crate::error::DaemonError;

/// 控制面 [`PolicyRepo`] 的真实 store 适配器：持 [`store::PolicyRepo`](StorePolicyRepo) 写句柄
/// （`with_view` 装配，写后在同一临界区重建并发布快照）+ 审计读句柄（append-only 审计载体）。
///
/// 写句柄**绝不**进数据面注入集合（红线 7.2-2）；数据面只读消费同一 `SnapshotView`。本适配器
/// 是控制面写端点三联动的 store 侧落点。
///
/// **审计读第二句柄**（[`audit`](StorePolicyRepoAdapter::audit)）：policy.db 写读句柄
/// （[`inner`](StorePolicyRepoAdapter::inner)）够不到 append-only 审计载体（JSONL，不走 policy
/// 写锁）——故适配器另持一个 [`AuditRead`] 缝，`list("audit")` / `list("denials_summary")` 经此
/// 投影。boot 装配处把真实 [`JsonlAuditReader`](super::audit_read::JsonlAuditReader)（复用三联动
/// 审计写支的**同一** `JsonlAuditSink` 实例）接上，单一载体、无双源。
pub struct StorePolicyRepoAdapter {
    /// 底层 store 写句柄（per-entity `*_and_rebuild` 经此驱动）。
    inner: Arc<StorePolicyRepo>,
    /// 审计读句柄（`list("audit")` / `list("denials_summary")` 经此投影；与 policy.db 载体分离）。
    audit: Arc<dyn AuditRead>,
}

impl StorePolicyRepoAdapter {
    /// 由已装配 [`SnapshotView`] 的 store 写句柄 + 审计读句柄构造适配器（boot 在快照就绪后装配）。
    ///
    /// `audit` 是 append-only 审计载体的读缝（与 policy.db 截然分离）——boot 复用三联动审计写支
    /// 的同一 `JsonlAuditSink` 实例物化它，使审计读 / 写见同一载体（单一权威状态）。
    pub fn new(inner: Arc<StorePolicyRepo>, audit: Arc<dyn AuditRead>) -> Self {
        Self { inner, audit }
    }
}

/// 控制面 [`Actor`] → store [`base::write::Actor`](StoreActor) 映射。
///
/// 控制面只交"是谁在写"；五个审计字段由 store base 自动填充。`Operator` → 操作者标识、
/// `System`（sweeper / import）→ store `System`（不参与乐观锁）。
fn to_store_actor(actor: &Actor) -> StoreActor {
    match actor {
        Actor::Operator(id) => StoreActor::Operator(id.clone()),
        Actor::System => StoreActor::System,
    }
}

/// store 写错误族 → 控制面 [`WriteError`] 映射（穷尽 per-variant，无 `_ =>` 兜底）。
///
/// 乐观锁冲突 → [`WriteError::VersionConflict`]（端点映射 409）；约束违反 / IO / id 生成 / 未知
/// schema 版本一律折叠为 [`WriteError::Transaction`]（事务级失败，fail-closed、无半态）。快照重建
/// 失败的 [`WriteError::SnapshotRebuild`] 由 store `commit_and_rebuild` 内部以 `StoreError` 上抛，
/// 在此同样落 `Transaction`（store 不细分"重建失败"为独立变体——三联动在 store 内是单一原子事务，
/// 任一相失败整体 ROLLBACK，对 daemon 表现为一次事务失败）。
fn to_write_error(err: StoreError) -> WriteError {
    match err {
        StoreError::VersionConflict => WriteError::VersionConflict,
        StoreError::ConstraintViolation => WriteError::Transaction,
        StoreError::Io => WriteError::Transaction,
        StoreError::UnknownSchemaVersion => WriteError::Transaction,
        StoreError::IdGen => WriteError::Transaction,
    }
}

impl PolicyRepo for StorePolicyRepoAdapter {
    fn commit_write(
        &self,
        actor: &Actor,
        intent: &WriteIntent,
    ) -> Result<WriteOutcome, WriteError> {
        let store_actor = to_store_actor(actor);
        // 按实体分流到对应 store `*_and_rebuild`（实体写 + rev + 重建同一临界区原子）。
        // 每分支把 intent.fields（DTO 序列化而来）解构成该实体的具体写参数（域内填），
        // intent.expected_version 透传为乐观锁期望版本。store 返 (new_version, new_rev)。
        //
        // grants 分支单独直返 [`WriteError`]（而非经 `to_write_error` 折叠 `StoreError`）：
        // elevate 的资源**代号**反查未命中是「客户端引用了不存在的资源」（404），须区别于真实
        // 事务失败（500）——这层区分在 `StoreError` 里不可表达，故 `commit_grant` 直接产出
        // 富错误（`ResourceNotFound`）。其余实体仍走 `StoreError → to_write_error` 统一映射。
        let result: Result<(i64, u64), WriteError> = match intent.entity {
            "grants" => commit_grant(&self.inner, &store_actor, intent),
            other => {
                let store_result: Result<(i64, u64), StoreError> = match other {
                    "principals" => commit_principal(&self.inner, &store_actor, intent),
                    "roles" => commit_role(&self.inner, &store_actor, intent),
                    "resources" => commit_resource(&self.inner, &store_actor, intent),
                    "bindings" => commit_binding(&self.inner, &store_actor, intent),
                    // 对象细则 / 求值条件 / 拒绝说明：create（expected_version None）/ delete（Some）二分。
                    "constraints" => commit_constraint(&self.inner, &store_actor, intent),
                    "conditions" => commit_condition(&self.inner, &store_actor, intent),
                    "deny_notes" => commit_deny_note(&self.inner, &store_actor, intent),
                    // 模式（upsert by 辖区）/ 设置（upsert by key）：store 专用 *_and_rebuild 接通。
                    "mode" => commit_mode(&self.inner, &store_actor, intent),
                    "settings" => commit_setting(&self.inner, &store_actor, intent),
                    // 未知实体类别绝不静默放行：端点固定 entity，理应恒命中；兜底 fail-closed。
                    _ => Err(StoreError::ConstraintViolation),
                };
                store_result.map_err(to_write_error)
            }
        };
        match result {
            Ok((version, new_rev)) => Ok(WriteOutcome {
                version,
                policy_rev: new_rev,
            }),
            Err(e) => Err(e),
        }
    }

    fn list(
        &self,
        entity: &'static str,
        page: PageQuery,
    ) -> Result<Page<serde_json::Value>, DaemonError> {
        // 按实体分流到对应 store `list_*`，store 读模型行投影为 serde_json::Value
        // （id 一律字符串）。store 读失败 fail-closed → DaemonError::Boot（不伪造空信封）。
        match entity {
            "principals" => list_principals(&self.inner, page),
            "roles" => list_roles(&self.inner, page),
            "resources" => list_resources(&self.inner, page),
            "bindings" => list_bindings(&self.inner, page),
            "constraints" => list_constraints(&self.inner, page),
            "conditions" => list_conditions(&self.inner, page),
            "deny_notes" => list_deny_notes(&self.inner, page),
            "settings" => list_settings(&self.inner, page),
            "mode" => list_mode(&self.inner, page),
            "grants" => list_grants(&self.inner, page),
            // 审计读不在 policy.db 载体上（append-only JSONL，不走 policy 写锁）——经第二句柄
            // （AuditRead）投影。`scan`/deny 聚合见同一 JsonlAuditSink 实例（boot 复用，单一载体）。
            "audit" => self.audit.scan_audit(page),
            "denials_summary" => self.audit.denials_summary(page),
            _ => Err(DaemonError::Boot),
        }
    }

    fn policy_rev(&self) -> Result<u64, DaemonError> {
        // 健康投影锚点：取持久 policy_rev（写接缝在同事务内 bump 它、并据此重建快照，故它即
        // 当前权威快照的 rev）。经 store base::meta 读（DB 读经 store API，零原始 SQL）。
        // 读失败 fail-closed → DaemonError::Boot（不伪报修订号）。
        read_policy_rev(self.inner.db()).map_err(|_| DaemonError::Boot)
    }
}

// ──────────────────────────────────────────── per-entity 写解构（intent.fields → store 调用）

/// 从 `intent.fields` 取一个必填字符串字段；缺字段 / 非字符串 ⇒ `ConstraintViolation`
/// （端点 DTO 序列化而来，理应恒在；缺即 fail-closed 折为事务级失败，绝不放行半态写）。
fn field_str<'a>(intent: &'a WriteIntent, key: &str) -> Result<&'a str, StoreError> {
    intent
        .fields
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or(StoreError::ConstraintViolation)
}

/// 从 `intent.fields` 取一个雪花 id 字段（出线一律字符串，入线解析十进制 u64）；缺 / 非法
/// ⇒ `ConstraintViolation`。
fn field_id(intent: &WriteIntent, key: &str) -> Result<SnowflakeId, StoreError> {
    let raw: u64 = field_str(intent, key)?
        .parse()
        .map_err(|_| StoreError::ConstraintViolation)?;
    Ok(SnowflakeId::from_raw(raw))
}

/// 取一个可空字符串字段：缺 / JSON null ⇒ `None`；存在且为字符串 ⇒ `Some`；存在但非字符串
/// （类型错）⇒ `ConstraintViolation`（fail-closed，绝不静默当缺省）。
fn field_str_opt<'a>(intent: &'a WriteIntent, key: &str) -> Result<Option<&'a str>, StoreError> {
    match intent.fields.get(key) {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(v) => v.as_str().map(Some).ok_or(StoreError::ConstraintViolation),
    }
}

/// 取一个可空雪花 id 字段：缺 / null ⇒ `None`；存在 ⇒ 解析（非法十进制 ⇒ `ConstraintViolation`）。
fn field_id_opt(intent: &WriteIntent, key: &str) -> Result<Option<SnowflakeId>, StoreError> {
    match field_str_opt(intent, key)? {
        None => Ok(None),
        Some(s) => {
            let raw: u64 = s.parse().map_err(|_| StoreError::ConstraintViolation)?;
            Ok(Some(SnowflakeId::from_raw(raw)))
        }
    }
}

/// principals 写解构：把 `intent.fields`（principal DTO 序列化）解构为 create / rename 调用。
///
/// `expected_version` 为 `None` ⇒ 新增（`create_principal_and_rebuild`，取 `name`/`kind`）；
/// `Some(v)` ⇒ 改名乐观锁（`rename_principal_and_rebuild`，取 `id`/`name`，期望版本 `v`）。
fn commit_principal(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    match intent.expected_version {
        None => {
            let name = field_str(intent, "name")?;
            let kind = field_str(intent, "kind")?;
            inner.create_principal_and_rebuild(actor, name, kind)
        }
        Some(expected) => {
            let id = field_id(intent, "id")?;
            let name = field_str(intent, "name")?;
            inner.rename_principal_and_rebuild(actor, id, expected, name)
        }
    }
}

/// roles 写解构（新增）：`intent.fields` → `create_role_and_rebuild`（`name` + 可空 `description`）。
fn commit_role(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    let name = field_str(intent, "name")?;
    let description = intent.fields.get("description").and_then(|v| v.as_str());
    inner.create_role_and_rebuild(actor, name, description)
}

/// resources 写解构（新增）：`intent.fields` → `create_resource_and_rebuild`
/// （`code` 落 codename + `adapter` + `transport`）。
fn commit_resource(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    let code = field_str(intent, "code")?;
    let adapter = field_str(intent, "adapter")?;
    let transport = field_str(intent, "transport")?;
    inner.create_resource_and_rebuild(actor, code, adapter, transport)
}

/// bindings 写解构（新增）：`intent.fields` → `create_binding_and_rebuild`
/// （`principal_id` + `role_id`，二者入线为雪花字符串、此处解析）。
fn commit_binding(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    let principal_id = field_id(intent, "principal_id")?;
    let role_id = field_id(intent, "role_id")?;
    inner.create_binding_and_rebuild(actor, principal_id, role_id)
}

/// constraints 写解构：`None` ⇒ 新增（`resource_id`/`capability`/`kind`/可空 `spec`）；
/// `Some(v)` ⇒ 乐观锁逻辑删除（`id`，期望版本 `v`）。
fn commit_constraint(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    match intent.expected_version {
        None => {
            let resource_id = field_id(intent, "resource_id")?;
            let capability = field_str(intent, "capability")?;
            let kind = field_str(intent, "kind")?;
            let spec = field_str_opt(intent, "spec")?;
            inner.create_constraint_and_rebuild(actor, resource_id, capability, kind, spec)
        }
        Some(expected) => {
            let id = field_id(intent, "id")?;
            inner.delete_constraint_and_rebuild(actor, id, expected)
        }
    }
}

/// conditions 写解构：`None` ⇒ 新增（可空 `resource_id`/`capability`、必填 `predicate`、可空
/// `spec`）；`Some(v)` ⇒ 乐观锁逻辑删除（`id`，期望版本 `v`）。
fn commit_condition(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    match intent.expected_version {
        None => {
            let resource_id = field_id_opt(intent, "resource_id")?;
            let capability = field_str_opt(intent, "capability")?;
            let predicate = field_str(intent, "predicate")?;
            let spec = field_str_opt(intent, "spec")?;
            inner.create_condition_and_rebuild(actor, resource_id, capability, predicate, spec)
        }
        Some(expected) => {
            let id = field_id(intent, "id")?;
            inner.delete_condition_and_rebuild(actor, id, expected)
        }
    }
}

/// deny-notes 写解构：`None` ⇒ 新增（`resource_id`/`capability`/`note`）；`Some(v)` ⇒ 乐观锁
/// 逻辑删除（`id`，期望版本 `v`）。
fn commit_deny_note(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    match intent.expected_version {
        None => {
            let resource_id = field_id(intent, "resource_id")?;
            let capability = field_str(intent, "capability")?;
            let note = field_str(intent, "note")?;
            inner.create_deny_note_and_rebuild(actor, resource_id, capability, note)
        }
        Some(expected) => {
            let id = field_id(intent, "id")?;
            inner.delete_deny_note_and_rebuild(actor, id, expected)
        }
    }
}

/// mode 写解构（**upsert** by 辖区）：`scope`（可空雪花 id = 全局）/ `mode` / 可空 `expires_at`
/// → `set_mode_and_rebuild`。store 内按辖区哨兵决定 insert vs update（version 自增），故本处
/// **不**按 `expected_version` 分流——upsert 语义由 store 承载，期望版本不在本写参数里。
fn commit_mode(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    let scope_resource_id = field_id_opt(intent, "scope")?;
    let mode = field_str(intent, "mode")?;
    let expires_at = field_str_opt(intent, "expires_at")?;
    inner.set_mode_and_rebuild(actor, scope_resource_id, mode, expires_at)
}

/// settings 写解构（**upsert** by key）：`key` / `value` → `set_setting_and_rebuild`。store 内
/// 按 key 决定 insert vs update（version 自增），故本处不按 `expected_version` 分流。
fn commit_setting(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), StoreError> {
    let key = field_str(intent, "key")?;
    let value = field_str(intent, "value")?;
    inner.set_setting_and_rebuild(actor, key, value)
}

/// grants 写解构（按 `op` 判别）：`elevate` ⇒ 新增临时授权（`principal` 为雪花 id 字符串、
/// `resource` 为资源**代号**、`capability`、正 `ttl_ms`）→ `elevate_grant_and_rebuild`；
/// `revoke` ⇒ 乐观锁撤销（`id`，期望版本 `expected_version`）→ `revoke_grant_and_rebuild`。
///
/// **`resource` 是资源代号（非雪花 id）**：`ElevateRequest.resource` 文档恒为代号（如
/// `db-main`）。store `elevate_grant_and_rebuild` 形参要求 `resource_id` 为 [`SnowflakeId`]，
/// 故此处经 store [`resource_id_by_code`](StorePolicyRepo::resource_id_by_code) 反查代号 → id；
/// 未命中（无此代号 / 已删 / 已停用）⇒ [`WriteError::ResourceNotFound`]（fail-closed，404，绝不
/// 臆造资源 / 误折为 500）。`principal` 仍为雪花 id 字符串（`field_id`）。`ttl_ms` 非正 / 缺、
/// `op` 未知 / 缺、字段缺失等解构失败一律折为 [`WriteError::Transaction`]（事务级失败，绝不放行
/// 半态写）。store 写错误经 [`to_write_error`] 映射（乐观锁冲突 ⇒ `VersionConflict`）。
fn commit_grant(
    inner: &StorePolicyRepo,
    actor: &StoreActor,
    intent: &WriteIntent,
) -> Result<(i64, u64), WriteError> {
    let op = field_str(intent, "op").map_err(to_write_error)?;
    match op {
        "elevate" => {
            let principal_id = field_id(intent, "principal").map_err(to_write_error)?;
            // resource 是资源代号（恒为代号）：经 store 只读反查代号 → 资源 id；未命中即 404
            // （ResourceNotFound），区别于真实事务失败（绝不误折为 500）。
            let code = field_str(intent, "resource").map_err(to_write_error)?;
            let resource_id = inner
                .resource_id_by_code(code)
                .map_err(to_write_error)?
                .ok_or(WriteError::ResourceNotFound)?;
            let capability = field_str(intent, "capability").map_err(to_write_error)?;
            let ttl_ms = intent
                .fields
                .get("ttl_ms")
                .and_then(|v| v.as_i64())
                .filter(|ttl| *ttl > 0)
                .ok_or(WriteError::Transaction)? as u64;
            inner
                .elevate_grant_and_rebuild(actor, principal_id, resource_id, capability, ttl_ms)
                .map_err(to_write_error)
        }
        "revoke" => {
            let id = field_id(intent, "id").map_err(to_write_error)?;
            // 撤销既有行：走乐观锁（期望版本必传，缺即 fail-closed）。
            let expected = intent.expected_version.ok_or(WriteError::Transaction)?;
            inner
                .revoke_grant_and_rebuild(actor, id, expected)
                .map_err(to_write_error)
        }
        // 未知 / 缺 op：端点固定，理应恒命中；兜底 fail-closed。
        _ => Err(WriteError::Transaction),
    }
}

// ──────────────────────────────────────────── per-entity 读投影（store row → serde_json::Value）

/// 把一页读模型行经 `to_dto` 映射为出线 DTO，再序列化为 `serde_json::Value`，保留分页元数据。
///
/// id 字符串化纪律由各 DTO（[`dto`](super::dto) 经 [`id_to_string`](super::dto::id_to_string)）
/// 承载；序列化失败（DTO 恒可序列化，理应不发生）⇒ fail-closed → `DaemonError::Boot`。
fn project_page<R, D, F>(
    page: Result<Page<R>, StoreError>,
    to_dto: F,
) -> Result<Page<serde_json::Value>, DaemonError>
where
    D: serde::Serialize,
    F: Fn(R) -> D,
{
    let page = page.map_err(|_| DaemonError::Boot)?;
    let mut items = Vec::with_capacity(page.items.len());
    for row in page.items {
        let value = serde_json::to_value(to_dto(row)).map_err(|_| DaemonError::Boot)?;
        items.push(value);
    }
    Ok(Page {
        items,
        page_no: page.page_no,
        page_size: page.page_size,
        total: page.total,
    })
}

/// principals 列读：store `list_principals` → `Page<serde_json::Value>`（id 字符串投影）。
fn list_principals(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_principals(page), |row| PrincipalDto {
        id: row.id.as_raw().to_string(),
        name: row.name,
        kind: row.kind,
        version: row.version,
    })
}

/// roles 列读：store `list_roles` → `Page<serde_json::Value>`。
fn list_roles(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_roles(page), |row| RoleDto {
        id: row.id.as_raw().to_string(),
        name: row.name,
        description: row.description,
        version: row.version,
    })
}

/// resources 列读：store `list_resources` → `Page<serde_json::Value>`（store `codename` → DTO `code`）。
fn list_resources(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_resources(page), |row| ResourceDto {
        id: row.id.as_raw().to_string(),
        code: row.codename,
        adapter: row.adapter,
        transport: row.transport,
        version: row.version,
    })
}

/// bindings 列读（全量）：store `list_bindings` → `Page<serde_json::Value>`（id /
/// `principal_id` / `role_id` 一律雪花字符串）。
fn list_bindings(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_bindings(page), |row| BindingDto {
        id: row.id.as_raw().to_string(),
        principal_id: row.principal_id.as_raw().to_string(),
        role_id: row.role_id.as_raw().to_string(),
        version: row.version,
    })
}

/// constraints 列读：store `list_constraints` → `Page`（`resource_id` 投影为雪花字符串；
/// 恒 id、绝非真实地址）。
fn list_constraints(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_constraints(page), |row| ConstraintDto {
        id: row.id.as_raw().to_string(),
        resource: row.resource_id.as_raw().to_string(),
        capability: row.capability,
        kind: row.kind,
        spec: row.spec,
        version: row.version,
    })
}

/// conditions 列读：store `list_conditions` → `Page`（可空 `resource_id`/`capability` 如实投影，
/// 资源 id 字符串化）。
fn list_conditions(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_conditions(page), |row| ConditionDto {
        id: row.id.as_raw().to_string(),
        resource: row.resource_id.map(|r| r.as_raw().to_string()),
        capability: row.capability,
        predicate: row.predicate,
        spec: row.spec,
        version: row.version,
    })
}

/// deny-notes 列读：store `list_deny_notes` → `Page`（`resource_id` 投影为雪花字符串）。
fn list_deny_notes(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_deny_notes(page), |row| DenyNoteDto {
        id: row.id.as_raw().to_string(),
        resource: row.resource_id.as_raw().to_string(),
        capability: row.capability,
        note: row.note,
        version: row.version,
    })
}

/// 已知设置项目录：`(key, default, writable, kind)`。元数据（默认值 / 是否可写 / 类型）由
/// daemon 按已知 key 定义、不入库（store 只承载 `key`/`value`/`version`）；前端按此固定 key 集
/// 渲染（types.ts `SettingRow`），未列出的 key 为系统未知 key、不在此出现。
///
/// 纪律：`approval.on_timeout` 恒为 `deny` 且 **不可写**（L-12，ESCALATE_FOLDS_TO_DENY 的配置面
/// 体现——审批超时处置永不可被改成放行）。`audit.fsync` 缺省 `always`、`audit.retention_days`
/// 缺省 `30` 与 store 审计载体的同名缺省（`FsyncPolicy::PerEvent` / `DEFAULT_RETENTION_DAYS`）一致。
const SETTINGS_CATALOG: &[(&str, &str, bool, &str)] = &[
    ("approval.enabled", "false", true, "bool"),
    ("approval.on_timeout", "deny", false, "enum"),
    ("audit.fsync", "always", true, "enum"),
    ("audit.retention_days", "30", true, "int"),
    ("audit.exporter.otel.enabled", "false", true, "bool"),
];

/// settings 列读：已知设置目录（[`SETTINGS_CATALOG`]）叠加 store 持有值。
///
/// store `list_settings` 取持久化的 `key→(value, version)`，目录据此产出固定 key 集合的出线行：
/// 已持久化的 key 出其存值 + version；未持久化的 key 出目录默认值 + version 0。元数据
/// （default / writable / kind）恒由目录定义。settings 是固定小集（非 DataTable、无分页），故
/// 一次出全（忽略入参分页页码）；store 侧以全局上限单页足以覆盖全部持久化 key。
fn list_settings(
    inner: &StorePolicyRepo,
    _page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    // 取全部持久化设置（固定小集，单页 MAX_SIZE 覆盖全部）：key → (value, version)。
    let stored = inner
        .list_settings(PageQuery {
            page_no: 1,
            page_size: PageQuery::MAX_SIZE,
        })
        .map_err(|_| DaemonError::Boot)?;
    let mut by_key = std::collections::BTreeMap::new();
    for row in stored.items {
        by_key.insert(row.key, (row.value, row.version));
    }

    let mut items = Vec::with_capacity(SETTINGS_CATALOG.len());
    for &(key, default, writable, kind) in SETTINGS_CATALOG {
        let (value, version) = match by_key.get(key) {
            Some((v, ver)) => (v.clone(), *ver),
            None => (default.to_string(), 0),
        };
        let dto = SettingDto {
            key: key.to_string(),
            value,
            default: default.to_string(),
            writable,
            version,
            kind: kind.to_string(),
        };
        items.push(serde_json::to_value(dto).map_err(|_| DaemonError::Boot)?);
    }

    let total = items.len() as u64;
    Ok(Page {
        items,
        page_no: 1,
        page_size: SETTINGS_CATALOG.len() as u32,
        total,
    })
}

/// mode 列读：store `list_mode_state` → `Page`（`scope` 投影为辖区资源雪花字符串，`None` = 全局）。
fn list_mode(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_mode_state(page), |row| ModeRowDto {
        id: row.id.as_raw().to_string(),
        scope: row.scope_resource_id.map(|r| r.as_raw().to_string()),
        mode: row.mode,
        expires_at: row.expires_at,
        version: row.version,
    })
}

/// grants 列读：store `list_temp_grants` → `Page`（`principal`/`resource` 投影为雪花字符串；
/// 终态字段如实出线）。
fn list_grants(
    inner: &StorePolicyRepo,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    project_page(inner.list_temp_grants(page), |row| TempGrantDto {
        id: row.id.as_raw().to_string(),
        principal: row.principal_id.as_raw().to_string(),
        resource: row.resource_id.as_raw().to_string(),
        capability: row.capability,
        granted_at: row.granted_at,
        expires_at: row.expires_at,
        ended_at: row.ended_at,
        end_reason: row.end_reason,
        version: row.version,
    })
}
