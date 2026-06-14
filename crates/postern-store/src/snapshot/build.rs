//! 快照构建：一次事务全量加载、角色继承展开、授权空间物化、fail-closed 兜底（§3.4）。
//!
//! [`build_snapshot`] 在一次只读事务内把权威库投影为 [`PolicySnapshot`]：
//!
//! - **全量加载**按表语义分两类——授予性表 `delete_flag = 0 AND enable_flag = 1`、
//!   限制性表仅 `delete_flag = 0`（绝不引入 `enable_flag` 过滤，§7-11）。集合加载经
//!   `LIMIT + 游标分页`分块读完整表（`DB_PAGINATION_MANDATORY`：快照全量加载也分页）。
//! - **角色继承展开**：沿 `role_inherits` 求传递闭包，有限深度 + 已访问集去重防环。
//! - **授权空间物化**：`bindings × role_capabilities`（含继承）合 `binding_scope` 辖区
//!   （`selector` 此刻按 `resource_labels` 展开为具体资源集）∪ 有效 `temp_grants`，
//!   逐格落 [`GrantCell`]（挂 `grant_constraints` / `grant_conditions`）。
//! - **fail-closed**：加载 / 解析失败 ⇒ 该格不可见 / 拒绝（绝不放行悬挂引用、绝不取最宽松）。

use std::collections::{BTreeMap, BTreeSet};

use crate::base::db::{Db, ReadConn};
use crate::base::error::StoreError;
use postern_core::domain::{
    Capability, ConditionSpec, ConstraintSpec, CredentialMeta, CredentialTier, CredentialView,
    GrantAction, GrantCell, Mode, PolicySnapshot, PrincipalId, ResourceCode, Role, TierDecl,
};
use postern_core::id::SnowflakeId;

/// 全量加载的游标分页块大小（`LIMIT` 上限）。整表经 `LIMIT + OFFSET` 分块读完，
/// 每张表绝不写无界全表 SELECT（`DB_PAGINATION_MANDATORY`）。
const LOAD_PAGE: i64 = 1000;

// ============================================================ 行模型（库 → 内存）

/// 一条角色→动词声明（授予性表，物化授权格的来源）。
struct RoleCapRow {
    role_id: i64,
    capability: String,
    action: String,
}

/// 一条角色继承边（子角色 → 父角色）。
struct InheritRow {
    role_id: i64,
    parent_role_id: i64,
}

/// 一条主体↔角色绑定（授予性表）。
struct BindingRow {
    id: i64,
    principal_id: i64,
    role_id: i64,
}

/// 一条绑定辖区（resource 直挂 / selector 标签选择器）。
struct ScopeRow {
    binding_id: i64,
    kind: String,
    resource_id: Option<i64>,
    selector: Option<String>,
}

/// 一条资源标签（供 selector 按标签展开）。
struct LabelRow {
    resource_id: i64,
    key: String,
    value: String,
}

/// 一条有效临时授权（授予性表；TTL 不在本域裁决，原值投影）。
struct TempGrantRow {
    principal_id: i64,
    resource_id: i64,
    capability: String,
}

/// 一条对象细则（限制性表；挂到匹配 (resource, capability) 的授权格上）。
struct ConstraintRow {
    resource_id: i64,
    capability: String,
    kind: String,
    spec: Option<String>,
}

/// 一条求值条件（限制性表；capability 可空——空表示该资源全动词通用条件）。
struct ConditionRow {
    resource_id: Option<i64>,
    capability: Option<String>,
    predicate: String,
    spec: Option<String>,
}

/// 一条辖区运行模式（限制性表；`scope_resource_id` 为空 = 全局模式，
/// 非空 = 该资源的模式覆盖）。失控切断的安全特性。
struct ModeRow {
    scope_resource_id: Option<i64>,
    mode: String,
}

// ============================================================ 入口

