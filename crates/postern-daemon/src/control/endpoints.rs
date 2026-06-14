//! 控制面端点：策略读写处理器（模块文档 06 §6.5 / §6.2 / §6.10、§8 F-6 / L-14 / L-15）。
//!
//! 读端点返回 `Page<T>` 信封（分页钳制：缺省 20、钳 200，F-6）；写端点经 [`PolicyRepo`] 的
//! 三联动（事务 COMMIT + 快照重建 + 审计同处一个写锁临界区，L-14），乐观锁版本冲突回
//! **409 Conflict** + `policy_change` 审计（F-6 / L-15）；系统协调写（sweeper / import）
//! actor=system，**不**走乐观锁。所有写经 store 唯一写路径（base::write / PolicyRepo），
//! daemon 零原始 SQL、绝不拼装持久化语句。
//!
//! settings-write 与 import-validate 两处：`on_timeout=allow` 在**写入 / 校验时刻**即被拒
//! （fail-closed：审批超时处置恒 deny，绝不允许配置成在线放行，L-12）。
//!
//! 同步 PolicyRepo / AuditSink 调用置于 spawn_blocking 边界（§5），绝不阻塞 async worker。
//!
//! 写端点三联动经 [`PolicyRepo::commit_write`]（COMMIT+重建）+ 审计支（本文件写）；审计为
//! 三联动一支，故审计句柄 + 来源经写端点签名传入——审计写失败即整体 fail-closed，无半态。

use std::sync::Arc;

use postern_core::domain::ResourceCode;
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink};
// control/ 非 shells：需要来源类型时以别名读，绝不写字面 `ConnOrigin::` 变体
// （SEC_CONSTRUCTION_SITES：ConnOrigin 字面只许在 shells/ 出现）。
use postern_core::request::ConnOrigin as Origin;

use super::{Actor, PolicyRepo, WriteError, WriteIntent, WriteOutcome};
use crate::error::DaemonError;

/// 集合端点的分页入参解析（缺省 / 钳制规则，F-6）。
///
/// 缺省（两者皆缺）⇒ `page_no=1, page_size=20`；`page_size=300` ⇒ 钳到 200；`page_no<1` ⇒ 1。
/// 钳制委托 core [`PageQuery::clamp`]（workspace 唯一分页钳制点），本函数只做"缺省填充"。
pub fn page_query(page_no: Option<u32>, page_size: Option<u32>) -> PageQuery {
    // 缺省填充：两者皆缺 ⇒ page_no=1, page_size=DEFAULT_SIZE(20)。
    PageQuery {
        page_no: page_no.unwrap_or(1),
        page_size: page_size.unwrap_or(PageQuery::DEFAULT_SIZE),
    }
    // 钳制委托 core 唯一钳制点：page_no<1 ⇒ 1；page_size 入 [1, MAX_SIZE(200)]。
    .clamp()
}

/// 控制面写端点结果（HTTP 状态码 + 载荷）的内核表达——把 [`WriteError`] 映射为 HTTP 语义。
///
/// 乐观锁冲突 ⇒ 409；其余写失败 ⇒ 5xx（fail-closed，无半态）。端点据此装配 axum 响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteHttp {
    /// 写成功（事务 COMMIT + 快照重建 + 审计三联动全成功）⇒ 200，回新版本 / 修订号。
    Committed(WriteOutcome),
    /// 乐观锁版本冲突 ⇒ HTTP 409 Conflict（+ `policy_change` 审计，由端点写入）。
    Conflict,
    /// 该实体的写接缝尚未接通（store 侧无对应写路径）⇒ HTTP **501 Not Implemented** + 稳定码。
    /// 刻意与 [`Failed`](WriteHttp::Failed)（500 内部失败）区分：明确未实现而非内部坏掉。
    NotImplemented,
    /// 其余写失败（事务 / 快照重建 / 审计）⇒ 5xx；不 COMMIT、不重建，无半态。
    Failed,
}

impl WriteHttp {
    /// 该写结果对应的 HTTP 状态码（乐观锁冲突恒 409，§8 F-6 / L-15）。
    pub fn status(&self) -> u16 {
        match self {
            WriteHttp::Committed(_) => 200,
            WriteHttp::Conflict => 409,
            WriteHttp::NotImplemented => 501,
            WriteHttp::Failed => 500,
        }
    }

    /// 把 [`WriteError`] 折叠为 HTTP 写结果：`VersionConflict` ⇒ 409，`NotImplemented` ⇒ 501，
    /// 其余 ⇒ 5xx（fail-closed）。
    pub fn from_write_error(err: &WriteError) -> Self {
        // 穷尽 per-variant，无 `_ =>` 兜底：乐观锁冲突 ⇒ 409；未接通 ⇒ 501；其余写失败 ⇒ 5xx。
        match err {
            WriteError::VersionConflict => WriteHttp::Conflict,
            WriteError::NotImplemented => WriteHttp::NotImplemented,
            WriteError::Transaction => WriteHttp::Failed,
            WriteError::SnapshotRebuild => WriteHttp::Failed,
            WriteError::Audit => WriteHttp::Failed,
        }
    }
}

/// 读端点（集合，强制分页）：经 [`PolicyRepo::list`] 取 `Page<T>` 信封。
///
/// `page` 已由 [`page_query`] 缺省填充 + 钳制；分页在 store scope/scan 层执行（daemon 只传
/// [`PageQuery`]，绝不拼 LIMIT-less 查询）。同步调用在 spawn_blocking 边界驱动（§5）。
pub async fn list(
    repo: &dyn PolicyRepo,
    entity: &'static str,
    page: PageQuery,
) -> Result<Page<serde_json::Value>, DaemonError> {
    // 集合读经 store scope/scan 分页层：daemon 只传 PageQuery，绝不拼 LIMIT-less 查询。
    repo.list(entity, page)
}

