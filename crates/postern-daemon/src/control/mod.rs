//! 控制面子域（模块文档 06 §8.10 / §3.4 control + 系统自动机）。
//!
//! 独立的 axum router 挂在 control.sock（0600 + 认证）上，承载策略读写端点。控制面的注入
//! 集合**与数据面截然不同**：它持 [`PolicyRepo`]（事务写）、机密面 [`Enrollment`] 接口、
//! [`AuditSink`]，但**绝不**被注入连接池 / Sanitizer（红线 7.2-2 / L-2 / L-14）。PolicyRepo
//! 句柄绝不进数据面注入集合——控制面与数据面共享底层 store，但句柄分持、互不串通。
//!
//! 写端点**三联动**（§8 L-14）：一次事务 COMMIT + 快照重建（同一写锁临界区内 Arc swap）+
//! 审计事件，三者同处一个写锁临界区；任一步失败 ⇒ 不 COMMIT、不重建、回 error + 审计、
//! 绝不留半态。集合端点强制分页（缺省 20、钳 200、回 `Page<T>` 信封，F-6）；乐观锁版本
//! 冲突 ⇒ 409 + `policy_change` 审计；系统协调写（sweeper / import）actor=system、不走乐观锁。
//!
//! 审批（F-6 / L-12）：escalate→内存待审队列，`on_timeout` **恒固定为 deny**（fail-closed）；
//! 审批关闭时 `escalate_denied` 不入队；进程重启 ⇒ 所有待审一律 deny。同步 PolicyRepo /
//! AuditSink（DB 写 / fsync）调用**必须**在 spawn_blocking 边界，绝不阻塞 async worker。
//!
//! 纪律（雷区）：本目录零 SQL 标记（写全经 store base::write / PolicyRepo）；需要 `ConnOrigin`
//! 时以 `use ... ConnOrigin as Origin` 别名读/解构（control/ 非 shells，字面即违规）；anyhow
//! 禁用，只用 thiserror / `DaemonError`。
//!
//! BLOCKER（见 type_level_notes）：postern-store 的 `PolicyRepo` + 快照重建为空占位、
//! postern-secrets 无 enrollment 接口——故控制面在此**定义自己的注入缝 trait**（[`PolicyRepo`]
//! / [`Enrollment`]），boot 在 store/secrets 缺口闭合后把真实实现接上；集成测试以内存 Fake
//! 驱动这些缝。端点 / 认证 / 审批 / router 已落实现，缝 trait 的真实 store/secrets 接入待
//! 缺口闭合。

pub mod approvals;
pub mod auth;
pub mod endpoints;
pub mod router;
pub mod verify;

use std::sync::Arc;

use postern_core::domain::{PrincipalId, ResourceCode};
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::AuditSink;

use crate::error::DaemonError;

/// 写操作者：`created_by` / `updated_by` 的取值来源（镜像 store `base::write::Actor`）。
///
/// 控制面写=已认证操作者标识（[`Actor::Operator`]）；系统协调写（sweeper / import）=
/// [`Actor::System`]，**不**走乐观锁。控制面只交"是谁在写"，五个审计字段由 store base 自动
/// 填充——daemon 绝不在写 API 暴露 `version` / `created_*` / `updated_*`（§7-2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// 控制面写入：已认证操作者标识。
    Operator(String),
    /// 系统自动写入（sweeper 回收 / import 协调）：落 `system`，不参与乐观锁。
    System,
}

/// 写端点提交的一次策略写意图（业务字段 + 乐观锁期望版本）。
///
/// 写 API **绝不**暴露五个审计字段（version 仅作乐观锁期望值传入，由 store 自增）；
/// [`expected_version`] 为 `None` 时为系统协调写（不走乐观锁，[`Actor::System`]）。
///
/// [`expected_version`]: WriteIntent::expected_version
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteIntent {
    /// 目标表 / 实体类别（资源 / 角色 / 绑定 ... 由端点固定，绝非请求自报任意表名）。
    pub entity: &'static str,
    /// 业务字段的 JSON 文本（由端点 DTO 序列化而来，零原始 SQL）。
    pub fields: serde_json::Value,
    /// 乐观锁期望版本；`None` ⇒ 系统协调写（不走乐观锁）。
    pub expected_version: Option<i64>,
}

/// 写端点三联动的结果（§8 L-14）：一次事务 COMMIT + 快照重建 + 审计三者同处一个写锁临界区，
/// 全成功才返回新版本号。任一步失败 ⇒ [`WriteError`]，无半态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    /// COMMIT 后新行 / 新版本的版本号（乐观锁下一期望值）。
    pub version: i64,
    /// 重建后的策略修订号（Arc swap 后的新快照 `policy_rev`）——审计对账锚点。
    pub policy_rev: u64,
}

/// 控制面写失败族（§8 L-14 三联动 / F-6 乐观锁）。
///
/// 穷尽 per-variant，无 `_ =>` 兜底臂。任一变体都意味着**不 COMMIT、不重建、无半态**
/// （fail-closed）。`VersionConflict` 由端点映射为 HTTP 409 + `policy_change` 审计。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    /// 乐观锁版本不符（期望 version 与库内当前不一致）⇒ HTTP 409 Conflict。
    #[error("version conflict")]
    VersionConflict,
    /// 事务 COMMIT 失败（IO / 约束）⇒ 不重建、回 error + 审计，无半态。
    #[error("transaction failed")]
    Transaction,
    /// 快照重建失败（事务已 COMMIT 但重建失败）⇒ fail-closed 整体回滚为 error，无半态。
    #[error("snapshot rebuild failed")]
    SnapshotRebuild,
    /// 三联动中的审计写失败 ⇒ 不 COMMIT、不重建，回 error。
    #[error("audit write failed")]
    Audit,
}