/// 在一次只读事务内把权威库投影为不可变 [`PolicySnapshot`]（`policy_rev` 为本次
/// 重建的策略修订号，每次重建递增——审计对账锚点）。
///
/// 授予性表 `delete_flag = 0 AND enable_flag = 1`、限制性表仅 `delete_flag = 0`
/// 加载；角色继承展开、授权空间物化、约束/条件挂载，全部在本调用内完成。
/// 任一加载/解析失败 ⇒ fail-closed（该格不可见 / 整体拒绝），绝不产半截放行的快照。
pub fn build_snapshot(db: &Db, policy_rev: u64) -> Result<PolicySnapshot, StoreError> {
    db.with_read(|conn| build_snapshot_on(conn, policy_rev))
}

/// 在调用方已持有的只读连接上把权威库投影为不可变 [`PolicySnapshot`]，语义同
/// [`build_snapshot`]。供"提交+重建"编排在**同一写锁临界区**内、事务 COMMIT 后于
/// 持有的连接上重建（避免对非重入互斥锁二次取锁——见
/// [`Db::commit_and_rebuild`](crate::base::db::Db::commit_and_rebuild)）。
pub fn build_snapshot_on(
    conn: &ReadConn<'_>,
    policy_rev: u64,
) -> Result<PolicySnapshot, StoreError> {
    // ---- 授予性表加载（delete_flag = 0 AND enable_flag = 1）。
    let resources = load_resources(conn)?; // id -> ResourceCode（活跃+启用，悬挂引用判定的真集）
    let roles = load_roles(conn)?; // id -> Role 名（活跃+启用）
    let role_caps = load_role_capabilities(conn)?;
    let inherits = load_role_inherits(conn)?;
    let bindings = load_bindings(conn)?;
    let scopes = load_binding_scope(conn)?;
    let labels = load_resource_labels(conn)?;
    let temp_grants = load_temp_grants(conn)?;
    let tiers = load_tiers(conn, &resources)?;
    let credentials = load_credentials(conn)?;

    // ---- 限制性表加载（仅 delete_flag = 0，绝不引入 enable_flag 过滤）。
    let deny_notes = load_deny_notes(conn, &resources)?;
    let constraints = load_constraints(conn)?;
    let conditions = load_conditions(conn)?;
    let mode_rows = load_mode_state(conn)?;

    // ---- 角色 → 有效动词（含继承传递闭包；返回 capability -> 声明它的 role_id）。
    let role_effective = expand_role_capabilities(&role_caps, &inherits);

    // ---- 物化授权空间：bindings × 有效动词，沿 binding_scope 辖区落格。
    let mut grants: BTreeMap<PrincipalId, BTreeMap<(ResourceCode, Capability), GrantCell>> =
        BTreeMap::new();

    // 绑定的辖区索引：binding_id -> 该绑定覆盖的资源集合（已按当前标签展开、已去悬挂）。
    let scope_index = index_scopes(&scopes, &resources, &labels);

    for b in &bindings {
        let principal = PrincipalId::new(SnowflakeId::from_raw(b.principal_id as u64));
        let Some(effective) = role_effective.get(&b.role_id) else {
            continue; // 该绑定的角色无任何有效动词（或角色不可见）→ 不授予
        };
        let Some(scope_resources) = scope_index.get(&b.id) else {
            continue; // 空辖区（无 resource / selector 展开为空集）→ fail-closed 不授予
        };
        for res_code in scope_resources {
            for (cap, role_id) in effective {
                let Some(action) =
                    action_of(&role_effective_action(&role_caps, &inherits, *role_id, cap))
                else {
                    continue;
                };
                let Some(role_name) = roles.get(role_id) else {
                    continue; // 声明该动词的角色不可见（悬挂）→ 不放行
                };
                let cell = GrantCell {
                    resource: res_code.clone(),
                    capability: *cap,
                    role: Role::new(role_name.clone()),
                    action,
                    constraints: collect_constraints(&constraints, res_code, *cap, &resources),
                    conditions: collect_conditions(&conditions, res_code, *cap, &resources),
                };
                grants
                    .entry(principal)
                    .or_default()
                    .insert((res_code.clone(), *cap), cell);
            }
        }
    }

    // ---- 有效临时授权 ∪ 进授权空间（直授，无需 binding/role）。
    for tg in &temp_grants {
        let Some(res_code) = resources.get(&tg.resource_id) else {
            continue; // 指向不可见资源（悬挂引用）→ fail-closed 不放行
        };
        let Some(cap) = parse_capability(&tg.capability) else {
            continue; // 无法解析的动词 → 不放行
        };
        let principal = PrincipalId::new(SnowflakeId::from_raw(tg.principal_id as u64));
        let cell = GrantCell {
            resource: res_code.clone(),
            capability: cap,
            role: Role::new("temp"),
            action: GrantAction::Allow,
            constraints: collect_constraints(&constraints, res_code, cap, &resources),
            conditions: collect_conditions(&conditions, res_code, cap, &resources),
        };
        grants
            .entry(principal)
            .or_default()
            .entry((res_code.clone(), cap))
            .or_insert(cell);
    }

    // ---- grantable：从物化授权格反推（每资源可授动词集，request_hint 的机械来源）。
    let mut grantable: BTreeMap<ResourceCode, Vec<Capability>> = BTreeMap::new();
    for cells in grants.values() {
        for (res, cap) in cells.keys() {
            let set = grantable.entry(res.clone()).or_default();
            if !set.contains(cap) {
                set.push(*cap);
            }
        }
    }
    for caps in grantable.values_mut() {
        caps.sort();
    }

    // ---- 辖区运行模式物化（失控切断）：每行按 scope_resource_id 映射到
    //      modes[None]（全局）或 modes[Some(resource)]（资源级）。store 只如实存
    //      各辖区模式（meet 计算在 evaluator/core）；唯一索引保证每辖区至多一行，
    //      但同辖区若出现多行（不该有），保守取最严（Mode::meet）兜底。指向不可见
    //      资源（悬挂引用）或无法解析的模式文本 → 跳过（fail-closed，不投影）。
    let mut modes: BTreeMap<Option<ResourceCode>, Mode> = BTreeMap::new();
    for row in &mode_rows {
        let Some(mode) = parse_mode(&row.mode) else {
            continue; // 未知模式文本 → 不投影
        };
        let scope: Option<ResourceCode> = match row.scope_resource_id {
            None => None, // 全局
            Some(rid) => match resources.get(&rid) {
                Some(code) => Some(code.clone()),
                None => continue, // 指向不可见资源 → 悬挂引用，跳过
            },
        };
        modes
            .entry(scope)
            .and_modify(|existing| *existing = existing.meet(mode))
            .or_insert(mode);
    }

    Ok(PolicySnapshot {
        policy_rev,
        grants,
        tiers,
        credentials,
        deny_notes,
        grantable,
        modes,
    })
}

