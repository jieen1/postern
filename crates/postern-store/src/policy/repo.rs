//! PolicyRepo：策略状态事务读写句柄，一律经 base 仓储（唯一写路径）。
//!
//! 本文件**绝不**直接出现数据库写语句：每个写方法都在一次写事务内委托
//! [`base::write`](crate::base::write) 的 API（写改逻辑删除级联只在
//! `src/base/`，契约 `DB_WRITE_PATH_CENTRALIZED`）。读端点经
//! [`base::scope`](crate::base::scope) 的分页执行器（默认作用域 `delete_flag = 0`
//! + `LIMIT`），返回携 `version` 的读模型行。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::base::db::Db;
use crate::base::error::StoreError;
use crate::base::scope::execute_page;
use crate::base::timestamp;
use crate::base::write::{self, Actor, InsertRow};
use crate::snapshot::{build_snapshot_on, SnapshotView};
use postern_core::domain::{Capability, PrincipalId, ResourceCode, Timestamp};
use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::PolicyView;
use rusqlite::types::Value;

/// principals 读模型行：8 基础字段中对调用方有意义的子集（含 `version` 供乐观锁
/// 端到端贯通）+ 业务列。读端点统一带 `version`（§6.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalRow {
    /// 主键（雪花 id 原始值）。
    pub id: SnowflakeId,
    /// 乐观锁版本（更新/删除时由调用方回传为期望 version）。
    pub version: i64,
    /// 主体名（归一化后落库值）。
    pub name: String,
    /// 主体类别（`agent`/`program`/`human`）。
    pub kind: String,
}

/// roles 读模型行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 角色名（归一化后落库值；禁 admin 名由 schema CHECK 兜底）。
    pub name: String,
    /// 角色描述（可空）。
    pub description: Option<String>,
}

/// resources 读模型行（本库不存真实地址；敏感项 `vault://` 引用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 资源代号（归一化后落库值）。
    pub codename: String,
    /// 适配器标识。
    pub adapter: String,
    /// 传输标识。
    pub transport: String,
}

/// bindings 读模型行（主体↔角色）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 被绑定主体 id。
    pub principal_id: SnowflakeId,
    /// 被绑定角色 id。
    pub role_id: SnowflakeId,
}

/// binding_scope 读模型行（绑定辖区：`resource` 枚举 / `selector` 标签选择器，二选一）。
/// 一个绑定可有 0..N 条辖区行（resource 多枚举 / selector），故独立成行投影，**不**塞进
/// [`BindingRow`]（避免对 1:N 关系做有损的"单 scope"建模）。`resource_id`/`selector` 按
/// `kind` 二选一非空（store 忠实读出原始值；标签展开为资源集是 daemon 从快照投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingScopeRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 所属绑定 id。
    pub binding_id: SnowflakeId,
    /// 辖区种类（`resource`/`selector`，schema CHECK 兜底）。
    pub kind: String,
    /// 枚举资源 id（`kind = 'resource'` 时非空；`selector` 时为空）。
    pub resource_id: Option<SnowflakeId>,
    /// 标签选择器（`kind = 'selector'` 时非空；`resource` 时为空）。
    pub selector: Option<String>,
}

/// settings 读模型行（业务级标量配置键值对；限制性表）。元数据（默认值/是否可写/类型）
/// 不入库——由 daemon 按已知 key 定义；store 只忠实承载 `key`/`value` + 乐观锁 `version`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 业务键（partial unique，`WHERE delete_flag = 0`）。
    pub key: String,
    /// 当前值（NOT NULL 文本）。
    pub value: String,
}

/// grant_constraints 读模型行（对象细则；限制性表）。`id` 留 [`SnowflakeId`] 原始值，
/// daemon 侧再转 string。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 受约束资源 id（grant_constraints 上 NOT NULL）。
    pub resource_id: SnowflakeId,
    /// 受约束动词。
    pub capability: String,
    /// 约束种类。
    pub kind: String,
    /// 约束规格（可空 JSON 文本）。
    pub spec: Option<String>,
}

/// grant_conditions 读模型行（求值条件；限制性表）。`resource_id` / `capability`
/// 可空（资源级 / 全动词通用条件），`predicate` NOT NULL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 受约束资源 id（可空：空 = 全局通用条件）。
    pub resource_id: Option<SnowflakeId>,
    /// 受约束动词（可空：空 = 资源全动词通用）。
    pub capability: Option<String>,
    /// 求值谓词（NOT NULL）。
    pub predicate: String,
    /// 条件规格（可空 JSON 文本）。
    pub spec: Option<String>,
}

/// deny_notes 读模型行（人亲笔预写的拒绝说明；限制性表）。uq(resource_id, capability)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyNoteRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 受约束资源 id（deny_notes 上 NOT NULL）。
    pub resource_id: SnowflakeId,
    /// 受约束动词（NOT NULL）。
    pub capability: String,
    /// 拒绝说明（NOT NULL）。
    pub note: String,
}

/// mode_state 读模型行（辖区运行模式；限制性表）。`scope_resource_id` 可空
/// （`None` = 全局模式哨兵）；`expires_at` 可空。每辖区至多一行活跃（uq ON
/// COALESCE(scope_resource_id, 0)）。`effective_mode` 是 daemon 侧投影，store 不算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeStateRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 受约束辖区资源 id（`None` = 全局模式）。
    pub scope_resource_id: Option<SnowflakeId>,
    /// 模式文本（`normal`/`observe`/`maintain`/`freeze`，schema CHECK 兜底）。
    pub mode: String,
    /// 过期墙钟文本（可空：空 = 不过期）。
    pub expires_at: Option<String>,
}

/// temp_grants 读模型行（临时授权；终态字段 `ended_at`/`end_reason`）。`granted_at`/
/// `expires_at` NOT NULL（24 字节文本）；`ended_at`/`end_reason` 可空（活跃时为空，
/// 置终态后填 `expired`/`revoked`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempGrantRow {
    /// 主键。
    pub id: SnowflakeId,
    /// 乐观锁版本。
    pub version: i64,
    /// 被授权主体 id。
    pub principal_id: SnowflakeId,
    /// 被授权资源 id。
    pub resource_id: SnowflakeId,
    /// 被授权动词。
    pub capability: String,
    /// 授予墙钟（NOT NULL）。
    pub granted_at: String,
    /// 过期墙钟（NOT NULL）。
    pub expires_at: String,
    /// 终态墙钟（可空：活跃时为空）。
    pub ended_at: Option<String>,
    /// 终态原因（可空：`expired`/`revoked`）。
    pub end_reason: Option<String>,
}

