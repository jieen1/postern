//! postgres 发现：能力面探测（骨架占位，§3.5）。
//!
//! 在递来的 `&mut Channel` 上**只发只读的元信息探测**，把资源的客观能力事实装进
//! `CapabilitySurface`：引擎版本、可见 schema / 表清单、以及该接入账号的**真实权限**
//! （后者是 §3.3「tier 声明权限 ⊆ 底层账号真实权限」前提的取证来源——声明只读却实测
//! 可写的账号据此可见缺口）。
//!
//! 产物是**纯事实、零授权字段**——`CapabilitySurface` 绝不含 allow/tier/grant；授权化是
//! 人经控制面圈选的后续动作（发现≠授权）。仅控制面触发，数据面无任何路径触达它
//! （CONS-20 命名边界，L-12）。探测失败一律 `DiscoverError`，绝不据失败 / 部分结果生成
//! 任何授权（fail-closed）。

use postern_core::error::DiscoverError;
use postern_core::plugin::{CapabilitySurface, Channel};

/// 控制面发现（§3.5）：在 `Channel` 上探测能力面，产出纯事实 `CapabilitySurface`。
///
/// 在递来的 `&mut Channel` 上**只发只读的元信息探测**（引擎版本 / 可见 schema 表清单 /
/// 接入账号真实权限），把客观能力事实装入 `CapabilitySurface`——产物纯事实、**零授权
/// 字段**（绝不含 allow/tier/grant；发现≠授权）。
///
/// `Channel.handle` 是 `Box<dyn Send + Sync>` 的不透明本地通路抽象：真实 pg 客户端在容器
/// 集成层（`pg-itest`）接管底层流、对可达引擎回 `Ok(CapabilitySurface)`。本波次无真实
/// 引擎，本地通路上的只读元信息探测无法落地——一律 **fail-closed** 为
/// `Err(DiscoverError::ProbeFailed)`：探测连不上 / 不可读即失败，**绝不据失败或部分结果
/// 伪造任何 `Ok` 能力面**（公理二，§3.5）。失败唯一表达是 `Err(DiscoverError)`。
pub async fn discover(ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError> {
    let _ = ch;

    // 本地通路无可达引擎可探：只读元信息探测失败，fail-closed 为 ProbeFailed（发现≠授权）。
    Err(DiscoverError::ProbeFailed)
}