// ============================================================ 分块全量加载器

/// 通用游标分页加载：以调用方给定的"带 `delete_flag = 0` 谓词且以 `LIMIT ?1 OFFSET ?2`
/// 收尾"的 SELECT，分块读完整表（每块 [`LOAD_PAGE`] 行），逐行经 `map` 映射收集。
///
/// 集合加载恒经 `LIMIT + 游标`分块（绝不无界全表 SELECT，`DB_PAGINATION_MANDATORY`）；
/// 任一行映射失败 ⇒ fail-closed `Err`（不产半截结果）。
fn load_paged<T, M>(conn: &ReadConn<'_>, select: &str, map: M) -> Result<Vec<T>, StoreError>
where
    M: Fn(&rusqlite::Row<'_>) -> Result<T, StoreError>,
{
    let mut out = Vec::new();
    let mut offset: i64 = 0;
    loop {
        let mut stmt = conn.prepare(select).map_err(|_| StoreError::Io)?;
        let mut rows = stmt
            .query(rusqlite::params![LOAD_PAGE, offset])
            .map_err(|_| StoreError::Io)?;
        let mut n: i64 = 0;
        while let Some(row) = rows.next().map_err(|_| StoreError::Io)? {
            out.push(map(row)?);
            n += 1;
        }
        if n < LOAD_PAGE {
            break;
        }
        offset += LOAD_PAGE;
    }
    Ok(out)
}

fn get_i64(row: &rusqlite::Row<'_>, idx: usize) -> Result<i64, StoreError> {
    row.get(idx).map_err(|_| StoreError::Io)
}

fn get_text(row: &rusqlite::Row<'_>, idx: usize) -> Result<String, StoreError> {
    row.get(idx).map_err(|_| StoreError::Io)
}

fn get_opt_i64(row: &rusqlite::Row<'_>, idx: usize) -> Result<Option<i64>, StoreError> {
    row.get(idx).map_err(|_| StoreError::Io)
}

fn get_opt_text(row: &rusqlite::Row<'_>, idx: usize) -> Result<Option<String>, StoreError> {
    row.get(idx).map_err(|_| StoreError::Io)
}

// ============================================================ 各表加载（self-contained SELECT）

fn load_resources(conn: &ReadConn<'_>) -> Result<BTreeMap<i64, ResourceCode>, StoreError> {
    let select = "SELECT id, codename FROM resources \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    let rows = load_paged(conn, select, |r| Ok((get_i64(r, 0)?, get_text(r, 1)?)))?;
    Ok(rows
        .into_iter()
        .map(|(id, code)| (id, ResourceCode::new(code)))
        .collect())
}

fn load_roles(conn: &ReadConn<'_>) -> Result<BTreeMap<i64, String>, StoreError> {
    let select = "SELECT id, name FROM roles \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    let rows = load_paged(conn, select, |r| Ok((get_i64(r, 0)?, get_text(r, 1)?)))?;
    Ok(rows.into_iter().collect())
}

fn load_role_capabilities(conn: &ReadConn<'_>) -> Result<Vec<RoleCapRow>, StoreError> {
    let select = "SELECT role_id, capability, action FROM role_capabilities \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(RoleCapRow {
            role_id: get_i64(r, 0)?,
            capability: get_text(r, 1)?,
            action: get_text(r, 2)?,
        })
    })
}

