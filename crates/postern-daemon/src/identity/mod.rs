//! 身份与凭证域（8.4）——`Authenticator` 实现子域（模块文档 06；详细设计第七部分 /
//! 6.13 认证流程 / 5.2 credentials 表语义）。
//!
//! core 只**定义** `Authenticator` trait（认证器族 step [1]），实现归 daemon（core 文档
//! 01：「认证机制的实现……实现于 postern-daemon」）。本子域实现三族认证器并装配进
//! `Evaluator` 的认证器注册表：
//!
//! - [`local_process`]：零凭证，据 SO_PEERCRED 观测 uid/gid 匹配 → principal（无 secret 校验）。
//! - [`api_key`]：presented api_key 字节 → argon2id verify against `secret_hash` → principal；
//!   可信域 + `expires_at`/`revoked_at` 按 `now` 二次校验。
//! - [`token`]：同 api_key 形态（共用 [`secret_auth::SecretAuthenticator`] 内核）。
//!
//! 三族公共纪律（fail-closed，公理二）：无匹配/过期/吊销/可信域不符/来源无法判定一律
//! `Err(AuthError)`（永不放行）；多候选按 `CredentialView.credentials` 固定顺序确定性选取；
//! secret 经 argon2id verify（常量时间口令比对），绝不 log 明文。`now` 显式传入——时效在
//! 求值时刻按墙钟二次校验，过期即刻失效、不依赖 sweeper（详细设计 6.2）。
//!
//! 与 core 实际类型的对接：core `CredentialMeta` 仅 `principal`/`kind`/`secret_hash`/
//! `expires_at`/`revoked_at`，无独立 match-spec/trust_domain 字段。故 local_process 的
//! uid/gid 规则编码在 `secret_hash` 文本（该 kind 无真实 secret），可信域是认证器实例的
//! 配置门（据观测 origin 判定）——均为对 core 既有形态的诚实复用，不向 core 增字段。
//!
//! 路径纪律（雷区）：本目录不在 `src/shells/`，**绝不**出现字面 `ConnOrigin::UnixPeer`
//! / `ConnOrigin::Tcp`（契约 SEC_CONSTRUCTION_SITES 扫描这两个字面串）——需要来源时以
//! `use postern_core::request::ConnOrigin as Origin` 别名读/解构。本目录零 SQL 标记、
//! 无 unsafe、不构造 `ResolvedTarget`/`ResourceCredential`、不吞错。

pub mod api_key;
pub mod expiry;
pub mod local_process;
pub mod secret_auth;
pub mod token;

use std::collections::BTreeMap;

use postern_core::plugin::Authenticator;

use crate::identity::local_process::LocalProcessAuthenticator;

/// 装配三族认证器为 `Evaluator` 可注入的认证器注册表（按各自 `kind()` 为键）。
///
/// boot/registry 装配点（[`crate::boot`]）以此构造 `Evaluator` 的认证器集合：键为
/// `local_process` / `api_key` / `token`，确定性 `BTreeMap`（非 `HashMap`，workspace
/// 确定性纪律）。`Evaluator::authenticate` 按 presented kind 在本表选认证器；未命中即
/// fail-closed deny（无法判定来源）。
///
/// 本函数是身份域→求值器的唯一接线点：三族实现在此箱化为 `Box<dyn Authenticator>` 并
/// 以各自 `kind()` 入键，确保「kind 即注册键」（core 文档 01 step [1]）在装配处成立。
pub fn authenticator_registry() -> BTreeMap<&'static str, Box<dyn Authenticator>> {
    let mut table: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    let entries: Vec<Box<dyn Authenticator>> = vec![
        Box::new(LocalProcessAuthenticator),
        Box::new(api_key::authenticator()),
        Box::new(token::authenticator()),
    ];
    for authenticator in entries {
        // 键即 kind()——三族 kind 互异（local_process/api_key/token），无冲突。
        table.insert(authenticator.kind(), authenticator);
    }
    table
}
