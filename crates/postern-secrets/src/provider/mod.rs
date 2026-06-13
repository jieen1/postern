//! Provider 面：`CredentialProvider` 实现（设计承诺级桩，函数体 `todo!()`）。
//!
//! 职责（§3 凭据解析 / §5.3 / 详细设计 4.3、6.13）：实现 core 定义的
//! `#[async_trait] CredentialProvider`（签名权威：`core::plugin::channel`）。按持久凭据
//! 形态分两类来源，对外都收敛为同一不透明 `ResourceCredential`，调用方不感知内部形态：
//! - **静态来源**（`static_vault`）：数据库密码/redis ACL 账号/API token——解锁后从
//!   payload `secrets` 段纯查表物化，无运行期状态、零 IO（§3 静态来源）。
//! - **会话来源**（`session`）：账号密码/OAuth——有状态 live-session 缓存，"登录一次→复用→
//!   续期/重登"，单飞续会话、硬过期 fail-closed（详细设计 6.13）。
//!
//! 唯一构造点（§5.1 / §8 F-5、契约 `SEC_CONSTRUCTION_SITES`）：`ResourceCredential`
//! 在 core 仅有不透明声明（无构造路径），本 crate 写其结构体字面量即唯一构造点。
//!
//! 错误词汇（§3.1）：复用 core `CredentialError`（NotFound/VaultUnavailable/RefreshFailed/
//! InteractiveAuthRequired），**本域不重定义**，import 即可。每变体不内插 res/tier/账号明文。
//!
//! 依赖冲突上报（见 static_vault 头注释 / notes）：`CredentialProvider` 标 `#[async_trait]`，
//! 而 `async-trait` proc-macro 不在 secrets 白名单、core 未 re-export——本桩以手工 desugar
//! 的 future 返回签名实现该 trait，不擅自引入 `async-trait`；是否纳入白名单由 wrap 裁决。

pub mod session;
pub mod static_vault;