fn load_role_inherits(conn: &ReadConn<'_>) -> Result<Vec<InheritRow>, StoreError> {
    let select = "SELECT role_id, parent_role_id FROM role_inherits \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(InheritRow {
            role_id: get_i64(r, 0)?,
            parent_role_id: get_i64(r, 1)?,
        })
    })
}

fn load_bindings(conn: &ReadConn<'_>) -> Result<Vec<BindingRow>, StoreError> {
    let select = "SELECT id, principal_id, role_id FROM bindings \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(BindingRow {
            id: get_i64(r, 0)?,
            principal_id: get_i64(r, 1)?,
            role_id: get_i64(r, 2)?,
        })
    })
}

fn load_binding_scope(conn: &ReadConn<'_>) -> Result<Vec<ScopeRow>, StoreError> {
    let select = "SELECT binding_id, kind, resource_id, selector FROM binding_scope \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(ScopeRow {
            binding_id: get_i64(r, 0)?,
            kind: get_text(r, 1)?,
            resource_id: get_opt_i64(r, 2)?,
            selector: get_opt_text(r, 3)?,
        })
    })
}

fn load_resource_labels(conn: &ReadConn<'_>) -> Result<Vec<LabelRow>, StoreError> {
    let select = "SELECT resource_id, key, value FROM resource_labels \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(LabelRow {
            resource_id: get_i64(r, 0)?,
            key: get_text(r, 1)?,
            value: get_text(r, 2)?,
        })
    })
}

fn load_temp_grants(conn: &ReadConn<'_>) -> Result<Vec<TempGrantRow>, StoreError> {
    let select = "SELECT principal_id, resource_id, capability FROM temp_grants \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(TempGrantRow {
            principal_id: get_i64(r, 0)?,
            resource_id: get_i64(r, 1)?,
            capability: get_text(r, 2)?,
        })
    })
}

