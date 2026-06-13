//! 连接管理子域（模块文档 06 §8.5 / §3.5）。
//!
//! 池键为 (ResourceCode, CredentialTier)：获取/复用/健康/上限/回收/中断/归池前会话净化
//! 全在此。tier 之间绝不共享连接（账号隔离在连接粒度成立，L-8）。daemon 绝不构造机密类型
//! （ResolvedTarget / ResourceCredential 只在 postern-secrets 构造）；本子域经
//! CredentialProvider/映射解析一次性取**不透明句柄**，按值传给 Transport::open，全程不构造
//! 它们，调用边界外句柄即时释放（不入池、不缓存，F-7 / L-17）。
//!
//! 本波次为骨架：池/租约/退避子模块声明 + 获取入口与连接审计事件类型桩，零连接逻辑。

pub mod backoff;
pub mod lease;
pub mod pool;

use postern_core::domain::{CredentialTier, ResourceCode};
use postern_core::error::Stage;

/// 连接审计事件（`connection_event`，§3.5 / F-7）。
///
/// 通路建立 / 健康剔除 / 回收 / 强制中断各落一条，字段**恰为** resource、tier 名、
/// transport 种类——**绝不含真实地址 / 凭据**（地址 / 凭据从未进入本层可读形态）。
/// 该审计由连接管理层写入（传输层只如实上报健康事实、不写审计，§6.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionEvent {
    /// 触发该事件的连接生命周期阶段。
    pub phase: ConnPhase,
    /// 目标资源代号（恒为代号，绝不为真实地址）。
    pub resource: ResourceCode,
    /// 凭据档位名（tier 名，不含凭据材料）。
    pub tier: CredentialTier,
    /// 传输种类键（`direct` / `ssh` / `ssm`），取自 `Transport::kind()`。
    pub transport_kind: String,
}

/// 连接生命周期阶段——`connection_event` 的写入点（§3.5「通路建立/健康剔除/回收/强制中断」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnPhase {
    /// 通路建立（`Transport::open` 成功新建一条 `Channel`）。
    Establish,
    /// 健康剔除（周期健康检查判定通路死亡、从池槽剔除）。
    HealthEvict,
    /// 回收（空闲回收 / 优雅销毁，或归池前净化失败销毁）。
    Recycle,
    /// 强制中断（freeze / 吊销对在用连接 abort/cancel）。
    Abort,
}

/// `acquire` 失败码——映射到 connect 拒绝阶段（§3.8 / L-6）。
///
/// 凭据物化 / 代号解析 / 通路建立失败在 daemon 层**统一折叠**为建连失败：fail-closed、
/// 不降级、不改路、不静默重试到他路。上游据 [`AcquireError::stage`] 组装 `Deny{stage}`，
/// 错误跨边界前脱敏、**不含真实地址 / 凭据**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireError {
    /// 凭据物化失败（`CredentialProvider::credential_for`）。
    Credential,
    /// 代号→真实地址解析失败（`UnlockedVault::resolve`）。
    Resolve,
    /// 通路建立失败（`Transport::open` / 通路生命周期）。
    Transport,
    /// 无匹配 transport 种类（登记册选型未命中，fail-closed）。
    NoTransport,
    /// 超并发上限且有界等待队列已满（容量 `Q` 触顶，背压即 deny）。
    CapacityExceeded,
    /// 该池键处于退避窗口内（上次建连死亡后的指数退避未到期）——拒绝风暴重连
    /// （§8 健康与退避状态机：退避期内 acquire 走 deny 或有界等待，绝不立即重连）。
    BackoffActive,
}

impl AcquireError {
    /// 拒绝 stage 归因——五支**全部**折叠到 `Stage::Transport`（= "connect" 拒绝阶段，
    /// §3.8 / L-5 / L-6）。穷尽 per-variant match，无 `_ =>` 兜底臂。
    pub fn stage(&self) -> Stage {
        match self {
            AcquireError::Credential => Stage::Transport,
            AcquireError::Resolve => Stage::Transport,
            AcquireError::Transport => Stage::Transport,
            AcquireError::NoTransport => Stage::Transport,
            AcquireError::CapacityExceeded => Stage::Transport,
            AcquireError::BackoffActive => Stage::Transport,
        }
    }
}
