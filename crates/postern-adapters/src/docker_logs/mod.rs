//! `docker_logs` 适配器：容器日志只读取数（骨架占位，承诺级签名）。
//!
//! **`engine_enforced=false`：不存在引擎级账号分级，归类+细则是唯一防线**（§3.3 /
//! F-10 / L-9，此标注串经返回值 + 文档如实标注，公理三）。
//!
//! `Intent` 负载是**封闭枚举的只读取数请求**（容器选择符 + `since`/`tail`/`follow` 等
//! 只读取数参数），其形态本身**不含任何写表达**——没有「执行命令」「重启容器」这类变体
//! 可被构造。因此 `classify` 不做语法树遍历，只做「负载结构合法即归 `Observe`、否则
//! `Err`」的形态校验，恒归 `Observe`（§3.1）；只读性下沉到 `Intent` schema 层与远端
//! 只读端点 / 探针，而非靠运行期识别危险。`objects` 取 `container:<名>`。
//!
//! 子模块：
//! - [`intent`]：封闭枚举的只读取数请求负载与动词工具 `request` schema（§3.6）。
//! - [`classify`]：形态校验恒归 `Observe`（§3.1）。
//! - [`constraint`]：`container_prefix` 细则语义（§3.2）。
//! - [`execute`]：经只读日志端点 / 探针取数，`follow` 产流式 `RawResponse`（§3.4）。

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
pub const PROTOCOL: &str = "docker_logs";

/// 引擎级强制兜底声明（§3.3 / F-10 / L-9）——**编译期固定常量布尔**。
///
/// 容器类无引擎账号分级 → 恒为 `false`：归类+细则是唯一防线（须如实标注，公理三）。
pub const ENGINE_ENFORCED: bool = false;

/// docker_logs 适配器：容器日志只读取数的唯一解释者（§1）。
///
/// **无任何字段**——并发安全靠「无内部可变共享态 + `&self` 方法」（§3.7）。不持连接、
/// 不池化、不选 tier、不感知 tier（§4 边界）。
pub struct DockerLogsAdapter;

#[async_trait]
impl Adapter for DockerLogsAdapter {
    /// 协议注册键：恒为 `"docker_logs"`（§5）。
    fn protocol(&self) -> &'static str {
        PROTOCOL
    }

    /// 本协议可承载的动词集（§3.3）：容器日志恒只读 → 仅 `Observe`。
    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Observe]
    }

    /// 引擎级强制兜底：恒为**编译期常量** [`ENGINE_ENFORCED`]（`false`，§3.3 / F-10）。
    fn engine_enforced(&self) -> bool {
        ENGINE_ENFORCED
    }

    /// 步骤[2] 归类：委派 [`classify`]（形态校验恒归 `Observe`，§3.1）。
    fn classify(&self, intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
        classify::classify(intent)
    }

    /// 步骤[4] 细则：委派 [`constraint`]（`container_prefix`，§3.2）。
    fn check_constraint(
        &self,
        spec: &ConstraintSpec,
        ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError> {
        constraint::check(spec, ci)
    }

    /// 步骤[8] 执行：委派 [`execute`]（经只读端点 / 探针取数，§3.4）。
    async fn execute(&self, ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError> {
        execute::execute(ch, intent).await
    }

    /// 控制面发现（§3.5）：探测远端运行时 / 探针协议版本与可达只读端点，确认能力面
    /// 恒为只读（探针能力面恒不含写动词）。探针协商语义由 docker_logs 实现波次填实
    /// （§3.5 / 详设 6.12）；在此之前 **fail-closed**（公理二）——不 panic、不伪造能力面。
    async fn discover(&self, ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError> {
        let _ = ch;
        Err(DiscoverError::ProbeFailed)
    }
}
