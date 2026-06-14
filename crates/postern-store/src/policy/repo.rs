//! PolicyRepo：策略状态事务读写句柄，一律经 base 仓储（唯一写路径）。
//!
//! 本文件**绝不**直接出现数据库写语句：每个写方法都在一次写事务内委托
//! [`base::write`](crate::base::write) 的 API（写改逻辑删除级联只在
//! `src/base/`，契约 `DB_WRITE_PATH_CENTRALIZED`）。读端点经
//! [`base::scope`](crate::base::scope) 的分页执行器（默认作用域 `delete_flag = 0`
//! + `LIMIT`），返回携 `version` 的读模型行。

use std::sync::Arc;

use crate::base::db::Db;
use crate::base::error::StoreError;
use crate::base::scope::execute_page;
use crate::base::write::{self, Actor, InsertRow};
use crate::snapshot::{build_snapshot_on, SnapshotView};
use postern_core::domain::Timestamp;
use postern_core::id::{Clock, IdGen, SnowflakeId};
use postern_core::page::{Page, PageQuery};
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
