//! daemon 进程级错误词汇 + 「下游错误族 → 拒绝 stage」穷尽映射（模块文档 06 §3.8、
//! §6.1~§6.6、§8 L-3/L-5/L-6；core `error/stage.rs` 的 `Stage` 闭枚举）。
//!
//! 本文件是 layer-0 词汇，其它单元（kernel/connpool/control/boot/sweeper）共用：
//! - `DaemonError`：装配/IO/生命周期层失败基底（**全 crate 唯一** thiserror 枚举），
//!   统一 fail-closed 上抛；跨边界前由出口脱敏，不携带机密细节。
//! - `DownstreamError`：把六个下游域的失败族聚成一个枚举，供 `deny_stage` 穷尽分流。
//! - `deny_stage(&DownstreamError) -> Stage`：**无 `_ =>` 兜底臂**——新增一个下游变体
//!   而不写映射臂即编译失败（镜像 core「错误→stage」纪律，完整性是编译期义务）。
//! - `OutcomeDegraded`：内核出口降级错误码类型（「已执行但审计降级」），L-3 第③分支用——
//!   有副作用动词已 `execute` 后 outcome 审计写失败时返回该码，**绝不返 deny**。
//!
//! 路径纪律（build.rs 扫描器）：本文件在 `src/error.rs`（**非** `src/kernel/`），故
//! `.ok()`/`unwrap_or` 不在 `EVAL_NO_ERROR_SWALLOWING` 扫描范围内——但仍保持 fail-closed，
//! 绝不在此吞错。本文件零 SQL 标记，绝不构造/字面引用 `ConnOrigin`/`ResolvedTarget`/
//! `ResourceCredential`；`anyhow` 禁用（仅 main.rs），只用 `thiserror`。

use postern_core::error::{
    AuditError, AuthError, ClassifyError, ConstraintError, CredentialError, DiscoverError,
    ExecError, PredicateError, Stage, TransportError,
};
use postern_secrets::error::ResolveError;
use thiserror::Error;

/// daemon 装配层错误（全 crate 唯一 thiserror 枚举）。
///
/// 承载「装配/IO/生命周期」层面的失败：启动序列、UDS 监听、连接池、出口脱敏。
/// 数据面求值的拒绝走 core 的结构化 deny（带 `Stage`），**不**经此类型——求值链的
/// 错误先经 [`deny_stage`] 映射到 `Stage` 再组装 `DenyResponse`。
///
/// 刻意 **不** 标 `#[non_exhaustive]`：本 crate 是依赖图叶子（二进制），无跨 crate
/// 下游需要为未知变体兜底，闭枚举让内部消费者的 match 保持穷尽。
#[derive(Debug, Error)]
pub enum DaemonError {
    /// 启动序列某一步失败（开库/重建快照/解锁/注册插件/绑定 socket）。
    #[error("boot stage failed")]
    Boot,

    /// 外壳/控制面绑定或服务 UDS 监听失败。
    #[error("listener failed")]
    Listener,

    /// 连接池获取/复用/健康检查失败。
    #[error("connection pool failed")]
    Pool,

    /// 出口脱敏阶段失败（净化器拒绝放行）。
    #[error("sanitize failed")]
    Sanitize,

    /// 控制面端点指向的读/写能力在本波次尚未接通（store 侧无对应读模型 / 写接缝，留待
    /// 后续波次：settings/audit/mode/grants/denials/全量 bindings 等）。**刻意与
    /// [`Boot`](DaemonError::Boot)（真实内部失败）区分**：它是「明确未实现」而非「内部错误」，
    /// 故端点据此回 **501 Not Implemented** + 稳定机读码 [`NOT_IMPLEMENTED_CODE`]
    /// （镜像 credentials/discover 的「未启用」语义），绝不伪装成 500 内部失败（运维据 501
    /// 知悉「能力未接通」而非「daemon 坏了」）。fail-closed：未实现绝不静默放行 / 伪造空信封。
    #[error("control capability not implemented")]
    NotImplemented,
}

/// 控制面「能力未接通」的稳定机读码（[`DaemonError::NotImplemented`] 经端点出线时携带）。
///
/// 运维 / SPA / CLI 据此把「能力尚未接通」与「内部失败（500）」「乐观锁冲突（409）」逐一区分；
/// 与 `credentials_not_enabled` / `discover_not_enabled` 同一「未启用」族（稳定码、无机密）。
pub const NOT_IMPLEMENTED_CODE: &str = "not_implemented";

/// daemon 装配层 Result 别名。
pub type Result<T> = std::result::Result<T, DaemonError>;

/// 「已执行但审计降级」出口码（§6.2 / §8 L-3 第③分支）。
///
/// 有副作用动词在 `Adapter::execute` 完成 **之后**、[10] outcome 审计写失败时返回——
/// 操作确已生效，绝不能误报 deny（公理：已执行绝不返 deny）。承载一个可识别错误码，
/// 跨边界前脱敏、不含机密细节；与 [`DaemonError`] 区分，因其语义不是「失败拒绝」而是
/// 「成功但审计未落地」，调用方据此返回成功响应 + 标注降级，而非走 deny 路径。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("executed but audit downgraded")]
pub struct OutcomeDegraded {
    /// 触发降级的底层审计写失败族（仅常量码，无机密）。
    pub cause: AuditError,
}