/// 策略状态事务读写句柄：持 [`Db`] 与 [`IdGen`]，仅控制面（daemon::control +
/// sweeper）可达（§3.3 / §7-18）。
///
/// 全部写经一把进程内写互斥锁串行执行（由 [`Db::with_write_txn`] 提供临界区）；
/// 读只在快照重建与控制面读端点发生。本句柄**不**进入数据面依赖集合
/// （数据面只持 `PolicyView` 只读 + `AuditSink`，§7-17）。
pub struct PolicyRepo {
    db: Db,
    idgen: IdGen,
    clock: Box<dyn Clock>,
    /// 数据面消费的只读快照视图：写提交后在**同一写锁临界区**内经
    /// [`commit_and_rebuild`](PolicyRepo::commit_and_rebuild) 原子 `replace`。boot 物化
    /// 首份快照、`SnapshotView::new` 装配后注入此处，使写句柄成为快照重建的唯一驱动者
    /// （单一权威状态，无双源，§7-13）。`None` 时该句柄不重建快照（仅 per-entity 写 + 读）。
    view: Option<Arc<SnapshotView>>,
}

impl PolicyRepo {
    /// 以已开/已迁移的 [`Db`]、core [`IdGen`] 与墙钟 [`Clock`] 装配句柄（**不**持快照视图）。
    ///
    /// `clock` 是写入时 `now`（落 `created_at`/`updated_at`，经 `base` 唯一格式化点）的
    /// 来源；`idgen` 是主键来源。二者注入使写行为可在测试里确定复现。不持视图时
    /// [`commit_and_rebuild`](PolicyRepo::commit_and_rebuild) 仍递增 rev、构建快照并返回，
    /// 但不向任何 `SnapshotView` 发布（供仅需 per-entity 写的场景 / 既有调用点不破坏）。
    pub fn new(db: Db, idgen: IdGen, clock: Box<dyn Clock>) -> Self {
        Self {
            db,
            idgen,
            clock,
            view: None,
        }
    }

    /// 以已开/已迁移的 [`Db`]、[`IdGen`]、[`Clock`] 与既有 [`SnapshotView`] 装配句柄
    /// （持视图：写提交后在同一临界区内原子 `replace` 该视图）。
    ///
    /// **装配关系（谁持有 view / boot 怎么传入）**：boot 序列先 `migrate` → 以
    /// 当前持久 `policy_rev` 物化首份 [`PolicySnapshot`](postern_core::domain::PolicySnapshot)
    /// → `Arc::new(SnapshotView::new(Arc::new(first)))` 得唯一视图；该 `Arc<SnapshotView>`
    /// 一份克隆注入数据面 router（只读消费），另一份经本构造器交给控制面写句柄（写后重建
    /// 的唯一发布者）。二者共享同一 `arc-swap` 视图，故"写句柄 replace"对"数据面 snapshot()"
    /// 即时可见，且全程无双源。
    pub fn with_view(db: Db, idgen: IdGen, clock: Box<dyn Clock>, view: Arc<SnapshotView>) -> Self {
        Self {
            db,
            idgen,
            clock,
            view: Some(view),
        }
    }

