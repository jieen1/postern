//! 快照域：权威库的原子投影构建与只读视图（§3.4 / §6.2）。
//!
//! `snapshot` 模块在**一次事务**内全量加载并完成角色继承展开、授权空间物化，产出
//! [`PolicySnapshot`](postern_core::domain::PolicySnapshot)——权威库的**原子投影**。
//! 求值零库访问、微秒级（内存查表）。重建与写入在**同一写锁临界区**完成
//! `Arc` 原子替换，保证"单一权威状态"无双源（§7-13）。
//!
//! 加载规则按表语义分两类（§3.4，与 fail-closed 一致）：
//!
//! - **授予性表**：加载 `delete_flag = 0 AND enable_flag = 1`（停用即收回授权）。
//! - **限制性表**：仅 `delete_flag = 0`，**绝不引入 `enable_flag` 过滤**（否则构成
//!   解冻 / 解约的 fail-open，§7-11 / L-2b）。
//!
//! fail-closed 兜底（存储层职责，§7-14）：引用链父行不可见 ⇒ 子行不入快照；同辖区
//! 多生效模式取最严格者（`freeze > maintain > observe > normal`，L-10）；选择器
//! 无法解析或展开为空集 ⇒ 该绑定不授予任何资源（空集、不报错也不放行，L-9）。

pub mod build;
pub mod view;

pub use build::build_snapshot;
pub use view::SnapshotView;