fn load_tiers(
    conn: &ReadConn<'_>,
    resources: &BTreeMap<i64, ResourceCode>,
) -> Result<BTreeMap<ResourceCode, Vec<TierDecl>>, StoreError> {
    let select = "SELECT resource_id, tier, capabilities FROM resource_credential_tiers \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    let rows = load_paged(conn, select, |r| {
        Ok((get_i64(r, 0)?, get_text(r, 1)?, get_opt_text(r, 2)?))
    })?;
    let mut out: BTreeMap<ResourceCode, Vec<TierDecl>> = BTreeMap::new();
    for (rid, tier, caps) in rows {
        let Some(res_code) = resources.get(&rid) else {
            continue; // tier 挂在不可见资源上（悬挂引用）→ 不投影
        };
        let carries = caps
            .as_deref()
            .map(parse_capability_list)
            .unwrap_or_default();
        out.entry(res_code.clone()).or_default().push(TierDecl {
            tier: CredentialTier::new(tier),
            carries,
        });
    }
    Ok(out)
}

fn load_credentials(conn: &ReadConn<'_>) -> Result<CredentialView, StoreError> {
    let select = "SELECT principal_id, kind, secret_hash FROM credentials \
                  WHERE delete_flag = 0 AND enable_flag = 1 ORDER BY id LIMIT ?1 OFFSET ?2";
    let rows = load_paged(conn, select, |r| {
        Ok((get_i64(r, 0)?, get_text(r, 1)?, get_opt_text(r, 2)?))
    })?;
    let credentials = rows
        .into_iter()
        .map(|(pid, kind, secret_hash)| CredentialMeta {
            principal: PrincipalId::new(SnowflakeId::from_raw(pid as u64)),
            kind,
            secret_hash: secret_hash.unwrap_or_default(),
            expires_at: None,
            revoked_at: None,
        })
        .collect();
    Ok(CredentialView { credentials })
}

fn load_deny_notes(
    conn: &ReadConn<'_>,
    resources: &BTreeMap<i64, ResourceCode>,
) -> Result<BTreeMap<(ResourceCode, Capability), String>, StoreError> {
    // 限制性表：仅 delete_flag = 0（绝不 enable_flag 过滤，否则解约 fail-open）。
    let select = "SELECT resource_id, capability, note FROM deny_notes \
                  WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
    let rows = load_paged(conn, select, |r| {
        Ok((get_i64(r, 0)?, get_text(r, 1)?, get_text(r, 2)?))
    })?;
    let mut out = BTreeMap::new();
    for (rid, cap, note) in rows {
        let Some(res_code) = resources.get(&rid) else {
            continue; // 挂在不可见资源上 → 不投影
        };
        let Some(capability) = parse_capability(&cap) else {
            continue;
        };
        out.insert((res_code.clone(), capability), note);
    }
    Ok(out)
}

fn load_constraints(conn: &ReadConn<'_>) -> Result<Vec<ConstraintRow>, StoreError> {
    // 限制性表：仅 delete_flag = 0。
    let select = "SELECT resource_id, capability, kind, spec FROM grant_constraints \
                  WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(ConstraintRow {
            resource_id: get_i64(r, 0)?,
            capability: get_text(r, 1)?,
            kind: get_text(r, 2)?,
            spec: get_opt_text(r, 3)?,
        })
    })
}

fn load_conditions(conn: &ReadConn<'_>) -> Result<Vec<ConditionRow>, StoreError> {
    // 限制性表：仅 delete_flag = 0。capability 可空（资源级通用条件）。
    let select = "SELECT resource_id, capability, predicate, spec FROM grant_conditions \
                  WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(ConditionRow {
            resource_id: get_opt_i64(r, 0)?,
            capability: get_opt_text(r, 1)?,
            predicate: get_text(r, 2)?,
            spec: get_opt_text(r, 3)?,
        })
    })
}

fn load_mode_state(conn: &ReadConn<'_>) -> Result<Vec<ModeRow>, StoreError> {
    // 限制性表：仅 delete_flag = 0（绝不引入 enable_flag 过滤，否则解冻 fail-open）。
    // scope_resource_id 可空（NULL = 全局模式）。
    let select = "SELECT scope_resource_id, mode FROM mode_state \
                  WHERE delete_flag = 0 ORDER BY id LIMIT ?1 OFFSET ?2";
    load_paged(conn, select, |r| {
        Ok(ModeRow {
            scope_resource_id: get_opt_i64(r, 0)?,
            mode: get_text(r, 1)?,
        })
    })
}