    /// 借底层 [`Db`]（供测试在真实 SQLite 上核对落库形态；非跨 crate 公开语义）。
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 当前写入墙钟：经 `base` 唯一格式化点落 `created_at`/`updated_at`。
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_ms(self.clock.now_unix_ms())
    }

    /// 统一"提交 + 重建"编排（D2 写入主入口）：在**同一写锁临界区**内不可分地完成
    /// —— ① 经 `write`（在写事务内执行调用方给定的 `base::write` per-entity 写，自带乐观锁
    /// `expected_version`，返回该实体的**新** version）→ ② 经
    /// [`base::write::bump_policy_rev`](crate::base::write::bump_policy_rev) 原子递增持久
    /// `policy_rev` 得 `new_rev` → ③ COMMIT → ④
    /// [`build_snapshot`](crate::snapshot::build_snapshot)`(db, new_rev)` 物化新快照 →
    /// ⑤ 若持视图，`SnapshotView::replace(Arc::new(snapshot))` 原子发布。返回
    /// `(new_version, new_rev)`。
    ///
    /// **原子（全或无）**：①②同事务——`write` 返 [`StoreError::VersionConflict`]（乐观锁
    /// 版本冲突，供上层映射 `409`）或任何 `Err` ⇒ 整体 ROLLBACK，**既不改库、也不前进 rev、
    /// 也不换 snapshot**；并发读者经 `SnapshotView` 在本临界区释放前绝不见 torn 态（写未
    /// 提交、或 rev 与 snapshot 不一致都不可见——③之后④⑤仍在同一持有的锁内）。
    ///
    /// `write` 闭包是 store 提供的"执行某个 base::write 写 + 原子重建"能力的注入点：
    /// daemon 侧 `commit_write(actor, WriteIntent{entity, fields, expected_version})` 把
    /// entity/fields 解构成具体的 `base::write::{insert,update,logical_delete,...}` 调用塞进
    /// 此闭包（解构是 daemon 的事），本层只负责"在写事务内跑它 + 原子重建并发布"。
    pub fn commit_and_rebuild<W>(&self, write: W) -> Result<(i64, u64), StoreError>
    where
        W: FnOnce(&rusqlite::Transaction<'_>) -> Result<i64, StoreError>,
    {
        self.db.commit_and_rebuild(
            // 第一相（写事务内，COMMIT 前）：① 执行调用方给定的 per-entity base::write
            // （自带乐观锁，返回该实体新 version）；任一 Err（含 VersionConflict）⇒ 整体
            // ROLLBACK 且不进第二相（rev 不进、快照不换——全或无）。② 同事务内
            // base::write::bump_policy_rev 原子 +1 得 new_rev。两者共 COMMIT/ROLLBACK 边界。
            |txn| {
                let new_version = write(txn)?;
                let new_rev = write::bump_policy_rev(txn)?;
                Ok((new_version, new_rev))
            },
            // 第二相（COMMIT 后，同一持有的写锁内、于刚提交的连接上）：③ 以 new_rev 物化
            // 新快照（build_snapshot_on 复用本连接，绝不二次取锁）；④ 若持视图，原子 replace
            // 发布。读者在本临界区释放前绝不见 torn 态（rev 与 snapshot 始终一致）。
            |conn, (new_version, new_rev)| {
                let snapshot = build_snapshot_on(conn, new_rev)?;
                if let Some(view) = &self.view {
                    view.replace(Arc::new(snapshot));
                }
                Ok((new_version, new_rev))
            },
        )
    }

    // ----------------------------------------------- per-entity 写 + 原子重建（D2b 写接缝）

    // 以下 `*_and_rebuild` 是 D2b **写接缝的 store 侧**：把既有 per-entity `base::write`
    // 调用塞进 [`commit_and_rebuild`](PolicyRepo::commit_and_rebuild) 的写闭包，使「实体写 +
    // bump_policy_rev + COMMIT + build_snapshot + SnapshotView::replace」在同一写锁临界区内
    // 原子完成（全或无：乐观锁冲突 / 任何 Err ⇒ 整体 ROLLBACK，rev 不进、快照不换）。返回
    // `(new_version, new_rev)`——daemon control::PolicyRepo 适配器据此组装 `WriteOutcome`。
    //
    // 与既有 `create_*`/`delete_*`（各自 `with_write_txn`、**不**重建）并存：那些供仅需
    // per-entity 写、不重建快照的既有调用点；控制面写端点的三联动一律经此 `*_and_rebuild`。
    // DB 写一律经 `base::write`（契约 `DB_WRITE_PATH_CENTRALIZED`），本层零原始 SQL。
    //
    // version 语义：INSERT 落新行 `version = 0`（[`base::write::insert`] 固定），故新增类
    // 返回 `0`；UPDATE / 逻辑删除恒 `version = version + 1`，故返回 `expected_version + 1`。

    /// 新增主体 + 原子重建：在同一临界区内经 `base::write::insert` 落一行 principals、
    /// bump rev、重建并发布快照。返回 `(新行 version=0, 新 rev)`。
    pub fn create_principal_and_rebuild(
        &self,
        actor: &Actor,
        name: &str,
        kind: &str,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "principals",
                    columns: vec!["name", "kind"],
                    values: vec![Value::Text(name.to_string()), Value::Text(kind.to_string())],
                    enable_flag: 1,
                },
            )?;
            // INSERT 固定落 version = 0（base::write::insert）；新行版本即乐观锁下一期望前驱。
            Ok(0)
        })
    }

    /// 新增角色 + 原子重建。返回 `(新行 version=0, 新 rev)`。
    pub fn create_role_and_rebuild(
        &self,
        actor: &Actor,
        name: &str,
        description: Option<&str>,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        let desc = match description {
            Some(d) => Value::Text(d.to_string()),
            None => Value::Null,
        };
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "roles",
                    columns: vec!["name", "description"],
                    values: vec![Value::Text(name.to_string()), desc],
                    enable_flag: 1,
                },
            )?;
            Ok(0)
        })
    }

    /// 新增资源 + 原子重建。返回 `(新行 version=0, 新 rev)`。
    pub fn create_resource_and_rebuild(
        &self,
        actor: &Actor,
        codename: &str,
        adapter: &str,
        transport: &str,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "resources",
                    columns: vec!["codename", "adapter", "transport"],
                    values: vec![
                        Value::Text(codename.to_string()),
                        Value::Text(adapter.to_string()),
                        Value::Text(transport.to_string()),
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(0)
        })
    }

    /// 绑定主体到角色 + 原子重建。返回 `(新行 version=0, 新 rev)`。
    pub fn create_binding_and_rebuild(
        &self,
        actor: &Actor,
        principal_id: SnowflakeId,
        role_id: SnowflakeId,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "bindings",
                    columns: vec!["principal_id", "role_id"],
                    values: vec![
                        Value::Integer(principal_id.as_raw() as i64),
                        Value::Integer(role_id.as_raw() as i64),
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(0)
        })
    }

    /// 绑定主体到角色 + 同事务写一条辖区 + 原子重建：在**同一** `commit_and_rebuild`
    /// 闭包内先经 `base::write::insert` 落一行 bindings，再以其新 id 落一行 binding_scope
    /// （`kind` = `resource`/`selector`，`resource_id`/`selector` 二选一），最后 bump rev、
    /// 重建并发布快照。任一步 Err（含 partial unique 冲突）⇒ 整体 ROLLBACK（绑定与辖区
    /// 均不留、rev 不进、快照不换）。`scope_resource_id` 与 `scope_selector` 由调用方按
    /// `kind` 二选一提供（store 忠实落库，二选一语义由上层保证）。返回 `(新绑定 version=0, 新 rev)`。
    pub fn create_binding_with_scope_and_rebuild(
        &self,
        actor: &Actor,
        principal_id: SnowflakeId,
        role_id: SnowflakeId,
        scope_kind: &str,
        scope_resource_id: Option<SnowflakeId>,
        scope_selector: Option<&str>,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        let res_val = match scope_resource_id {
            Some(r) => Value::Integer(r.as_raw() as i64),
            None => Value::Null,
        };
        let sel_val = match scope_selector {
            Some(s) => Value::Text(s.to_string()),
            None => Value::Null,
        };
        self.commit_and_rebuild(|txn| {
            let binding_id = write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "bindings",
                    columns: vec!["principal_id", "role_id"],
                    values: vec![
                        Value::Integer(principal_id.as_raw() as i64),
                        Value::Integer(role_id.as_raw() as i64),
                    ],
                    enable_flag: 1,
                },
            )?;
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "binding_scope",
                    columns: vec!["binding_id", "kind", "resource_id", "selector"],
                    values: vec![
                        Value::Integer(binding_id.as_raw() as i64),
                        Value::Text(scope_kind.to_string()),
                        res_val,
                        sel_val,
                    ],
                    enable_flag: 1,
                },
            )?;
            // 新绑定 INSERT 固定落 version = 0（base::write::insert）。
            Ok(0)
        })
    }

    /// 改名主体（乐观锁）+ 原子重建：期望 `expected_version` 不符 ⇒ `VersionConflict`、
    /// 整体 ROLLBACK（rev 不进、快照不换）。成功返回 `(expected_version + 1, 新 rev)`。
    pub fn rename_principal_and_rebuild(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
        new_name: &str,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::update(
                txn,
                now,
                actor,
                "principals",
                id,
                expected_version,
                vec!["name"],
                vec![Value::Text(new_name.to_string())],
                None,
            )?;
            // 乐观锁 UPDATE 恒 version = version + 1：新版本即 expected_version + 1。
            Ok(expected_version + 1)
        })
    }

    /// 新增对象细则 + 原子重建：经 `base::write::insert` 落一行 grant_constraints
    /// （`resource_id`/`capability`/`kind`/`spec`；限制性表 `enable_flag` 固定 1）。
    /// 返回 `(新行 version=0, 新 rev)`。
    pub fn create_constraint_and_rebuild(
        &self,
        actor: &Actor,
        resource_id: SnowflakeId,
        capability: &str,
        kind: &str,
        spec: Option<&str>,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        let spec_val = match spec {
            Some(s) => Value::Text(s.to_string()),
            None => Value::Null,
        };
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "grant_constraints",
                    columns: vec!["resource_id", "capability", "kind", "spec"],
                    values: vec![
                        Value::Integer(resource_id.as_raw() as i64),
                        Value::Text(capability.to_string()),
                        Value::Text(kind.to_string()),
                        spec_val,
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(0)
        })
    }

    /// 逻辑删除对象细则（乐观锁）+ 原子重建：期望 `expected_version` 不符 ⇒
    /// `VersionConflict`、整体 ROLLBACK（rev 不进、快照不换）。成功返回
    /// `(expected_version + 1, 新 rev)`。
    pub fn delete_constraint_and_rebuild(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::logical_delete(txn, now, actor, "grant_constraints", id, expected_version)?;
            Ok(expected_version + 1)
        })
    }

    /// 新增求值条件 + 原子重建：经 `base::write::insert` 落一行 grant_conditions
    /// （`resource_id`/`capability` 可空、`predicate` NOT NULL、`spec` 可空；
    /// 限制性表 `enable_flag` 固定 1）。返回 `(新行 version=0, 新 rev)`。
    pub fn create_condition_and_rebuild(
        &self,
        actor: &Actor,
        resource_id: Option<SnowflakeId>,
        capability: Option<&str>,
        predicate: &str,
        spec: Option<&str>,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        let res_val = match resource_id {
            Some(r) => Value::Integer(r.as_raw() as i64),
            None => Value::Null,
        };
        let cap_val = match capability {
            Some(c) => Value::Text(c.to_string()),
            None => Value::Null,
        };
        let spec_val = match spec {
            Some(s) => Value::Text(s.to_string()),
            None => Value::Null,
        };
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "grant_conditions",
                    columns: vec!["resource_id", "capability", "predicate", "spec"],
                    values: vec![
                        res_val,
                        cap_val,
                        Value::Text(predicate.to_string()),
                        spec_val,
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(0)
        })
    }

    /// 逻辑删除求值条件（乐观锁）+ 原子重建：期望 `expected_version` 不符 ⇒
    /// `VersionConflict`、整体 ROLLBACK。成功返回 `(expected_version + 1, 新 rev)`。
    pub fn delete_condition_and_rebuild(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::logical_delete(txn, now, actor, "grant_conditions", id, expected_version)?;
            Ok(expected_version + 1)
        })
    }

    /// 新增拒绝说明 + 原子重建：经 `base::write::insert` 落一行 deny_notes
    /// （`resource_id`/`capability`/`note` 均 NOT NULL；限制性表 `enable_flag` 固定 1）。
    /// 同 `(resource_id, capability)` 重复（`delete_flag=0`）→ uq 拒 →
    /// [`StoreError::ConstraintViolation`]，整体 ROLLBACK（rev 不进、快照不换）。
    /// 返回 `(新行 version=0, 新 rev)`。
    pub fn create_deny_note_and_rebuild(
        &self,
        actor: &Actor,
        resource_id: SnowflakeId,
        capability: &str,
        note: &str,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "deny_notes",
                    columns: vec!["resource_id", "capability", "note"],
                    values: vec![
                        Value::Integer(resource_id.as_raw() as i64),
                        Value::Text(capability.to_string()),
                        Value::Text(note.to_string()),
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(0)
        })
    }

    /// 逻辑删除拒绝说明（乐观锁）+ 原子重建：期望 `expected_version` 不符 ⇒
    /// `VersionConflict`、整体 ROLLBACK。成功返回 `(expected_version + 1, 新 rev)`。
    pub fn delete_deny_note_and_rebuild(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            write::logical_delete(txn, now, actor, "deny_notes", id, expected_version)?;
            Ok(expected_version + 1)
        })
    }

    // ----------------------------------------------- mode（upsert）+ 原子重建

    /// 设置辖区运行模式（**upsert**）+ 原子重建：按 uq `ON COALESCE(scope_resource_id, 0)`
    /// 的语义，若该辖区已有活跃行（`delete_flag=0`）则经 `base::write::update` 乐观锁
    /// 改其 `mode`/`expires_at`（version 自增）；否则经 `base::write::insert` 落新行。
    /// 全局辖区以 `scope_resource_id = None`（NULL 哨兵）表达。
    ///
    /// store 忠实落入调用方给定的 `mode`/`expires_at`（收窄语义由上层保证）。返回
    /// `(新 version, 新 rev)`：插入分支 `version = 0`，更新分支 `version = 既有 + 1`。
    /// `expires_at` 由调用方按需提供（mode_state.expires_at 无固定宽度约束，如实落库）。
    pub fn set_mode_and_rebuild(
        &self,
        actor: &Actor,
        scope_resource_id: Option<SnowflakeId>,
        mode: &str,
        expires_at: Option<&str>,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        let exp_val = match expires_at {
            Some(e) => Value::Text(e.to_string()),
            None => Value::Null,
        };
        self.commit_and_rebuild(|txn| {
            // 在写事务内按辖区哨兵（COALESCE(scope_resource_id, 0)）查既有活跃行
            // （id, version）以决定 insert vs update —— 写一律经 base::write。
            let sentinel = scope_resource_id.map(|r| r.as_raw() as i64).unwrap_or(0);
            let existing: Option<(i64, i64)> = txn
                .query_row(
                    "SELECT id, version FROM mode_state \
                     WHERE COALESCE(scope_resource_id, 0) = ?1 AND delete_flag = 0 LIMIT 1",
                    [sentinel],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();

            match existing {
                Some((id_raw, version)) => {
                    // 既有行 → 乐观锁就地改 mode/expires_at（收窄 upsert）。
                    write::update(
                        txn,
                        now,
                        actor,
                        "mode_state",
                        SnowflakeId::from_raw(id_raw as u64),
                        version,
                        vec!["mode", "expires_at"],
                        vec![Value::Text(mode.to_string()), exp_val],
                        None,
                    )?;
                    Ok(version + 1)
                }
                None => {
                    // 新辖区 → 插新行（限制性表 enable_flag 固定 1）。
                    let scope_val = match scope_resource_id {
                        Some(r) => Value::Integer(r.as_raw() as i64),
                        None => Value::Null,
                    };
                    write::insert(
                        txn,
                        &self.idgen,
                        now,
                        actor,
                        InsertRow {
                            table: "mode_state",
                            columns: vec!["scope_resource_id", "mode", "expires_at"],
                            values: vec![scope_val, Value::Text(mode.to_string()), exp_val],
                            enable_flag: 1,
                        },
                    )?;
                    Ok(0)
                }
            }
        })
    }

    // ----------------------------------------------- settings（upsert by key）+ 原子重建

    /// 设置一个业务级配置项（**upsert by key**）+ 原子重建：按 `key` 的 partial unique
    /// （`WHERE delete_flag = 0`）语义，若该 key 已有活跃行则经 `base::write::update` 乐观锁
    /// 改其 `value`（version 自增）；否则经 `base::write::insert` 落新行。元数据
    /// （默认值/是否可写/类型）不入库——由 daemon 按已知 key 定义；store 只忠实落 `value`。
    ///
    /// 返回 `(新 version, 新 rev)`：插入分支 `version = 0`，更新分支 `version = 既有 + 1`。
    /// 写一律经 `base::write`（限制性表 `enable_flag` 固定 1，唯一写路径）；写事务内按
    /// key 查既有活跃行（id, version）以决定 insert vs update（带 `delete_flag = 0` + `LIMIT 1`）。
    pub fn set_setting_and_rebuild(
        &self,
        actor: &Actor,
        key: &str,
        value: &str,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        self.commit_and_rebuild(|txn| {
            // 在写事务内按 key 查既有活跃行（id, version）以决定 insert vs update——写一律经
            // base::write（本闭包仅省读以分流，默认作用域 delete_flag = 0 + LIMIT 1）。
            let existing: Option<(i64, i64)> = txn
                .query_row(
                    "SELECT id, version FROM settings \
                     WHERE key = ?1 AND delete_flag = 0 LIMIT 1",
                    [key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();

            match existing {
                Some((id_raw, version)) => {
                    // 既有 key → 乐观锁就地改 value（同 key 再次写改既有行，不新增）。
                    write::update(
                        txn,
                        now,
                        actor,
                        "settings",
                        SnowflakeId::from_raw(id_raw as u64),
                        version,
                        vec!["value"],
                        vec![Value::Text(value.to_string())],
                        None,
                    )?;
                    Ok(version + 1)
                }
                None => {
                    // 新 key → 插新行（限制性表 enable_flag 固定 1）。
                    write::insert(
                        txn,
                        &self.idgen,
                        now,
                        actor,
                        InsertRow {
                            table: "settings",
                            columns: vec!["key", "value"],
                            values: vec![
                                Value::Text(key.to_string()),
                                Value::Text(value.to_string()),
                            ],
                            enable_flag: 1,
                        },
                    )?;
                    Ok(0)
                }
            }
        })
    }

    // ----------------------------------------------- grants（temp_grants）+ 原子重建

    /// 临时提权（直授）+ 原子重建：经 `base::write::insert` 落一行 temp_grants
    /// （`principal_id`/`resource_id`/`capability`/`granted_at`/`expires_at`）。
    /// `granted_at = now`、`expires_at = now + ttl_ms`，二者经唯一格式化点
    /// （[`base::timestamp::format`](crate::base::timestamp::format)）落 24 字节文本。
    /// `ended_at`/`end_reason` 留空（活跃）。返回 `(新行 version=0, 新 rev)`。
    pub fn elevate_grant_and_rebuild(
        &self,
        actor: &Actor,
        principal_id: SnowflakeId,
        resource_id: SnowflakeId,
        capability: &str,
        ttl_ms: u64,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        let granted_at = timestamp::format(now);
        let expires_at = timestamp::format(Timestamp::from_unix_ms(
            now.as_unix_ms().saturating_add(ttl_ms),
        ));
        self.commit_and_rebuild(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "temp_grants",
                    columns: vec![
                        "principal_id",
                        "resource_id",
                        "capability",
                        "granted_at",
                        "expires_at",
                    ],
                    values: vec![
                        Value::Integer(principal_id.as_raw() as i64),
                        Value::Integer(resource_id.as_raw() as i64),
                        Value::Text(capability.to_string()),
                        Value::Text(granted_at),
                        Value::Text(expires_at),
                    ],
                    enable_flag: 1,
                },
            )?;
            Ok(0)
        })
    }

    /// 撤销临时授权（乐观锁）+ 原子重建：经 `base::write::update` 置该行终态
    /// `ended_at = now`、`end_reason = 'revoked'`（version 自增）。期望
    /// `expected_version` 不符 ⇒ [`StoreError::VersionConflict`]、整体 ROLLBACK
    /// （rev 不进、快照不换）。成功返回 `(expected_version + 1, 新 rev)`。
    pub fn revoke_grant_and_rebuild(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
    ) -> Result<(i64, u64), StoreError> {
        let now = self.now();
        let ended_at = timestamp::format(now);
        self.commit_and_rebuild(|txn| {
            write::update(
                txn,
                now,
                actor,
                "temp_grants",
                id,
                expected_version,
                vec!["ended_at", "end_reason"],
                vec![Value::Text(ended_at), Value::Text("revoked".to_string())],
                None,
            )?;
            Ok(expected_version + 1)
        })
    }

    // ---------------------------------------------------------------- principals

    /// 新增一个主体：在写事务内经 `base::write::insert` 落一行 principals
    /// （业务列 `name`/`kind`；审计字段自动填充、`version=0`、归一化入库）。返回新行 id。
    ///
    /// `name` 经 `base` 归一化；重复（归一化后相同、`delete_flag=0`）→ partial unique
    /// 拒 → [`StoreError::ConstraintViolation`]，库不变。
    pub fn create_principal(
        &self,
        actor: &Actor,
        name: &str,
        kind: &str,
    ) -> Result<SnowflakeId, StoreError> {
        let now = self.now();
        self.db.with_write_txn(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "principals",
                    columns: vec!["name", "kind"],
                    values: vec![Value::Text(name.to_string()), Value::Text(kind.to_string())],
                    enable_flag: 1,
                },
            )
        })
    }

    /// 改名一个主体（乐观锁）：经 `base::write::update` 更新 `name`，期望
    /// `expected_version` 由调用方传入；影响 0 行 → [`StoreError::VersionConflict`]。
    pub fn rename_principal(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
        new_name: &str,
    ) -> Result<(), StoreError> {
        let now = self.now();
        self.db.with_write_txn(|txn| {
            write::update(
                txn,
                now,
                actor,
                "principals",
                id,
                expected_version,
                vec!["name"],
                vec![Value::Text(new_name.to_string())],
                None,
            )
        })
    }

    /// 逻辑删除一个主体（乐观锁 + 级联）：在**同一事务**内经
    /// `base::write::logical_delete` 置该行 `delete_flag=1`，并经
    /// `base::write::cascade_logical_delete` 级联把 `credentials/bindings/temp_grants`
    /// 的直接子行 `delete_flag=1`、`updated_by` 标 `cascade:principals#<id>`（§3.2）。
    /// 期望 version 不符 → [`StoreError::VersionConflict`]，整体 ROLLBACK（父子不变）。
    pub fn delete_principal(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
    ) -> Result<(), StoreError> {
        let now = self.now();
        self.db.with_write_txn(|txn| {
            // 先乐观锁删父（期望 version 不符 → 影响 0 行 → 冲突 → 整体 ROLLBACK）。
            write::logical_delete(txn, now, actor, "principals", id, expected_version)?;
            // 同事务级联子行（§3.2：principals → {credentials, bindings, temp_grants}）。
            for (child, fk) in [
                ("credentials", "principal_id"),
                ("bindings", "principal_id"),
                ("temp_grants", "principal_id"),
            ] {
                write::cascade_logical_delete(txn, now, "principals", id, child, fk)?;
            }
            Ok(())
        })
    }

    /// 分页列出主体（默认作用域 `delete_flag=0`、`LIMIT` 封顶）。返回携 `version`
    /// 的 [`PrincipalRow`] 信封；`page_size` 经 `clamp`。
    pub fn list_principals(&self, page: PageQuery) -> Result<Page<PrincipalRow>, StoreError> {
        let list = "SELECT id, version, name, kind FROM principals \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM principals WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, map_principal)
    }

    /// 按 id 取单个主体（默认作用域 `delete_flag=0`：已逻辑删除的行返回 `None`）。
    pub fn get_principal(&self, id: SnowflakeId) -> Result<Option<PrincipalRow>, StoreError> {
        let q = "SELECT id, version, name, kind FROM principals \
                 WHERE id = ?1 AND delete_flag = 0 LIMIT 1";
        self.db.with_read(|conn| {
            let row = conn
                .query_row(q, [id.as_raw() as i64], |r| {
                    Ok(PrincipalRow {
                        id: SnowflakeId::from_raw(r.get::<_, i64>(0)? as u64),
                        version: r.get(1)?,
                        name: r.get(2)?,
                        kind: r.get(3)?,
                    })
                })
                .ok();
            Ok(row)
        })
    }

    // ---------------------------------------------------------------- roles

    /// 新增一个角色：经 `base::write::insert` 落一行 roles（业务列 `name`/`description`）。
    /// `name` 归一化；写 `admin`（任意大小写/空白）→ schema `CHECK` 拒 →
    /// [`StoreError::ConstraintViolation`]，库不变（`SEC_ADMIN_NOT_GRANTABLE`）。
    pub fn create_role(
        &self,
        actor: &Actor,
        name: &str,
        description: Option<&str>,
    ) -> Result<SnowflakeId, StoreError> {
        let now = self.now();
        let desc = match description {
            Some(d) => Value::Text(d.to_string()),
            None => Value::Null,
        };
        self.db.with_write_txn(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "roles",
                    columns: vec!["name", "description"],
                    values: vec![Value::Text(name.to_string()), desc],
                    enable_flag: 1,
                },
            )
        })
    }

    /// 逻辑删除一个角色（乐观锁 + 级联）：同事务内置该行 `delete_flag=1` 并级联
    /// `role_inherits/role_capabilities/bindings` 子行（§3.2）。
    pub fn delete_role(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
    ) -> Result<(), StoreError> {
        let now = self.now();
        self.db.with_write_txn(|txn| {
            write::logical_delete(txn, now, actor, "roles", id, expected_version)?;
            // §3.2：roles → {role_inherits, role_capabilities, bindings}。
            for (child, fk) in [
                ("role_inherits", "role_id"),
                ("role_capabilities", "role_id"),
                ("bindings", "role_id"),
            ] {
                write::cascade_logical_delete(txn, now, "roles", id, child, fk)?;
            }
            Ok(())
        })
    }

    /// 分页列出角色（默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    pub fn list_roles(&self, page: PageQuery) -> Result<Page<RoleRow>, StoreError> {
        let list = "SELECT id, version, name, description FROM roles \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM roles WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(RoleRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                name: r.get(2).map_err(|_| StoreError::Io)?,
                description: r.get(3).map_err(|_| StoreError::Io)?,
            })
        })
    }

    // ---------------------------------------------------------------- resources

    /// 新增一个资源：经 `base::write::insert` 落一行 resources（业务列
    /// `codename`/`adapter`/`transport`）。`codename` 归一化；重复 → partial unique 拒。
    pub fn create_resource(
        &self,
        actor: &Actor,
        codename: &str,
        adapter: &str,
        transport: &str,
    ) -> Result<SnowflakeId, StoreError> {
        let now = self.now();
        self.db.with_write_txn(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "resources",
                    columns: vec!["codename", "adapter", "transport"],
                    values: vec![
                        Value::Text(codename.to_string()),
                        Value::Text(adapter.to_string()),
                        Value::Text(transport.to_string()),
                    ],
                    enable_flag: 1,
                },
            )
        })
    }

    /// 逻辑删除一个资源（乐观锁 + 级联）：同事务内置该行 `delete_flag=1` 并级联
    /// `resource_credential_tiers/binding_scope/grant_constraints/grant_conditions/
    /// mode_state/deny_notes/resource_labels` 子行（§3.2）。
    pub fn delete_resource(
        &self,
        actor: &Actor,
        id: SnowflakeId,
        expected_version: i64,
    ) -> Result<(), StoreError> {
        let now = self.now();
        self.db.with_write_txn(|txn| {
            write::logical_delete(txn, now, actor, "resources", id, expected_version)?;
            // §3.2：resources → {resource_credential_tiers, binding_scope, grant_constraints,
            // grant_conditions, mode_state(scope_resource_id), deny_notes, resource_labels}。
            for (child, fk) in [
                ("resource_credential_tiers", "resource_id"),
                ("binding_scope", "resource_id"),
                ("grant_constraints", "resource_id"),
                ("grant_conditions", "resource_id"),
                ("mode_state", "scope_resource_id"),
                ("deny_notes", "resource_id"),
                ("resource_labels", "resource_id"),
            ] {
                write::cascade_logical_delete(txn, now, "resources", id, child, fk)?;
            }
            Ok(())
        })
    }

    /// 分页列出资源（默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    pub fn list_resources(&self, page: PageQuery) -> Result<Page<ResourceRow>, StoreError> {
        let list = "SELECT id, version, codename, adapter, transport FROM resources \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM resources WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(ResourceRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                codename: r.get(2).map_err(|_| StoreError::Io)?,
                adapter: r.get(3).map_err(|_| StoreError::Io)?,
                transport: r.get(4).map_err(|_| StoreError::Io)?,
            })
        })
    }

    // ---------------------------------------------------------------- bindings

    /// 绑定主体到角色：经 `base::write::insert` 落一行 bindings（业务列
    /// `principal_id`/`role_id`）。同对重复（`delete_flag=0`）→ partial unique 拒。
    pub fn create_binding(
        &self,
        actor: &Actor,
        principal_id: SnowflakeId,
        role_id: SnowflakeId,
    ) -> Result<SnowflakeId, StoreError> {
        let now = self.now();
        self.db.with_write_txn(|txn| {
            write::insert(
                txn,
                &self.idgen,
                now,
                actor,
                InsertRow {
                    table: "bindings",
                    columns: vec!["principal_id", "role_id"],
                    values: vec![
                        Value::Integer(principal_id.as_raw() as i64),
                        Value::Integer(role_id.as_raw() as i64),
                    ],
                    enable_flag: 1,
                },
            )
        })
    }

    /// 分页列出某主体的绑定（默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    pub fn list_bindings_of(
        &self,
        principal_id: SnowflakeId,
        page: PageQuery,
    ) -> Result<Page<BindingRow>, StoreError> {
        let list = "SELECT id, version, principal_id, role_id FROM bindings \
                    WHERE principal_id = ?3 AND delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM bindings \
                     WHERE principal_id = ?1 AND delete_flag = 0";
        execute_page_filtered(
            &self.db,
            list,
            count,
            principal_id.as_raw() as i64,
            page,
            |r| {
                Ok(BindingRow {
                    id: SnowflakeId::from_raw(
                        r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64
                    ),
                    version: r.get(1).map_err(|_| StoreError::Io)?,
                    principal_id: SnowflakeId::from_raw(
                        r.get::<_, i64>(2).map_err(|_| StoreError::Io)? as u64,
                    ),
                    role_id: SnowflakeId::from_raw(
                        r.get::<_, i64>(3).map_err(|_| StoreError::Io)? as u64
                    ),
                })
            },
        )
    }

    /// 分页列出**全部**绑定（**无主体过滤**；默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    /// 与 [`list_bindings_of`](PolicyRepo::list_bindings_of)（按主体过滤）互补，供控制面
    /// 全量列读多主体绑定。`principal`/`role` 名与 `expanded_resources` 是 daemon 从快照
    /// 投影，store 只给 id + version。
    pub fn list_bindings(&self, page: PageQuery) -> Result<Page<BindingRow>, StoreError> {
        let list = "SELECT id, version, principal_id, role_id FROM bindings \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM bindings WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(BindingRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                principal_id: SnowflakeId::from_raw(
                    r.get::<_, i64>(2).map_err(|_| StoreError::Io)? as u64,
                ),
                role_id: SnowflakeId::from_raw(
                    r.get::<_, i64>(3).map_err(|_| StoreError::Io)? as u64
                ),
            })
        })
    }

    /// 分页列出绑定辖区（默认作用域 `delete_flag=0`、`LIMIT` 封顶）。一个绑定可有 0..N
    /// 条辖区行，逐条如实读出（`resource_id`/`selector` 按 `kind` 二选一非空；标签展开为
    /// 资源集是 daemon 从快照投影，store 只给原始 scope）。
    pub fn list_binding_scopes(
        &self,
        page: PageQuery,
    ) -> Result<Page<BindingScopeRow>, StoreError> {
        let list =
            "SELECT id, version, binding_id, kind, resource_id, selector FROM binding_scope \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM binding_scope WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(BindingScopeRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                binding_id: SnowflakeId::from_raw(
                    r.get::<_, i64>(2).map_err(|_| StoreError::Io)? as u64
                ),
                kind: r.get(3).map_err(|_| StoreError::Io)?,
                resource_id: r
                    .get::<_, Option<i64>>(4)
                    .map_err(|_| StoreError::Io)?
                    .map(|v| SnowflakeId::from_raw(v as u64)),
                selector: r.get(5).map_err(|_| StoreError::Io)?,
            })
        })
    }

    // ---------------------------------------------------------------- settings

    /// 分页列出业务级配置项（默认作用域 `delete_flag=0`、`LIMIT` 封顶）。返回携 `version`
    /// 的 [`SettingRow`]（`key`/`value`）；元数据（默认值/是否可写/类型）不入库，由 daemon
    /// 按已知 key 定义。
    pub fn list_settings(&self, page: PageQuery) -> Result<Page<SettingRow>, StoreError> {
        let list = "SELECT id, version, key, value FROM settings \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM settings WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(SettingRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                key: r.get(2).map_err(|_| StoreError::Io)?,
                value: r.get(3).map_err(|_| StoreError::Io)?,
            })
        })
    }

    // ---------------------------------------------------------------- constraints / conditions / deny-notes

    /// 分页列出对象细则（限制性表；默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    /// 返回携 `version` 的 [`ConstraintRow`]（`id` 留 [`SnowflakeId`]，daemon 再转 string）。
    pub fn list_constraints(&self, page: PageQuery) -> Result<Page<ConstraintRow>, StoreError> {
        let list =
            "SELECT id, version, resource_id, capability, kind, spec FROM grant_constraints \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM grant_constraints WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(ConstraintRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                resource_id: SnowflakeId::from_raw(
                    r.get::<_, i64>(2).map_err(|_| StoreError::Io)? as u64
                ),
                capability: r.get(3).map_err(|_| StoreError::Io)?,
                kind: r.get(4).map_err(|_| StoreError::Io)?,
                spec: r.get(5).map_err(|_| StoreError::Io)?,
            })
        })
    }

    /// 分页列出求值条件（限制性表；默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    /// `resource_id` / `capability` 可空（资源级 / 全动词通用条件），如实读出。
    pub fn list_conditions(&self, page: PageQuery) -> Result<Page<ConditionRow>, StoreError> {
        let list =
            "SELECT id, version, resource_id, capability, predicate, spec FROM grant_conditions \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM grant_conditions WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(ConditionRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                resource_id: r
                    .get::<_, Option<i64>>(2)
                    .map_err(|_| StoreError::Io)?
                    .map(|v| SnowflakeId::from_raw(v as u64)),
                capability: r.get(3).map_err(|_| StoreError::Io)?,
                predicate: r.get(4).map_err(|_| StoreError::Io)?,
                spec: r.get(5).map_err(|_| StoreError::Io)?,
            })
        })
    }

    /// 分页列出拒绝说明（限制性表；默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    pub fn list_deny_notes(&self, page: PageQuery) -> Result<Page<DenyNoteRow>, StoreError> {
        let list = "SELECT id, version, resource_id, capability, note FROM deny_notes \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM deny_notes WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(DenyNoteRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                resource_id: SnowflakeId::from_raw(
                    r.get::<_, i64>(2).map_err(|_| StoreError::Io)? as u64
                ),
                capability: r.get(3).map_err(|_| StoreError::Io)?,
                note: r.get(4).map_err(|_| StoreError::Io)?,
            })
        })
    }

    // ---------------------------------------------------------------- mode_state / temp_grants

    /// 分页列出辖区运行模式（限制性表；默认作用域 `delete_flag=0`、`LIMIT` 封顶）。
    /// `scope_resource_id` 可空（`None` = 全局模式），如实读出。`effective_mode` 是
    /// daemon 侧投影，store 不算（本读法仅忠实返回各辖区落库的 `mode`/`expires_at`）。
    pub fn list_mode_state(&self, page: PageQuery) -> Result<Page<ModeStateRow>, StoreError> {
        let list = "SELECT id, version, scope_resource_id, mode, expires_at FROM mode_state \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM mode_state WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(ModeStateRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                scope_resource_id: r
                    .get::<_, Option<i64>>(2)
                    .map_err(|_| StoreError::Io)?
                    .map(|v| SnowflakeId::from_raw(v as u64)),
                mode: r.get(3).map_err(|_| StoreError::Io)?,
                expires_at: r.get(4).map_err(|_| StoreError::Io)?,
            })
        })
    }

    /// 分页列出临时授权（默认作用域 `delete_flag=0`、`LIMIT` 封顶）。终态字段
    /// `ended_at`/`end_reason` 可空（活跃时为空），如实读出——撤销/过期的行仍在列表
    /// （终态、非逻辑删除）。
    pub fn list_temp_grants(&self, page: PageQuery) -> Result<Page<TempGrantRow>, StoreError> {
        let list = "SELECT id, version, principal_id, resource_id, capability, granted_at, \
                    expires_at, ended_at, end_reason FROM temp_grants \
                    WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
        let count = "SELECT COUNT(*) FROM temp_grants WHERE delete_flag = 0";
        execute_page(&self.db, list, count, page, |r| {
            Ok(TempGrantRow {
                id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
                version: r.get(1).map_err(|_| StoreError::Io)?,
                principal_id: SnowflakeId::from_raw(
                    r.get::<_, i64>(2).map_err(|_| StoreError::Io)? as u64,
                ),
                resource_id: SnowflakeId::from_raw(
                    r.get::<_, i64>(3).map_err(|_| StoreError::Io)? as u64
                ),
                capability: r.get(4).map_err(|_| StoreError::Io)?,
                granted_at: r.get(5).map_err(|_| StoreError::Io)?,
                expires_at: r.get(6).map_err(|_| StoreError::Io)?,
                ended_at: r.get(7).map_err(|_| StoreError::Io)?,
                end_reason: r.get(8).map_err(|_| StoreError::Io)?,
            })
        })
    }

    /// 主体的 `your_grants` 投影：从**已物化快照**（`SnapshotView::snapshot()`）取该主体
    /// 在授权空间里的 `resource → capability[]`（物化已含 binding×角色×辖区 ∪ 有效
    /// temp_grants，见 [`build_snapshot`](crate::snapshot::build_snapshot)）。store 不在此处
    /// 重算授权——只读快照里该主体的格、按资源聚合其动词集（每资源去重、有序）。
    ///
    /// 未持视图（[`PolicyRepo::new`]，无 `SnapshotView`）或该主体无任何格 → 空映射。
    pub fn your_grants_view(
        &self,
        principal_id: SnowflakeId,
    ) -> BTreeMap<ResourceCode, Vec<Capability>> {
        let mut out: BTreeMap<ResourceCode, Vec<Capability>> = BTreeMap::new();
        let Some(view) = &self.view else {
            return out; // 不持视图 → 无可投影的物化快照
        };
        let snapshot = view.snapshot();
        let principal = PrincipalId::new(principal_id);
        let Some(cells) = snapshot.grants.get(&principal) else {
            return out; // 该主体无任何授权格
        };
        for (resource, capability) in cells.keys() {
            let caps = out.entry(resource.clone()).or_default();
            if !caps.contains(capability) {
                caps.push(*capability);
            }
        }
        for caps in out.values_mut() {
            caps.sort();
        }
        out
    }
}