/// 「已执行但审计降级」的**可识别常量码**（§8 L-3 第③分支）。
///
/// 已执行不变量要求 outcome 写失败时返回成功（绝不 deny），但该成功必须**可与干净成功
/// 区分**——否则降级在内核边界完全不可观察（fail-open）。本常量是那个识别码：内核出口把
/// 它装进降级信封（[`DowngradeEnvelope`]），随成功响应一并出口，调用方据此知悉「操作确
/// 已生效，但审计未落地」。常量码、无机密。
pub const OUTCOME_DOWNGRADED_CODE: &str = "executed_but_audit_downgraded";

/// 内核出口的「已执行但审计降级」信封（§8 L-3 第③分支）。
///
/// outcome 审计写失败时，内核仍回成功（已执行不变量），但把执行结果裹进本信封，附上
/// 可识别降级码 [`OUTCOME_DOWNGRADED_CODE`]，使「成功但审计降级」在出口**可观察**、与
/// 干净成功逐字节可区分。信封整体过同一 `Sanitizer`（F-10 出口统一脱敏）后离开内核；
/// 仅含常量码 + 已脱敏执行结果字节，无机密。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DowngradeEnvelope {
    /// 恒为 [`OUTCOME_DOWNGRADED_CODE`]：标记本响应「已执行但审计降级」。
    pub downgraded: &'static str,
    /// 触发降级的底层审计写失败常量码（无机密）。
    pub cause: &'static str,
    /// 已脱敏的执行结果字节（操作确已生效；结果不丢，仅标注降级）。
    pub payload: Vec<u8>,
}

impl DowngradeEnvelope {
    /// 由降级码 + 已脱敏执行结果字节组装信封（识别码恒置）。
    pub fn new(cause: &'static str, payload: Vec<u8>) -> Self {
        Self {
            downgraded: OUTCOME_DOWNGRADED_CODE,
            cause,
            payload,
        }
    }
}

impl OutcomeDegraded {
    /// 触发降级的底层审计写失败的**可识别常量码**（无机密，供出口信封承载）。
    pub fn cause_code(&self) -> &'static str {
        match self.cause {
            AuditError::WriteFailed => "audit_write_failed",
            AuditError::StorageUnavailable => "audit_storage_unavailable",
        }
    }
}

/// 求值链可能遇到的下游失败族聚合（每变体恰对应一个下游域错误枚举）。
///
/// 内核管线把任一下游 `Err` 装进对应变体，再交 [`deny_stage`] 穷尽分流到 `Stage`，
/// 据此组装 `Deny{stage}`。聚合成单一枚举是为了让 [`deny_stage`] 是 **一个** 带穷尽
/// match 的函数——新增一族失败而不在 `deny_stage` 写映射臂即编译失败。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DownstreamError {
    /// [1] 认证失败（`Authenticator::authenticate`）。
    #[error(transparent)]
    Auth(AuthError),
    /// [2] 归类失败（`Adapter::classify`）。
    #[error(transparent)]
    Classify(ClassifyError),
    /// [4] 细则检查失败（`Adapter::check_constraint`）。
    #[error(transparent)]
    Constraint(ConstraintError),
    /// [5] 条件谓词失败（`ConditionPredicate::eval`）。
    #[error(transparent)]
    Predicate(PredicateError),
    /// [6/7b] 资源凭据物化失败（`CredentialProvider::credential_for`）。
    #[error(transparent)]
    Credential(CredentialError),
    /// [7b] 通路建立失败（`Transport::open` / 通路生命周期）。
    #[error(transparent)]
    Transport(TransportError),
    /// [7b] 代号→真实地址解析失败（`resolve`）。
    #[error(transparent)]
    Resolve(ResolveError),
    /// [8] 执行失败（`Adapter::execute`）。
    #[error(transparent)]
    Exec(ExecError),
    /// [7a]/[10] 审计写失败（`AuditSink::record`）。
    #[error(transparent)]
    Audit(AuditError),
    /// 控制面 discover 失败（`Adapter::discover`）。
    #[error(transparent)]
    Discover(DiscoverError),
}

/// 「下游错误族 → 拒绝 stage」穷尽映射（§3.8 / §8 L-5/L-6）。
///
/// **无 `_ =>` 兜底臂**：每族一条显式映射臂，新增下游变体不写映射即编译失败
/// （镜像 core 的「错误→stage」纪律）。映射约定（§3.8、§6.1~§6.3）：
/// - `Auth` → [`Stage::Auth`]
/// - `Classify` → [`Stage::Classify`]
/// - `Constraint` → [`Stage::Constraint`]
/// - `Predicate` → [`Stage::Condition`]
/// - `Credential` / `Transport` / `Resolve` → [`Stage::Transport`]（= "connect" 拒绝阶段：
///   凭据物化 / 通路建立 / 地址解析失败在 daemon 层统一折叠为建连失败，fail-closed，
///   不降级、不改路；区别于 core 的 per-enum `stage()` 把 `CredentialError` 归 `Tier`）
/// - `Exec` → [`Stage::Exec`]
/// - `Audit` → [`Stage::Audit`]
/// - `Discover` → [`Stage::Discover`]
pub fn deny_stage(err: &DownstreamError) -> Stage {
    match err {
        DownstreamError::Auth(_) => Stage::Auth,
        DownstreamError::Classify(_) => Stage::Classify,
        DownstreamError::Constraint(_) => Stage::Constraint,
        DownstreamError::Predicate(_) => Stage::Condition,
        DownstreamError::Credential(_) => Stage::Transport,
        DownstreamError::Transport(_) => Stage::Transport,
        DownstreamError::Resolve(_) => Stage::Transport,
        DownstreamError::Exec(_) => Stage::Exec,
        DownstreamError::Audit(_) => Stage::Audit,
        DownstreamError::Discover(_) => Stage::Discover,
    }
}
