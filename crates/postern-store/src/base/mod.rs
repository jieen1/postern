//! 统一基础仓储：全工作区**唯一写路径**与受约束读路径（layer 0，store 内其他
//! 单元都依赖它）。承载契约 D8 的全部写侧落点与 store 的共享类型。
//!
//! 子模块（§3.1）：
//! - [`error`]：全 crate 唯一的 thiserror [`StoreError`](error::StoreError) 枚举
//!   （rusqlite 原始错误就地映射、不外泄）；
//! - [`db`]：开库（`bundled`、`foreign_keys=ON`、WAL）+ 写互斥锁句柄 +
//!   [`with_write_txn`](db::Db::with_write_txn) 事务包裹器；
//! - [`timestamp`]：policy.db 时间列与审计 `ts` 的唯一格式化点；
//! - [`normalize`]：名称入库归一化（`trim` + 小写）；
//! - [`scope`]：默认作用域（`delete_flag = 0`）+ 分页执行器；
//! - [`meta`]：持久 `policy_meta` 键值表读路径（当前仅 `policy_rev`，写在 [`write`]）；
//! - [`write`]：唯一写路径（INSERT/UPDATE/逻辑删除/级联/系统协调写/`policy_rev` 自增）。
//!
//! 可见性（§5.2 / F-6）：`base` 仓储仅本 crate 内可见——不导出为跨 crate 公开
//! 接口。`base` 不出现在 store 的对外类型表中，依赖图也禁止除 daemon 之外的 crate
//! 依赖 store（契约 `ARCH_FORBIDDEN_EDGES`），故"不作为跨 crate 公开接口"由架构
//! 边界保证；模块内各项以 `pub` 暴露仅为同 crate 的兄弟单元与集成测试可达。

pub mod db;
pub mod error;
pub mod meta;
pub mod normalize;
pub mod scope;
pub mod timestamp;
pub mod write;
