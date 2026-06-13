//! `http` 适配器：HTTP API（骨架占位，承诺级签名）。
//!
//! **`engine_enforced=false`：不存在引擎级账号分级，归类+细则是唯一防线**（§3.3 /
//! F-10 / L-9，此标注串经返回值 + 文档如实标注，公理三）。一条被误归低危的写请求**不会**
//! 在下游被第二道防线拦下，故 http 的归类必须更保守。
//!
//! HTTP 没有 SQL 那样的协议级语义可解析，`classify` 依据**该资源声明的动词工具映射**
//! （接入时为该资源声明的 `(MCP 动词工具 → 方法×路径形态)` 表），把进来的
//! `(method, path)` 反查到声明的 `Capability`；命中声明形态归相应动词，未落任何声明形态
//! → `Err`（白名单，未声明即不可归类）。**不做任何启发式推断**（如「GET 即只读」在
//! 反代 / RPC-over-GET 场景会 fail-open，禁止采用）。`objects` 取 `route:<path>`（§3.1）。
//!
//! 子模块：
//! - [`intent`]：http `Intent` 负载（`method` / `path` / `headers` / `body`）与动词工具
//!   `request` schema（§3.6）。
//! - [`classify`]：按声明的动词工具映射反查 `Capability`（白名单，§3.1）。
//! - [`constraint`]：`http_route` 细则语义（§3.2）。
//! - [`execute`]：经 `Channel` 忠实转发到目标端点（§3.4）。

pub mod classify;
pub mod constraint;
pub mod execute;
pub mod intent;

use async_trait::async_trait;

use postern_core::domain::{Capability, ConstraintSpec};
use postern_core::error::{ClassifyError, ConstraintError, DiscoverError, ExecError};
use postern_core::plugin::{Adapter, CapabilitySurface, Channel, RawResponse};
use postern_core::request::{ClassifiedIntent, Intent};

/// 适配器注册表选型键（§5）：`protocol()` 恒返回此值。
pub const PROTOCOL: &str = "http";

/// 引擎级强制兜底声明（§3.3 / F-10 / L-9）——**编译期固定常量布尔**。
///
/// HTTP 无引擎账号分级 → 恒为 `false`：归类+细则是唯一防线（须如实标注，公理三）。
pub const ENGINE_ENFORCED: bool = false;

/// http 适配器：HTTP API 的唯一解释者（§1）。
///
/// **无任何字段**——并发安全靠「无内部可变共享态 + `&self` 方法」（§3.7）；动词工具映射
/// 为不可变只读结构。不持连接、不池化、不选 tier、不感知 tier（§4 边界）。
pub struct HttpAdapter;

#[async_trait]
impl Adapter for HttpAdapter {
    /// 协议注册键：恒为 `"http"`（§5）。
    fn protocol(&self) -> &'static str {
        PROTOCOL
    }

    /// 本协议可承载的动词集（§3.3）：按声明的动词工具映射可归 Observe/Query/Mutate 等
    /// （具体集合随资源声明，骨架先列读写两类常见档）。
    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Observe, Capability::Query, Capability::Mutate]
    }

    /// 引擎级强制兜底：恒为**编译期常量** [`ENGINE_ENFORCED`]（`false`，§3.3 / F-10）。
    fn engine_enforced(&self) -> bool {
        ENGINE_ENFORCED
    }

    /// 步骤[2] 归类：委派 [`classify`]（按声明动词工具映射反查，§3.1）。占位待实现。
    fn classify(&self, intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
        classify::classify(intent)
    }

    /// 步骤[4] 细则：委派 [`constraint`]（`http_route`，§3.2）。占位待实现。
    fn check_constraint(
        &self,
        spec: &ConstraintSpec,
        ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError> {
        constraint::check(spec, ci)
    }

    /// 步骤[8] 执行：委派 [`execute`]（经 `Channel` 忠实转发，§3.4）。占位待实现。
    async fn execute(&self, ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError> {
        execute::execute(ch, intent).await
    }

    /// 控制面发现（§3.5）：探测端点可达性与（若资源提供）声明式 API 描述，作为运维
    /// 声明动词工具映射的事实底稿。探测协议语义由 http 实现波次在集成测下填实（§3.5）；
    /// 在此之前 **fail-closed**（公理二）——不 panic、不伪造能力面。
    async fn discover(&self, ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError> {
        let _ = ch;
        Err(DiscoverError::ProbeFailed)
    }
}
