//! `postgres` 适配器：SQL 协议级语义归一化核心（骨架占位，承诺级签名）。
//!
//! **`engine_enforced=true`**：存在**引擎级强制兜底**——`Decision::Allow{tier}` 选定
//! 的凭据等级在数据库引擎账号层受真实权限约束，即便 `classify` 把一条伪装写误归为
//! `Query`，它也只走只读账号、被引擎拒绝。归类是第一道线，引擎账号是强制兜底（§3.3）。
//!
//! 归类规约（§3.1 D5）：原文 → 语法树 → 按语句树内**最高危写节点**定档（穿透 CTE /
//! 子查询 / `INTO` 等只读外壳，绝不降级）；无法可靠归类一律 `Err`（公理二）。归类用
//! 语法树枚举变体判别（不靠对原文做关键字子串匹配），故本 crate 源文件内**零 SQL 文本
//! 标记**——SQL 测试输入语料全部放进 `tests/corpus/` 下的数据文件（B 方案）。
//!
//! 子模块：
//! - [`intent`]：postgres `Intent` 负载结构与 MCP 动词工具 `request` schema（§3.6 F-12）。
//! - [`classify`]：语法树级归类（分级定档 + 对象提取同遍历，§3.1）。
//! - [`constraint`]：`table_allow` / `column_mask` 细则语义（§3.2）。
//! - [`execute`]：在 `Channel` 上以线协议执行已放行意图（§3.4）。
//! - [`discover`]：探测引擎版本 / schema / 账号真实权限（§3.5）。

pub mod classify;
pub mod constraint;
pub mod discover;
pub mod execute;
pub mod intent;

use async_trait::async_trait;

use postern_core::domain::{Capability, ConstraintSpec};
use postern_core::error::{ClassifyError, ConstraintError, DiscoverError, ExecError};
use postern_core::plugin::{Adapter, CapabilitySurface, Channel, RawResponse};
use postern_core::request::{ClassifiedIntent, Intent};

/// 适配器注册表选型键（§5）：`protocol()` 恒返回此值。
pub const PROTOCOL: &str = "postgres";

/// 引擎级强制兜底声明（§3.3 / F-10 / L-9）——**编译期固定常量布尔**。
///
/// SQL 类存在引擎账号权限模型 → 恒为 `true`。`engine_enforced()` 读此常量，**不**读
/// 配置 / 运行状态；其「为真的取证」由同一适配器 [`discover`] 探出账号真实权限供给（§3.3）。
pub const ENGINE_ENFORCED: bool = true;

/// postgres 适配器：SQL 唯一解释者（§1）。
///
/// **无任何字段**——并发安全靠「无内部可变共享态 + `&self` 方法」而非锁（§3.7，
/// `Adapter: Send + Sync`）；归类表 / 动词集为不可变只读结构。不持连接、不池化、
/// 不选 tier、不感知 tier（§4 边界）。
pub struct PostgresAdapter;

#[async_trait]
impl Adapter for PostgresAdapter {
    /// 协议注册键：恒为 `"postgres"`（§5）。
    fn protocol(&self) -> &'static str {
        PROTOCOL
    }

    /// 本协议可承载的动词集（§3.3）：SQL 可归 Observe/Query/Mutate/Destroy 四档。
    fn capabilities(&self) -> &'static [Capability] {
        &[
            Capability::Observe,
            Capability::Query,
            Capability::Mutate,
            Capability::Destroy,
        ]
    }

    /// 引擎级强制兜底：恒为**编译期常量** [`ENGINE_ENFORCED`]（`true`，§3.3 / F-10）。
    fn engine_enforced(&self) -> bool {
        ENGINE_ENFORCED
    }

    /// 步骤[2] 归类：委派 [`classify`]（语法树级最高危写定档，§3.1）。占位待实现。
    fn classify(&self, intent: &Intent) -> Result<ClassifiedIntent, ClassifyError> {
        classify::classify(intent)
    }

    /// 步骤[4] 细则：委派 [`constraint`]（`table_allow` / `column_mask`，§3.2）。占位待实现。
    fn check_constraint(
        &self,
        spec: &ConstraintSpec,
        ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError> {
        constraint::check(spec, ci)
    }

    /// 步骤[8] 执行：委派 [`execute`]（在 `Channel` 上以线协议执行，§3.4）。占位待实现。
    async fn execute(&self, ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError> {
        execute::execute(ch, intent).await
    }

    /// 控制面发现：委派 [`discover`]（探测引擎能力面 / 账号真实权限，§3.5）。占位待实现。
    async fn discover(&self, ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError> {
        discover::discover(ch).await
    }
}
