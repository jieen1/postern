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

use super::dto::{PrincipalDto, ResourceDto, RoleDto};
use super::{Actor, PolicyRepo, WriteError, WriteIntent, WriteOutcome};
use crate::error::DaemonError;

/// 控制面 [`PolicyRepo`] 的真实 store 适配器：持 [`store::PolicyRepo`](StorePolicyRepo) 写句柄
/// （`with_view` 装配，写后在同一临界区重建并发布快照）。
///
/// 写句柄**绝不**进数据面注入集合（红线 7.2-2）；数据面只读消费同一 `SnapshotView`。本适配器
/// 是控制面写端点三联动的 store 侧落点。
pub struct StorePolicyRepoAdapter {
    /// 底层 store 写句柄（per-entity `*_and_rebuild` 经此驱动）。
    inner: Arc<StorePolicyRepo>,
}

impl StorePolicyRepoAdapter {
    /// 由已装配 [`SnapshotView`] 的 store 写句柄构造适配器（boot 在快照就绪后装配）。
    pub fn new(inner: Arc<StorePolicyRepo>) -> Self {
        Self { inner }
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
        let result: Result<(i64, u64), StoreError> = match intent.entity {
            "principals" => commit_principal(&self.inner, &store_actor, intent),
            "roles" => commit_role(&self.inner, &store_actor, intent),
            "resources" => commit_resource(&self.inner, &store_actor, intent),
            "bindings" => commit_binding(&self.inner, &store_actor, intent),
            // settings / mode / grants 的写接缝（store 侧专用 *_and_rebuild）留待后续波次——
            // **绝不**折叠为通用事务失败（500，会让运维误判为 daemon 内部坏掉）；如实回
            // NotImplemented，端点据此回 501 + 稳定码（能力未接通，镜像 credentials/discover）。
            // 此前这些实体落 ConstraintViolation→Transaction→500，与真实事务失败不可区分（缺陷）。
            "settings" | "mode" | "grants" => return Err(WriteError::NotImplemented),
            // 未知实体类别绝不静默放行：端点固定 entity，理应恒命中；兜底 fail-closed。
            _ => Err(StoreError::ConstraintViolation),
        };
        match result {
            Ok((version, new_rev)) => Ok(WriteOutcome {
                version,
                policy_rev: new_rev,
            }),
            Err(e) => Err(to_write_error(e)),
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
            // 这些读模型的 store 侧投影留待后续波次（无全量 bindings / settings / audit / mode /
            // grants / denials 读模型）——如实回 NotImplemented，端点据此回 501 + 稳定码（能力未
            // 接通），**绝不**折叠为 Boot（端点会映成 500，与真实内部失败不可区分，缺陷所在）。
            "settings" | "audit" | "mode" | "grants" | "denials_summary" => {
                Err(DaemonError::NotImplemented)
            }
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

/// bindings 列读：store 只暴露 `list_bindings_of(principal_id, page)`（绑定按主体过滤），**无**
/// 全量 bindings 列读 API——无主体过滤的 `list("bindings")` 在 store 侧无对应能力，故如实回
/// `NotImplemented`（端点据此回 501 + 稳定码，能力未接通），**绝不**伪造空信封、**绝不**折叠为
/// Boot（端点会映成 500，与真实内部失败不可区分）。按主体过滤的绑定读由 bindings 域专用端点
/// 承接（store `list_bindings_of`，后续波次）。
fn list_bindings(
    _inner: &StorePolicyRepo,
    _page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    Err(DaemonError::NotImplemented)
}