/// 控制面策略事务读写句柄（**daemon 侧注入缝**）。
///
/// BLOCKER：postern-store 的 `PolicyRepo` 为空占位、快照重建未实现——故控制面在此定义自己
/// 的缝 trait，boot 在 store 缺口闭合后把真实实现（经 base::write / snapshot::build）接上。
/// 此处签名对齐 store 文档承诺：写经唯一写路径（base::write，零原始 SQL）、乐观锁期望版本由
/// 调用方传入、集合读经 store scope/scan 分页层（daemon 只传 [`PageQuery`]、绝不拼 LIMIT-less
/// 查询）。所有方法为同步（store 同步驱动），调用方在 spawn_blocking 边界驱动（§5）。
pub trait PolicyRepo: Send + Sync {
    /// 写端点三联动（§8 L-14）：在同一写锁临界区内 事务 COMMIT + 快照重建（Arc swap）+
    /// 审计，全成功回 [`WriteOutcome`]；任一步失败 ⇒ [`WriteError`]，**绝不留半态**
    /// （不 COMMIT、不重建）。`actor` 决定 `created_by` / `updated_by`；
    /// `intent.expected_version` 为 `None`（[`Actor::System`]）时不走乐观锁。
    fn commit_write(&self, actor: &Actor, intent: &WriteIntent)
        -> Result<WriteOutcome, WriteError>;

    /// 集合读（强制分页，§7-7 / F-6）：经 store scope/scan 分页层执行，`page` 先 `clamp`
    /// （缺省 20、钳 200），回 `Page<T>` 信封。daemon 只传 [`PageQuery`]，绝不构造 LIMIT-less
    /// 查询。返回的每项为已脱敏的策略读模型 JSON（控制面读模型不含凭据材料）。
    fn list(
        &self,
        entity: &'static str,
        page: PageQuery,
    ) -> Result<Page<serde_json::Value>, DaemonError>;

    /// 当前权威快照修订号（`policy_rev`）——读端点 / 审计对账锚点。
    fn policy_rev(&self) -> Result<u64, DaemonError>;
}

/// 机密面登记（enrollment）接口（**daemon 侧注入缝**）。
///
/// BLOCKER：postern-secrets 尚无 enrollment 接口——故控制面在此定义自己的缝 trait，boot 在
/// 机密面缺口闭合后接上真实实现。控制面经此登记资源凭据档位（tier），**绝不**在 daemon 构造
/// 机密类型（`ResolvedTarget` / `ResourceCredential` 只在 postern-secrets 构造）——本缝只交
/// 不透明的登记结果码，daemon 不持有任何凭据材料。
pub trait Enrollment: Send + Sync {
    /// 为某资源代号登记一个凭据档位（机密材料由机密面落地，daemon 不经手）。
    /// 失败 fail-closed 回 [`DaemonError`]，不泄露机密细节。
    fn enroll(&self, resource: &ResourceCode, tier: &str) -> Result<(), DaemonError>;
}

/// 控制面注入集合（§8 L-2 / L-14 / 红线 7.2-2）。
///
/// **恰好**持 [`PolicyRepo`] + [`Enrollment`] + [`AuditSink`]，**绝无**连接池 / Sanitizer。
/// 这是控制面与数据面截然不同的注入集合：PolicyRepo 写句柄绝不进数据面、连接池/Sanitizer
/// 绝不进控制面。boot 装配一次，按 `Arc` 共享给 control router。
#[derive(Clone)]
pub struct ControlState {
    /// 策略事务读写句柄（事务写 + 快照重建 + 分页读）。
    pub policy: Arc<dyn PolicyRepo>,
    /// 机密面登记接口（资源凭据档位登记）。
    pub enrollment: Arc<dyn Enrollment>,
    /// 审计写句柄（写端点三联动的审计支、`policy_change` 留痕）。
    pub audit: Arc<dyn AuditSink>,
}

impl ControlState {
    /// 由注入的三个句柄装配控制面状态（boot 装配点交付）。
    ///
    /// 刻意只收三个句柄：连接池 / Sanitizer **无**对应参数——注入集合在类型层就排除了它们
    /// （红线 7.2-2 在编译期成立，而非运行期检查）。
    pub fn new(
        policy: Arc<dyn PolicyRepo>,
        enrollment: Arc<dyn Enrollment>,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        Self {
            policy,
            enrollment,
            audit,
        }
    }
}

/// 控制面待审升权的最终处置（§8 L-12）。
///
/// `on_timeout` **恒固定为 deny**（fail-closed）：审批关闭时 escalate 不入队、直接 deny；
/// 进程重启 ⇒ 所有待审一律 deny。本枚举只有 deny 一种终态语义（无 allow 变体——allow 在类型
/// 层不可表达，杜绝被误配置成在线放行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// 升权被拒（审批关闭 / 超时 / 重启）——审计 decision 词为 `escalate_denied`。
    Denied,
}

/// 一个 principal 对某资源的待审升权（内存待审队列条目，§8 L-12）。
///
/// 仅承载对账所需事实（principal / resource）；**无** `on_timeout` 字段可被外部置为 allow——
/// 超时处置在类型层固定为 deny。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    /// 申请升权的 principal。
    pub principal: PrincipalId,
    /// 申请升权的目标资源代号。
    pub resource: ResourceCode,
}
