//! `ssh` 形态：SSH 隧道（feature 门控，承诺级骨架占位，`open` 体为 `todo!()`）。
//!
//! 在 `direct` 之上套一层隧道的 `Transport` 实现（§3.2）：与跳板 / 堡垒主机建立 SSH
//! 传输层会话（凭据来自注入的 `ResourceCredential`，可为私钥或口令），在会话内开一条
//! 到「真实地址:端口」的 `direct-tcpip` 转发信道；本地起监听端点，经 [`crate::pump`]
//! 把本地端点字节双向泵到这条 SSH 信道。对上呈现的仍是本地 socket，Agent / 适配器
//! 看不见跳板。`kind()` 取 `"ssh"`；属长连接型（隧道建立成本高、可池化复用），由
//! [`crate::keepalive`] 维持 SSH keepalive 心跳（接 [`crate::keepalive::KeepaliveBackend`]
//! 端口）。
//!
//! 本模块仅在启用 `ssh` feature 时编译。具体 SSH 通路库（russh / ssh2 等）一律在本
//! feature 内门控引入，默认不编译；不引入任何被封禁库。真实远端走 feature-gated
//! 集成测试（§9）。本波次为占位机制层：类型 + 签名对齐设计，`open` 体 `todo!()`，
//! 真实隧道建立由后续 feature 实现填入。

use async_trait::async_trait;
use postern_core::domain::{ResolvedTarget, ResourceCredential};
use postern_core::error::TransportError;
use postern_core::plugin::{Channel, Transport};

/// `ssh` 形态的注册键常量（§5.1 / F-8）：`kind()` 恒返回此值，用于传输注册表选型。
pub const KIND: &str = "ssh";

/// `ssh` 是否长连接型（§3.2 / §5.1 / F-6）——**编译期固定常量布尔**。
///
/// SSH 隧道建立成本高、可保活复用 → 定为**长连接型** `true`（连接管理层据此可池化）。
/// `persistent()` 读此常量，**不**读配置 / 通路状态（F-6）。
pub const PERSISTENT: bool = true;

/// SSH 隧道形态的 `Transport`（§3.2 ssh）。
///
/// 与 `direct` 平行的薄壳：差异仅在 `open` 内部「建底层接入」一步（建 SSH 会话 + 开
/// `direct-tcpip` 信道），对上呈现的 `Channel` 抽象与 `kind()` / `persistent()` 取值
/// 之外一致（L-9）。本结构体**无任何字段**——不持凭据、不池化、不做通路间生命周期
/// 决策（那是 daemon 连接管理层）。
pub struct SshTransport;

#[async_trait]
impl Transport for SshTransport {
    /// 传输注册选择键：恒为 `"ssh"`（固定常量，不读配置 / 状态，F-6 精神）。
    fn kind(&self) -> &'static str {
        KIND
    }

    /// 长连接型声明：SSH 隧道建立成本高、可保活复用 → 恒为**编译期常量** [`PERSISTENT`]
    /// （`true`，同一实例多次调用恒等，不依赖运行时状态，F-6）。
    fn persistent(&self) -> bool {
        PERSISTENT
    }

    /// 建立 SSH 隧道并暴露本地 socket（§3.2）。
    ///
    /// 数据流骨架：消费注入的 `(ResolvedTarget, ResourceCredential)` → 建 SSH 会话 →
    /// 开 `direct-tcpip` 信道 → 本地起端点经 [`crate::pump`] 双向桥接 → 组装 `Channel`。
    /// 失败路径**先关已半建的底层、再返脱敏 `Err(TransportError)`**，绝不返回伪健康
    /// `Channel`（公理二，§5.1 / §7-6）。`target` / `cred` 以 move 持有、调用结束即释放。
    ///
    /// 本波次为占位：真实隧道建立由 `ssh` feature 的后续实现填入（russh / ssh2 门控
    /// 引入），真实远端正确性由 feature-gated 集成测覆盖。占位体不 `panic` / `unwrap` /
    /// `expect`，以 `todo!()` 标记未实现路径（feature 关闭时本模块整体不编译，无可达
    /// 占位的运行期分支）。
    async fn open(
        &self,
        target: ResolvedTarget,
        cred: ResourceCredential,
    ) -> Result<Channel, TransportError> {
        // move 持有，随栈帧释放——不复制、不留存、不上抛（§7-1 / L-5）。
        let _ = (target, cred);
        todo!("ssh 真实隧道建立由 ssh feature 的后续实现填入（feature-gated 集成测覆盖）")
    }
}