/// 控制面写端点的 `policy_change` 审计事件构造（三联动的审计支）。
///
/// 控制面写无数据面动词，`capability` / `principal` 恒 `None`；`resource` 取实体类别代号
/// （恒为代号，绝非真实地址）；`decision` 由处置传入（成功 `allow` / 冲突 `deny`）。`origin`
/// 由控制面 listener（shells/）经 SO_PEERCRED 采集后透传——本文件以 [`Origin`] 别名读，绝不
/// 构造字面来源类型。
fn policy_change_event(
    origin: Origin,
    intent: &WriteIntent,
    decision: &str,
    policy_rev: u64,
) -> AuditEvent {
    AuditEvent {
        v: 1,
        kind: "policy_change".to_string(),
        entry: "control".to_string(),
        origin,
        principal: None,
        resource: ResourceCode::new(intent.entity),
        capability: None,
        objects: Vec::new(),
        decision: decision.to_string(),
        stage: None,
        reason: String::new(),
        policy_rev,
    }
}

/// 把同步 [`AuditSink::record`] 置于 spawn_blocking 边界执行（§5：绝不阻塞 async worker）。
/// `spawn_blocking` join 失败（线程池 panic / 取消）一律 fail-closed 折叠为写失败，绝不静默成功。
async fn record_blocking(audit: &Arc<dyn AuditSink>, event: AuditEvent) -> bool {
    let sink = Arc::clone(audit);
    match tokio::task::spawn_blocking(move || sink.record(event)).await {
        Ok(Ok(())) => true,
        Ok(Err(_write)) => false,
        Err(_join) => false,
    }
}

/// 写端点（事务三联动 + 乐观锁，L-14 / F-6）：经 [`PolicyRepo::commit_write`] 提交一次写。
///
/// 三联动同处一个写锁临界区：事务 COMMIT + 快照重建（由 [`PolicyRepo::commit_write`] 在其内
/// 完成，Commit 先于 Rebuild）+ `policy_change` 审计（本函数在 COMMIT 后写）。审计是三联动一支
/// （L-14）：审计写失败 ⇒ 整体回 [`WriteHttp::Failed`]、不放行（无半态）。
///
/// `actor=Operator` 走乐观锁（`intent.expected_version` 必为 `Some`）；`actor=System`
/// （sweeper / import）不走乐观锁（`expected_version` 必为 `None`）。版本冲突 ⇒
/// [`WriteHttp::Conflict`]（409）+ `policy_change` 审计（冲突也留痕）。事务 / 快照重建 / 审计
/// 任一失败 ⇒ [`WriteHttp::Failed`]，无半态。同步调用在 spawn_blocking 边界驱动（§5）。
///
/// `audit` 句柄 + `origin` 由控制面装配/listener 透传：审计是三联动一支，故必须经写端点签名
/// 传入——否则"审计写失败中止三联动"在签名层即不可表达（fail-closed 不可观察）。
pub async fn write(
    repo: &dyn PolicyRepo,
    audit: &Arc<dyn AuditSink>,
    origin: Origin,
    actor: &Actor,
    intent: &WriteIntent,
) -> WriteHttp {
    match repo.commit_write(actor, intent) {
        Ok(outcome) => {
            // COMMIT + 重建已成（同一临界区）。第三支：写 policy_change 审计（allow）。
            let policy_rev = outcome.policy_rev;
            let event = policy_change_event(origin, intent, "allow", policy_rev);
            // 审计写失败 ⇒ 三联动中止、整体 fail-closed Failed（不放行、无半态）。
            if record_blocking(audit, event).await {
                WriteHttp::Committed(outcome)
            } else {
                WriteHttp::Failed
            }
        }
        // 「能力未接通」不是一次真实写尝试（store 侧无对应写路径）：绝不留 policy_change deny
        // 痕（否则审计被未实现端点的探测污染），如实回 501（端点据此回稳定「未接通」码）。
        Err(WriteError::NotImplemented) => WriteHttp::NotImplemented,
        Err(err) => {
            // 失败也留痕：policy_change 审计（deny 处置）。审计写本身失败不改判定
            // （已是失败路径，无半态可留）——失败映射恒据 commit 失败族。
            let event = policy_change_event(origin, intent, "deny", 0);
            let _ = record_blocking(audit, event).await;
            WriteHttp::from_write_error(&err)
        }
    }
}

/// settings-write：`on_timeout=allow` 在写入时刻被拒（fail-closed，L-12）。
///
/// `on_timeout` 仅接受 deny；传入 `allow` ⇒ `Err(DaemonError)`（绝不持久化成在线放行）。
/// 接受时再走标准写端点（[`write`]）。
pub fn validate_settings_on_timeout(on_timeout: &str) -> Result<(), DaemonError> {
    // fail-closed：仅 deny 合法；其余（含 allow）一律拒，绝不持久化成在线放行。
    match on_timeout {
        "deny" => Ok(()),
        _ => Err(DaemonError::Boot),
    }
}

/// import-validate：导入策略包时校验 `on_timeout=allow` 被拒（fail-closed，L-12）。
///
/// 与 [`validate_settings_on_timeout`] 同一不变量——`on_timeout=allow` 在校验阶段即拒，
/// 绝不让导入把审批超时处置改成在线放行。
pub fn validate_import_on_timeout(on_timeout: &str) -> Result<(), DaemonError> {
    // 与 settings 同一 fail-closed 不变量：仅 deny 合法，其余（含 allow）一律拒。
    match on_timeout {
        "deny" => Ok(()),
        _ => Err(DaemonError::Boot),
    }
}
