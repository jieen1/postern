//! 策略库结构定义入口：内嵌 schema 文本与表元数据。
//!
//! 本模块是 `DB_BASE_FIELDS_REQUIRED` 与 `SEC_ADMIN_NOT_GRANTABLE` 两条契约扫描器
//! 的**真来源**：契约扫描 `src/schema.sql` 里的全部建表块，逐表核对 8 基础列齐备、
//! `roles` 带禁 admin 名 CHECK。schema 文本经 [`include_str!`] 内嵌为 [`SCHEMA_SQL`]，
//! 与扫描器看到的字节完全一致（单一事实源，无第二份漂移）。
//!
//! schema 版本走 `PRAGMA user_version`：[`CURRENT_SCHEMA_VERSION`] 是当前实现已知
//! 的最高版本，迁移据它判定三态（见 [`crate::migrate`]）。

/// 内嵌的 policy.db 结构定义（与契约扫描器读取的 `src/schema.sql` 同一字节）。
pub const SCHEMA_SQL: &str = include_str!("schema.sql");

/// 当前实现内置的"最高已知 schema 版本"。空库建库后 `PRAGMA user_version` 前进至此；
/// 迁移读库版本与之比对分三态处置（相等幂等 / 更低前向迁移 / 更高 fail-closed）。
///
/// v2 起新增持久 `policy_meta` 键值表（承载单调 `policy_rev`），由 v1→v2 前向步建表
/// 并播种 `policy_rev = 0`（见 [`crate::migrate::ddl`]）。
pub const CURRENT_SCHEMA_VERSION: i64 = 2;

/// 持久元数据表名（v2 起）：键值对承载 store 级标量状态，当前仅 `policy_rev`。
/// **非业务表**——无 8 基础字段、无逻辑删除语义，故不入 [`BUSINESS_TABLES`]，也不
/// 受 `delete_flag` 默认作用域约束（其读写落点在 `src/base/`，契约扫描器据路径豁免）。
pub const POLICY_META_TABLE: &str = "policy_meta";

/// `policy_meta` 中持久策略修订号的键名。其值单调递增、跨重启存活，由
/// [`crate::base::write::bump_policy_rev`] 原子 +1、[`crate::base::meta::read_policy_rev`] 读取。
pub const POLICY_REV_KEY: &str = "policy_rev";

/// 统一基础字段（8 列，5.1-①）：每张业务表都必须按此序声明在最前。
pub const BASE_COLUMNS: [&str; 8] = [
    "id",
    "version",
    "created_at",
    "created_by",
    "updated_at",
    "updated_by",
    "delete_flag",
    "enable_flag",
];

/// policy.db 全部业务表清单（5.2）。授予性表与限制性表合并列出，顺序与 schema.sql
/// 的建表块一致，供迁移建表与测试核对"表都建齐了"。
pub const BUSINESS_TABLES: [&str; 15] = [
    "principals",
    "credentials",
    "roles",
    "role_inherits",
    "role_capabilities",
    "resources",
    "resource_labels",
    "resource_credential_tiers",
    "bindings",
    "binding_scope",
    "grant_constraints",
    "grant_conditions",
    "temp_grants",
    "mode_state",
    "deny_notes",
];

/// 限制性表清单（5.2：禁用 `enable_flag`，建表带 `CHECK (enable_flag = 1)`）。
/// `settings` 同为限制性表，但其落点在 base 单元，本 schema 单元的业务表清单不含它。
pub const RESTRICTED_TABLES: [&str; 4] = [
    "grant_constraints",
    "grant_conditions",
    "mode_state",
    "deny_notes",
];
