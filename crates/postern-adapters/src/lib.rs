//! `postern-adapters`：各类资源协议的**唯一解释者**（模块设计 §1）。
//!
//! 把某种协议的原始 `Intent` 翻译为 `(Capability 动词, 操作对象)`（`classify`），
//! 按资源声明的细则校验该归类（`check_constraint`），并在连接管理层递来的不透明
//! `Channel` 上执行已被求值放行的意图（`execute`），探测能力面（`discover`）。
//! 它解释协议、不做授权；只见通路、不见传输与地址。
//!
//! 本 crate 是库（无二进制），其 [`postern_core::plugin::Adapter`] 实现以模块 +
//! cargo feature 形态共存：`postgres` / `docker_logs` / `http`（§1、§5）。
//!
//! # 依赖纪律（契约 `ARCH_FORBIDDEN_EDGES` 强制）
//!
//! 本 crate **仅依赖 `postern-core`**（消费领域类型与 `Adapter` trait 定义，§6.4）。
//! **禁止依赖** `postern-secrets` / `postern-transports` / `postern-store`：
//! `Channel`/`RawResponse`/`CapabilitySurface` 等一律 `use postern_core::...`。
//!
//! # 模块组织
//!
//! - [`common`]：各协议共享的对象规范化等纯工具（无协议语义）。
//! - [`postgres`]：SQL 语法树级归类核心，`engine_enforced=true`（凭据分级兜底）。
//! - [`docker_logs`]：容器日志只读取数，`engine_enforced=false`（归类+细则是唯一防线）。
//! - [`http`]：HTTP API，`engine_enforced=false`（归类+细则是唯一防线）。
//!
//! mysql 及 redis/rabbitmq/command/deploy 留 feature 占位，本波次不实现。

pub mod common;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "docker_logs")]
pub mod docker_logs;

#[cfg(feature = "http")]
pub mod http;
