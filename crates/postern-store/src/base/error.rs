//! 本域错误枚举：底层驱动错误就地归类、不外泄。
//!
//! 全 crate 唯一的 thiserror 错误枚举（每 crate 一个错误枚举纪律）：
//! `schema_migrate` / `policy` / `snapshot` / `audit` 单元都复用本枚举，故本
//! 单元先落地它。底层驱动的原始错误（连同库路径、SQL 片段）在 `base` 内即映射
//! 为本枚举的语义变体，**绝不**把原始驱动错误串透出 crate 边界（机密红线 7.5）。
//!
//! 关键映射须语义分明（§3.6 错误处理）：
//! - 乐观锁影响行数为 0 → [`StoreError::VersionConflict`]（独立变体，控制面据此
//!   返 `409 Conflict`），绝不与"行不存在 / IO 失败"混淆；
//! - 约束违反（partial unique / CHECK / 限制性表 `enable_flag≠1`）→
//!   [`StoreError::ConstraintViolation`]，fail-closed 拒写、库不变；
//! - 开库 / IO 失败 → [`StoreError::Io`]；
//! - 迁移版本不识别 → [`StoreError::UnknownSchemaVersion`]（留给 `schema_migrate`
//!   单元复用）。

use thiserror::Error;

/// 存储载体域的唯一错误枚举。变体文案为常量英文，绝不回显库路径、SQL 片段、
/// 业务数据或原始驱动错误串（机密红线 7.5）。**非** `#[non_exhaustive]`：新增变体
/// 须在 core 的穷尽映射里分类，否则不可编译。
#[derive(Debug, Error)]
pub enum StoreError {
    /// 乐观锁冲突：UPDATE 影响行数为 0（期望 `version` 与库中不符）。独立变体，
    /// 控制面据此返 `409 Conflict`，绝不与"行不存在 / IO 失败"混为一谈，绝不静默重试。
    #[error("optimistic-lock version conflict")]
    VersionConflict,

    /// 约束违反：partial unique、CHECK 或限制性表 `enable_flag≠1` 等被拒。
    /// fail-closed，库不变。
    #[error("constraint violation")]
    ConstraintViolation,

    /// 开库或底层 IO 失败（不回显库路径 / 原始驱动错误串）。
    #[error("storage io failure")]
    Io,

    /// 迁移：库版本高于当前实现已知最高版本，fail-closed 拒绝按旧假设解析。
    #[error("unknown schema version")]
    UnknownSchemaVersion,

    /// id 生成失败（时钟回拨 / 溢出），沿 core IdGen 的 fail-closed 语义传播。
    #[error("id generation failed")]
    IdGen,
}