// ============================================================ 角色继承展开

/// 沿 `role_inherits` 求每个角色的有效动词传递闭包（含自身 + 全部祖先的动词）。
/// 返回 `role_id -> (capability -> 声明该动词的 role_id)`；遇环以已访问集去重兜底
/// （不死循、不放大授权）。同动词由多角色声明时，取**最近**（自身优先于祖先）。
fn expand_role_capabilities(
    role_caps: &[RoleCapRow],
    inherits: &[InheritRow],
) -> BTreeMap<i64, BTreeMap<Capability, i64>> {
    // role_id -> 直接父集合
    let mut parents: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for e in inherits {
        parents.entry(e.role_id).or_default().push(e.parent_role_id);
    }
    // role_id -> 直接声明的动词
    let mut direct: BTreeMap<i64, Vec<Capability>> = BTreeMap::new();
    for rc in role_caps {
        if let Some(cap) = parse_capability(&rc.capability) {
            direct.entry(rc.role_id).or_default().push(cap);
        }
    }

    let all_roles: BTreeSet<i64> = parents
        .keys()
        .copied()
        .chain(parents.values().flatten().copied())
        .chain(direct.keys().copied())
        .collect();

    let mut out: BTreeMap<i64, BTreeMap<Capability, i64>> = BTreeMap::new();
    for root in all_roles {
        let mut effective: BTreeMap<Capability, i64> = BTreeMap::new();
        let mut visited: BTreeSet<i64> = BTreeSet::new();
        // BFS：root 先于祖先，故 root 自身声明优先（entry().or_insert 不覆盖）。
        let mut queue: Vec<i64> = vec![root];
        while let Some(role_id) = queue.pop() {
            if !visited.insert(role_id) {
                continue; // 已访问 → 防环兜底
            }
            if let Some(caps) = direct.get(&role_id) {
                for cap in caps {
                    effective.entry(*cap).or_insert(role_id);
                }
            }
            if let Some(ps) = parents.get(&role_id) {
                for p in ps {
                    if !visited.contains(p) {
                        queue.push(*p);
                    }
                }
            }
        }
        out.insert(root, effective);
    }
    out
}

/// 取某角色（沿继承）对某动词的 `action` 文本——以声明该动词的角色为准。
/// `declaring_role_id` 由 [`expand_role_capabilities`] 给出。
fn role_effective_action(
    role_caps: &[RoleCapRow],
    _inherits: &[InheritRow],
    declaring_role_id: i64,
    cap: &Capability,
) -> String {
    for rc in role_caps {
        if rc.role_id == declaring_role_id && parse_capability(&rc.capability) == Some(*cap) {
            return rc.action.clone();
        }
    }
    String::new()
}

// ============================================================ 辖区展开

/// 把每个 binding 的辖区行展开为它覆盖的具体资源集（resource 直挂 + selector 按当前
/// 标签展开），去除指向不可见资源的悬挂引用。空集（无 resource / selector 无匹配）的
/// 绑定不出现在索引里——其调用方据此 fail-closed 不授予。
fn index_scopes(
    scopes: &[ScopeRow],
    resources: &BTreeMap<i64, ResourceCode>,
    labels: &[LabelRow],
) -> BTreeMap<i64, BTreeSet<ResourceCode>> {
    let mut out: BTreeMap<i64, BTreeSet<ResourceCode>> = BTreeMap::new();
    for s in scopes {
        let mut matched: Vec<ResourceCode> = Vec::new();
        match s.kind.as_str() {
            "resource" => {
                if let Some(rid) = s.resource_id {
                    if let Some(code) = resources.get(&rid) {
                        matched.push(code.clone()); // 指向不可见资源 → 跳过（悬挂）
                    }
                }
            }
            "selector" => {
                if let Some(sel) = s.selector.as_deref() {
                    matched.extend(expand_selector(sel, resources, labels));
                }
            }
            _ => {}
        }
        if !matched.is_empty() {
            out.entry(s.binding_id).or_default().extend(matched);
        }
    }
    out
}

