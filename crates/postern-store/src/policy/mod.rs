//! 策略状态事务读写编排域：仅控制面可达。
//!
//! [`PolicyRepo`](repo::PolicyRepo) 把 [`base`](crate::base) 的唯一写路径组织为面向
//! 策略状态的**事务**读写句柄（§3.3）。纪律（§7）：
//!
//! - **写一律经 `base`**：本单元绝不直接出现 INSERT/UPDATE/逻辑删除语句——它们只在
//!   `src/base/write.rs`（契约 `DB_WRITE_PATH_CENTRALIZED`）。`PolicyRepo` 的每个写
//!   方法都在一次 [`Db::with_write_txn`](crate::base::db::Db::with_write_txn) 事务内
//!   调 `base::write::{insert,update,logical_delete,cascade_logical_delete}`。
//! - **审计字段自动化**：写 API **不暴露** `version/created_*/updated_*` 五字段参数；
//!   `created_by/updated_by` 取值经 [`Actor`](crate::base::write::Actor)（控制面=操作
//!   者标识、系统写=`system`）。
//! - **乐观锁端到端**：更新/逻辑删除 API **要求携带期望 `version`**；影响 0 行 →
//!   [`StoreError::VersionConflict`](crate::base::error::StoreError::VersionConflict)，
//!   绝不静默重试。读端点统一返回 `version`（供调用方下一次写带上）。
//! - **级联逻辑删除**：父表逻辑删除时，在**同一事务**内级联把直接子行 `delete_flag`
//!   置 1、`updated_by` 标 `cascade:<table>#<id>`（§3.2 级联图）；任一步失败整体
//!   ROLLBACK（父子行均不变）。
//! - **默认作用域 + 分页**：集合读默认追加 `delete_flag = 0`、接收
//!   [`PageQuery`](postern_core::page::PageQuery) 返回 [`Page<T>`](postern_core::page::Page)
//!   （`LIMIT` 封顶、`clamp`）。

pub mod repo;

pub use repo::{BindingRow, PolicyRepo, PrincipalRow, ResourceRow, RoleRow};
