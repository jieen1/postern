//! 系统级擦除集 ScrubSet 面：由 `targets`/`secrets` 派生的单向 match-and-erase 句柄。
//!
//! 职责（§3 第 6 项 / §5.4 / §6.4，详细设计 6.4 / 8.8）：本单元**构造、产出**系统级
//! ScrubSet 句柄——脱敏的**调用职责在内核**（`daemon::sanitize`），本单元不执行脱敏调用、
//! 不持有原子替换交付逻辑（那是 daemon 侧）。句柄是单向的：只 match-and-erase，不可枚举、
//! 不可序列化（§7-5 / §8 L-12）。
//!
//! - `build`：由 `Payload` 派生句柄的纯构造路径（`ScrubSet::from_payload`）。
//! - `handle`：不透明 `ScrubSet` 句柄类型与 `scrub`/`scrub_stream` 入口。

pub mod build;
pub mod handle;

pub use handle::{ScrubSet, SCRUB_MASK};
