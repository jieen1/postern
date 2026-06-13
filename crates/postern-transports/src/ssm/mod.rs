//! `ssm` 形态：SSM 端口转发（feature 门控，承诺级骨架占位，`open` 体为 `todo!()`）。
//!
//! 在 `direct` 之上套一层隧道的 `Transport` 实现（§3.2）：经云侧 SSM 通道开启
//! `AWS-StartPortForwardingSession`（或到远程主机的变体），把目标端口转发回本地；
//! 本地端点即该端口转发会话在本地暴露的一侧，经 [`crate::pump`] 双向搬运字节。
//! SSM 会话是**有时限**的，故 `ssm` 是 [`crate::keepalive`] 协议级续约的主要使用方
//! （`expiry` 由远端会话建立时给出，临近阈值续约，续约失败即死亡不自愈），续约动作
//! 接 [`crate::keepalive::KeepaliveBackend`] 端口。`kind()` 取 `"ssm"`；属长连接型。
//!
//! 本模块仅在启用 `ssm` feature 时编译。具体 SSM 通路库（aws-sdk-ssm 等）一律在本
//! feature 内门控引入，默认不编译；不引入任何被封禁库。真实远端走 feature-gated
//! 集成测试（§9）。本波次为占位机制层：类型 + 签名对齐设计，`open` 体 `todo!()`，
//! 真实端口转发会话建立与续约由后续 feature 实现填入。

use async_trait::async_trait;
use postern_core::domain::{ResolvedTarget, ResourceCredential};
use postern_core::error::TransportError;
use postern_core::plugin::{Channel, Transport};

/// `ssm` 形态的注册键常量（§5.1 / F-8）：`kind()` 恒返回此值，用于传输注册表选型。
pub const KIND: &str = "ssm";

/// `ssm` 是否长连接型（§3.2 / §5.1 / F-6）——**编译期固定常量布尔**。
///
/// SSM 端口转发会话建立成本高、可保活续约复用 → 定为**长连接型** `true`（连接管理层
/// 据此可池化复用）。SSM 会话虽有时限、续约是 [`crate::keepalive`] 的主要使用方，但
/// 「长连接型」声明本身是常量；`persistent()` 读此常量，**不**读配置 / 会话时限（F-6）。
pub const PERSISTENT: bool = true;

/// SSM 端口转发形态的 `Transport`（§3.2 ssm）。
///
/// 与 `direct` / `ssh` 平行的薄壳：差异仅在 `open` 内部「建底层接入」一步（开
/// `AWS-StartPortForwardingSession` 端口转发会话）与有时限会话的续约节律，对上呈现
/// 的 `Channel` 抽象与 `kind()` / `persistent()` 取值之外一致（L-9）。本结构体**无
/// 任何字段**——不持凭据、不池化、不做通路间生命周期决策（那是 daemon 连接管理层）。
pub struct SsmTransport;

#[async_trait]
impl Transport for SsmTransport {
    /// 传输注册选择键：恒为 `"ssm"`（固定常量，不读配置 / 状态，F-6 精神）。
    fn kind(&self) -> &'static str {
        KIND
    }

    /// 长连接型声明：SSM 端口转发会话建立成本高、可保活续约复用 → 恒为**编译期常量**
    /// [`PERSISTENT`]（`true`，同一实例多次调用恒等，不依赖运行时状态 / 会话时限，F-6）。
    fn persistent(&self) -> bool {
        PERSISTENT
    }

    /// 建立 SSM 端口转发会话并暴露本地 socket（§3.2）。
    ///
    /// 数据流骨架：消费注入的 `(ResolvedTarget, ResourceCredential)` → 开
    /// `AWS-StartPortForwardingSession` 会话 → 本地端点经 [`crate::pump`] 双向桥接 →
    /// 组装 `Channel`；有时限会话经 [`crate::keepalive`] 临近阈值续约。失败路径
    /// **先关已半建的底层、再返脱敏 `Err(TransportError)`**，绝不返回伪健康 `Channel`
    /// （公理二，§5.1 / §7-6）。`target` / `cred` 以 move 持有、调用结束即释放。
    ///
    /// 本波次为占位：真实端口转发会话建立与续约由 `ssm` feature 的后续实现填入
    /// （aws-sdk-ssm 门控引入），真实远端正确性由 feature-gated 集成测覆盖。占位体不
    /// `panic` / `unwrap` / `expect`，以 `todo!()` 标记未实现路径（feature 关闭时本模块
    /// 整体不编译，无可达占位的运行期分支）。
    async fn open(
        &self,
        target: ResolvedTarget,
        cred: ResourceCredential,
    ) -> Result<Channel, TransportError> {
        // move 持有，随栈帧释放——不复制、不留存、不上抛（§7-1 / L-5）。
        let _ = (target, cred);
        todo!("ssm 真实端口转发会话建立与续约由 ssm feature 的后续实现填入（feature-gated 集成测覆盖）")
    }
}