/// 把 `key=value` 标签选择器展开为带该标签的活跃资源集（fail-closed：无法解析或无匹配
/// ⇒ 空集，绝不放行）。当前仅支持单条 `key=value` 形式（5.2bis-②）。
fn expand_selector(
    selector: &str,
    resources: &BTreeMap<i64, ResourceCode>,
    labels: &[LabelRow],
) -> Vec<ResourceCode> {
    let Some((key, value)) = selector.split_once('=') else {
        return Vec::new(); // 无法解析 → 空集
    };
    let key = key.trim();
    let value = value.trim();
    let mut out = Vec::new();
    for l in labels {
        if l.key == key && l.value == value {
            if let Some(code) = resources.get(&l.resource_id) {
                if !out.contains(code) {
                    out.push(code.clone());
                }
            }
        }
    }
    out
}

// ============================================================ 约束 / 条件挂载

fn collect_constraints(
    constraints: &[ConstraintRow],
    resource: &ResourceCode,
    cap: Capability,
    resources: &BTreeMap<i64, ResourceCode>,
) -> Vec<ConstraintSpec> {
    constraints
        .iter()
        .filter(|c| {
            resources.get(&c.resource_id) == Some(resource)
                && parse_capability(&c.capability) == Some(cap)
        })
        .map(|c| ConstraintSpec {
            kind: c.kind.clone(),
            spec: c.spec.clone().unwrap_or_default(),
        })
        .collect()
}

fn collect_conditions(
    conditions: &[ConditionRow],
    resource: &ResourceCode,
    cap: Capability,
    resources: &BTreeMap<i64, ResourceCode>,
) -> Vec<ConditionSpec> {
    conditions
        .iter()
        .filter(|c| {
            // 资源匹配（NULL = 全局通用条件）。
            let res_ok = match c.resource_id {
                Some(rid) => resources.get(&rid) == Some(resource),
                None => true,
            };
            // capability 匹配（NULL = 资源全动词通用）。
            let cap_ok = match c.capability.as_deref() {
                Some(text) => parse_capability(text) == Some(cap),
                None => true,
            };
            res_ok && cap_ok
        })
        .map(|c| ConditionSpec {
            kind: c.predicate.clone(),
            spec: c.spec.clone().unwrap_or_default(),
        })
        .collect()
}

// ============================================================ 解析辅助

/// 6 动词文本 → [`Capability`]，未知动词 → `None`（fail-closed，不放行）。
fn parse_capability(text: &str) -> Option<Capability> {
    match text {
        "observe" => Some(Capability::Observe),
        "query" => Some(Capability::Query),
        "mutate" => Some(Capability::Mutate),
        "execute" => Some(Capability::Execute),
        "manage" => Some(Capability::Manage),
        "destroy" => Some(Capability::Destroy),
        _ => None,
    }
}

/// 逗号分隔动词列表 → `Vec<Capability>`（跳过无法解析项）。
fn parse_capability_list(text: &str) -> Vec<Capability> {
    text.split(',')
        .filter_map(|t| parse_capability(t.trim()))
        .collect()
}

/// `action` 文本 → [`GrantAction`]，未知 → `None`（fail-closed，不落格）。
fn action_of(text: &str) -> Option<GrantAction> {
    match text {
        "allow" => Some(GrantAction::Allow),
        "escalate" => Some(GrantAction::Escalate),
        _ => None,
    }
}

/// 4 模式文本 → [`Mode`]，未知 → `None`（fail-closed，不投影该辖区模式）。
fn parse_mode(text: &str) -> Option<Mode> {
    match text {
        "normal" => Some(Mode::Normal),
        "observe" => Some(Mode::Observe),
        "maintain" => Some(Mode::Maintain),
        "freeze" => Some(Mode::Freeze),
        _ => None,
    }
}