/// `principals` 行 → [`PrincipalRow`] 映射（fail-closed：任一列取值失败 → `Err`）。
fn map_principal(r: &rusqlite::Row<'_>) -> Result<PrincipalRow, StoreError> {
    Ok(PrincipalRow {
        id: SnowflakeId::from_raw(r.get::<_, i64>(0).map_err(|_| StoreError::Io)? as u64),
        version: r.get(1).map_err(|_| StoreError::Io)?,
        name: r.get(2).map_err(|_| StoreError::Io)?,
        kind: r.get(3).map_err(|_| StoreError::Io)?,
    })
}

/// 带单个过滤值（`?3`）的分页执行器：与 [`execute_page`] 同形，但 `list_sql` 多绑一个
/// 过滤参数（`?3`），`count_sql` 用 `?1` 绑同一过滤值。默认作用域 + `LIMIT` 由调用方
/// 在 SQL 文本里携带（`delete_flag = 0` / `LIMIT ?1 OFFSET ?2`）。
fn execute_page_filtered<T, M>(
    db: &Db,
    list_sql: &str,
    count_sql: &str,
    filter: i64,
    page: PageQuery,
    map_row: M,
) -> Result<Page<T>, StoreError>
where
    M: Fn(&rusqlite::Row<'_>) -> Result<T, StoreError>,
{
    let clamped = page.clamp();
    let limit = i64::from(clamped.page_size);
    let offset = i64::try_from((u64::from(clamped.page_no) - 1) * u64::from(clamped.page_size))
        .map_err(|_| StoreError::Io)?;

    db.with_read(|conn| {
        let total: i64 = conn
            .query_row(count_sql, [filter], |r| r.get(0))
            .map_err(|_| StoreError::Io)?;

        let mut stmt = conn.prepare(list_sql).map_err(|_| StoreError::Io)?;
        let mut rows = stmt
            .query(rusqlite::params![limit, offset, filter])
            .map_err(|_| StoreError::Io)?;

        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|_| StoreError::Io)? {
            items.push(map_row(row)?);
        }

        Ok(Page {
            items,
            page_no: clamped.page_no,
            page_size: clamped.page_size,
            total: total.max(0) as u64,
        })
    })
}
