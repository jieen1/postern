//! 映射面：代号↔真实地址解析（设计承诺级桩，函数体未实现）。
//!
//! 职责（§3 地址解析 / §5.4 / 详细设计 4.3）：对已解锁保险箱句柄持有的
//! `targets` 段做**纯查表**解析——把资源代号物化为不透明 `ResolvedTarget`。
//! 纯内存、零 IO、无运行期状态、常数级查表。
//!
//! 唯一构造点（§5.1 / §8 F-5、契约 `SEC_CONSTRUCTION_SITES`）：`ResolvedTarget`
//! 在 `postern-core` 中仅有不透明声明（无构造路径），本 crate 写其结构体字面量
//! 即唯一构造点。机密类型纪律（§7-1、契约 `SEC_SECRET_TYPE_DISCIPLINE`）：不
//! derive / 手写 `Clone` / `Serialize`，`Debug` 由 core 恒输出 `REDACTED`，本
//! crate 不另加任何 impl。
//!
//! fail-closed（§8 F-4 / L-5、§6.3）：未知代号 → `Err(ResolveError::UnknownCode)`、
//! 无产物；句柄不可用 → `Err(ResolveError::VaultUnavailable)`。签名层即无"缺省
//! 地址"返回路径。明文不出边界（§8 L-11）：成功只返回不透明 `ResolvedTarget`，
//! 错误侧只返回常量英文错误码、绝不内插 code / 真实地址明文。

pub mod resolve;
