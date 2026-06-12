//! 结构化拒绝响应（DenyResponse）的事实组装：纯函数，只搬运快照事实或人
//! 预写内容（公理六），绝不臆造、绝不泄露目标资源是否存在。
//!
//! 拒绝响应类型 `DenyResponse` / `DeniedFacts` 的**所有权**在 `decision`
//! 单元（已完成、冻结），本模块不重定义——只在求值管线命名空间内**再导出**
//! 这些权威类型，并提供把 `(PolicySnapshot, principal, resource, capability,
//! objects, reason 事实)` 机械组装成 `DenyResponse` 的纯函数 `assemble`。
//!
//! 字段取值规约（模块设计 §3.3 DenyResponse 组装 / §5.1 / §8 L-5/L-6；
//! 详细设计 8.3 范围内·DenyResponse 事实组装、必守不变量
//! `DENY_RESPONSE_SCOPE_BOUNDED`）：
//!
//!   - `decision`：恒为 `"deny"`。
//!   - `denied`：装 `DeniedFacts { resource, capability, objects }`，全部取自
//!     入参轨迹事实（代号类型，绝不触机密族）。
//!   - `reason`：引用策略事实（拒绝阶段相关、role/谓词 kind 等），机械取入参
//!     `reason` 文本，不编造话术、不泄露目标存在性。
//!   - `your_grants`：只导出 `snapshot.grants[principal]` 内**该 Principal 自身**
//!     授权资源代号 → 能力名列表（用 `Capability::as_str`），**绝不**查目标
//!     资源是否存在、绝不枚举他人/全局资源；`grants` 缺 principal → 空
//!     `BTreeMap`（fail-closed：空集合，不放行、不报错）。`BTreeMap` 保证序
//!     确定。受 `DENY_RESPONSE_SCOPE_BOUNDED` 约束，这使"Scope 外但存在的
//!     资源"与"根本不存在的资源"两次拒绝逐字节相同、不可区分（防拓扑探测）。
//!   - `request_hint`：目标动词在 `snapshot.grantable[resource]` 中 → 机械生成
//!     `postern elevate` 命令字符串 `Some(..)`；不在其中（含 resource 缺失）→
//!     `None`（序列化为 `null`）。
//!   - `operator_note`：`snapshot.deny_notes[(resource, capability)]` 存在 →
//!     取出原样为 `Some`；缺省 → `None` 且经 serde_json 序列化时该字段不出现
//!     （`DenyResponse` 已配 `skip_serializing_if`）。
//!
//! 查表缺失一律以 `match` / `Option` 组合处理，缺格 → 空集合 / `None` 而非
//! 吞错放行（求值路径 fail-closed，契约 `EVAL_NO_ERROR_SWALLOWING`）。逻辑
//! 待实现。

use std::collections::BTreeMap;

use crate::domain::{Capability, PolicySnapshot, PrincipalId, ResourceCode};
use crate::request::ObjectRef;

pub use crate::decision::{DeniedFacts, DenyResponse};

/// 据快照与轨迹事实机械组装结构化拒绝响应（纯函数，公理六）。
///
/// 入参：单一权威策略快照 `snapshot`、已认证（或拒绝阶段已知）的 `principal`、
/// 目标资源代号 `resource`、归类动词 `capability`、归类对象 `objects`、以及由
/// 轨迹截止步导出的策略事实文本 `reason`。
///
/// 出参：`DenyResponse`，其 `decision` 恒 `"deny"`，`your_grants` 仅含
/// `principal` 自身授权世界（Scope 受限），`request_hint`/`operator_note` 分别由
/// `snapshot.grantable` / `snapshot.deny_notes` 机械导出。无任何 IO、无机密。
///
/// 骨架占位：逻辑待实现。
pub fn assemble(
    snapshot: &PolicySnapshot,
    principal: &PrincipalId,
    resource: &ResourceCode,
    capability: Capability,
    objects: &[ObjectRef],
    reason: String,
) -> DenyResponse {
    // your_grants：仅从 principal 自身授权世界导出，绝不查目标资源是否存在、
    // 绝不枚举他人/全局资源。缺 principal → 空 BTreeMap（fail-closed：空集合，
    // 不放行、不报错）。BTreeMap 迭代序保证导出确定，且使"Scope 外但存在的
    // 资源"与"根本不存在的资源"两次拒绝逐字节相同（DENY_RESPONSE_SCOPE_BOUNDED）。
    let your_grants: BTreeMap<ResourceCode, Vec<String>> = match snapshot.grants.get(principal) {
        Some(per_principal) => {
            let mut exported: BTreeMap<ResourceCode, Vec<String>> = BTreeMap::new();
            for (res, cap) in per_principal.keys() {
                exported
                    .entry(res.clone())
                    .or_default()
                    .push(cap.as_str().to_string());
            }
            exported
        }
        None => BTreeMap::new(),
    };

    // request_hint：目标动词在 snapshot.grantable[resource] 中 → 机械生成 postern
    // elevate 命令；resource 缺格或动词不在其中 → None（不吞错，缺格即 None）。
    let request_hint: Option<String> = match snapshot.grantable.get(resource) {
        Some(verbs) if verbs.contains(&capability) => Some(format!(
            "postern elevate {} {}",
            resource.as_str(),
            capability.as_str()
        )),
        Some(_) => None,
        None => None,
    };

    // operator_note：snapshot.deny_notes[(resource, capability)] 存在 → 原样转述
    // Some；缺省 → None（DenyResponse 已配 skip_serializing_if，不序列化）。
    let operator_note: Option<String> = snapshot
        .deny_notes
        .get(&(resource.clone(), capability))
        .cloned();

    DenyResponse {
        decision: "deny",
        denied: DeniedFacts {
            resource: resource.clone(),
            capability,
            objects: objects.to_vec(),
        },
        reason,
        your_grants,
        request_hint,
        operator_note,
    }
}
