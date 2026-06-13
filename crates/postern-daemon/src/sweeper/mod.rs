//! 系统自动机子域（模块文档 06 §8.9 / §8.10 / §3.6）。
//!
//! 一个 tokio 周期任务，actor=system：按墙钟周期回收过期授权并留痕，与人写策略共用
//! `PolicyRepo` 写锁。正确性不依赖其调度时序——过期判定的真相在求值时刻按墙钟二次校验
//! （L-11，行为归 kernel/evaluate），sweeper 只负责把已过期项从可见集合清出并留下系统
//! 痕迹（可见性回收 + 留痕打扫）。
//!
//! 每拍以 `actor=system` 走与人写**同一** `PolicyRepo` 事务路径（写路径集中化，L-15）：
//! 在一个事务里用谓词扫出 `expires_at < now` 的四类过期项、按表写终态
//! （`temp_grants` 写 `ended_at` + `end_reason='expired'`、`credentials`/`mode_state`
//! 按各自终态字段回收、审批超时项写裁决），COMMIT 后**在同一写锁临界区内重建快照**，
//! 落 `policy_change`/`mode_change` 审计。系统协调写是**幂等谓词驱动**（"凡过期则回收"），
//! 无"读后写"竞态，故**不走乐观锁**（这也是它能与人写共路而不冲突的原因，L-15）。
//!
//! 雷区纪律：
//! - 以 `Actor::System`（store `SYSTEM_ACTOR='system'`）落 `created_by`/`updated_by`；
//! - 与控制面人写共用同一写锁，绝不自持第二把锁、绝不死锁；
//! - 同步 `PolicyRepo` / `AuditSink` 调用置于 `spawn_blocking` 边界，绝不在 async worker
//!   直接阻塞；
//! - 本目录**零 SQL 标记**：谓词回收经 store 的系统协调写 / `PolicyRepo`，daemon 不直接
//!   拼装持久化语句（B-5）；
//! - 绝不构造 `ConnOrigin`/机密字面（本子域无此需要）；
//! - 过期 SAFETY **不在 sweeper**——过期安全在求值时刻墙钟，sweeper 只是 housekeeping（L-11）。
//!
//! 本波次为骨架：清扫缝 [`SweepRepo`]、一拍回收报告 [`SweepReport`]、周期任务入口与单拍
//! 协调 [`Sweeper::tick`] 的类型/签名对齐设计，函数体为 `todo!()`。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use postern_core::domain::{ResourceCode, Timestamp};
// 以别名读取连接来源类型：sweeper 不在 shells 构造字面 ConnOrigin 变体（契约
// SEC_CONSTRUCTION_SITES 仅许 shells 出现字面变体），需要时经别名读/填。
use postern_core::plugin::{AuditEvent, AuditSink};
use postern_core::request::ConnOrigin as Origin;
// 以别名读取系统写入者标识：sweeper 落库恒为系统写（created_by/updated_by='system'）。
use postern_store::base::write::Actor;

use crate::error::{DaemonError, Result};

/// 一拍清扫所回收的四类过期项（§3.6 / F-8）。
///
/// 每类对应一条按表写终态的系统协调写；四类是否齐发由 [`SweepReport`] 承载，验收口径要求
/// 四类到期项**均经同一事务回收**（F-8）。`ModeState` 回收落 `mode_change` 审计，其余三类
/// 落 `policy_change` 审计（§3.6 留痕分类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExpiryClass {
    /// 临时授权格：写 `ended_at`（非空）+ `end_reason='expired'`（F-8 承重终态）。
    TempGrant,
    /// 凭据到期：`credentials.expires_at < now` 的凭据按终态回收。
    Credential,
    /// 司法/模式态到期：`mode_state.expires_at < now` 回收（落 `mode_change`）。
    ModeState,
    /// 审批超时挂起项：超时即写裁决（`on_timeout` 固定 deny，L-12）。
    ApprovalTimeout,
}

impl ExpiryClass {
    /// 四类过期项的确定性全集（一拍须逐类经同一事务回收，F-8）。
    pub const ALL: [ExpiryClass; 4] = [
        ExpiryClass::TempGrant,
        ExpiryClass::Credential,
        ExpiryClass::ModeState,
        ExpiryClass::ApprovalTimeout,
    ];

    /// 该类回收对应的审计 `kind`：`ModeState` → `mode_change`，其余 → `policy_change`
    /// （§3.6 / F-8：对应落一条 `policy_change`/`mode_change` 审计）。
    pub fn audit_kind(&self) -> &'static str {
        match self {
            ExpiryClass::ModeState => "mode_change",
            _ => "policy_change",
        }
    }
}

/// 终态留痕字段：临时授权回收落库的终态（F-8 承重）。
///
/// `temp_grants` 过期回收的承重事实：`ended_at` 非空、`end_reason` 恒为 [`END_REASON_EXPIRED`]。
/// 该结构承载一拍里被回收的临时格写终态摘要，供测试逐字段比对（F-8）。
pub const END_REASON_EXPIRED: &str = "expired";

/// 系统回收留痕的 `decision` 词：housekeeping 类「策略变更」痕，**非**请求放行/拒绝决策
/// （L-11：sweeper 不做安全判定，故绝不是 `deny`/`escalate_denied`）。恒为 [`END_REASON_EXPIRED`]
/// 同义的「过期回收」语义词，标识本痕来自系统自动机的过期清扫。
const RECYCLE_DECISION: &str = END_REASON_EXPIRED;

/// 一拍清扫的可观察回收报告（§3.6 / F-8 / L-15）。
///
/// 由 [`SweepRepo::sweep_expired`] 在**同一写锁临界区**内产出：四类过期项各回收了多少行、
/// 临时格的终态留痕（`end_reason`）、落库 `actor`、快照是否在同一临界区被重建、重建后的
/// `policy_rev`。验收口径（F-8/L-15）由本报告逐字段钉死：`actor==Actor::System`（即
/// `created_by==updated_by=='system'`）、四类均回收、快照同临界区重建。
#[derive(Debug, Clone)]
pub struct SweepReport {
    /// 本拍回收落库的写入者：恒为 [`Actor::System`]（`created_by==updated_by=='system'`，L-15）。
    pub actor: Actor,
    /// 四类过期项各自被回收的行数（按 [`ExpiryClass`] 索引；F-8：四类均经事务回收）。
    pub recycled: Vec<(ExpiryClass, usize)>,
    /// 被回收临时格写入的 `end_reason`（恒为 [`END_REASON_EXPIRED`]；非空 `ended_at` 的承重痕）。
    pub temp_grant_end_reason: &'static str,
    /// 本拍是否在**同一写锁临界区**内重建了快照（F-8/§3.6：COMMIT 后同临界区重建）。
    pub snapshot_rebuilt_in_critical_section: bool,
    /// 重建后快照的策略修订号（对账锚点；重建后快照不再含该过期项，F-8）。
    pub rebuilt_policy_rev: u64,
}

impl SweepReport {
    /// 某一过期类在本拍回收的行数（缺该类 → 0）。
    pub fn recycled_of(&self, class: ExpiryClass) -> usize {
        self.recycled
            .iter()
            .find_map(|(c, n)| if *c == class { Some(*n) } else { None })
            .unwrap_or(0)
    }
}

/// 清扫缝（§3.6 / L-15）：sweeper 经此一拍走完「与人写共用同一 `PolicyRepo` 写锁的
/// 事务回收 + 同临界区快照重建」。
///
/// 这是 daemon 侧对 store `PolicyRepo` 系统协调写 + 快照重建的注入缝（镜像 kernel 的
/// `ConnAcquire`）：实现侧（store `PolicyRepo`）在**一个事务**里谓词扫出四类 `expires_at < now`
/// 过期项、按表写终态（系统协调写、幂等谓词驱动、**不走乐观锁**），COMMIT 后在**同一写锁
/// 临界区**重建快照，回一份 [`SweepReport`]。失败（事务/重建失败）→ `Err`，整体不 COMMIT、
/// 不重建快照（L-14：写=事务+快照重建+审计，任一失败整体失败）。
///
/// daemon 绝不在本缝里拼装持久化语句（零 SQL 标记，B-5）；落库 `actor` 由实现侧固定为
/// `Actor::System`。以手写 `BoxFuture` 返回（不依赖 `async-trait`，使本 src 缝保持 dyn 兼容、
/// 可经 `Arc<dyn SweepRepo>` 注入）；同步 store 调用由实现侧在 `spawn_blocking` 边界承接。
pub trait SweepRepo: Send + Sync {
    /// 一拍：以 `actor=system` 走同一写锁事务回收四类过期项 + 同临界区重建快照，回报告。
    ///
    /// `now` 是本拍墙钟读数（确定性：谓词用入参 `now`，不读系统钟）；过期谓词为
    /// `expires_at < now`。失败 → `Err`（不 COMMIT、不重建）。
    fn sweep_expired<'a>(
        &'a self,
        now: Timestamp,
    ) -> Pin<Box<dyn Future<Output = Result<SweepReport>> + Send + 'a>>;
}

/// 系统自动机：一个以 `actor=system` 运行的周期清扫任务（§3.6）。
///
/// 持清扫缝与审计汇的只读句柄；[`tick`](Sweeper::tick) 驱动一拍回收 + 留痕，[`run`](Sweeper::run)
/// 是 `tokio::time::interval` 周期循环。`now` 由注入的墙钟读取（测试可注入定值保确定性）。
/// **绝不**持 `PolicyRepo` 之外的第二把写锁；**绝不**进入数据面注入集合（红线 7.2-2）。
pub struct Sweeper {
    /// 清扫缝：一拍事务回收 + 同临界区快照重建（store `PolicyRepo` 系统写实现侧）。
    repo: Arc<dyn SweepRepo>,
    /// 审计汇：回收后落 `policy_change`/`mode_change` 留痕（与人写同形态）。
    audit: Arc<dyn AuditSink>,
}

impl Sweeper {
    /// 由注入的清扫缝与审计汇装配（boot/control 装配点交付，持系统写句柄）。
    pub fn new(repo: Arc<dyn SweepRepo>, audit: Arc<dyn AuditSink>) -> Self {
        Self { repo, audit }
    }

    /// 一拍清扫：走清扫缝完成事务回收 + 同临界区快照重建，再按回收的过期类落
    /// `policy_change`/`mode_change` 审计（与人写同形态、`actor=system`）。回本拍报告。
    ///
    /// 清扫缝失败 → `Err`（不留痕、不放行任何半截状态，fail-closed）。审计落痕在回收
    /// COMMIT 之后（留痕打扫语义）。正确性不依赖本拍时序——过期安全在求值时刻墙钟（L-11）。
    pub async fn tick(&self, now: Timestamp) -> Result<SweepReport> {
        // 走清扫缝：同一写锁事务谓词回收四类过期项 + COMMIT 后同临界区重建快照。
        // 失败（事务/重建失败）→ 整体失败：`?` 上抛，**绝不**落任何审计留痕、不放半截状态
        // （L-14：写=事务+快照重建+审计三联动，任一失败整体失败）。
        let report = self.repo.sweep_expired(now).await?;

        // 三联动完整性校验（L-14/F-8）：一拍报告自称回收成功，但若其未在**同一写锁临界区**
        // 重建快照（`snapshot_rebuilt_in_critical_section == false`），则「事务+快照重建+审计」
        // 三联动不成立——回收已写却未重建可见快照即**半截状态**，绝不放行、绝不留痕，fail-closed
        // 折叠为装配层 `Err`（tick 在此对该不变量负责，不被动透传清扫缝自报的成功）。
        if !report.snapshot_rebuilt_in_critical_section {
            return Err(DaemonError::Boot);
        }

        // 回收 COMMIT 之后才留痕（留痕打扫语义：先回收再留痕）。按实际被回收的过期类逐类落
        // 一条审计——审计须**如实反映回收事实**：某类本拍未回收（count==0）则不落该类痕。
        // ModeState → `mode_change`，其余三类 → `policy_change`（§3.6 留痕分类）。
        for class in ExpiryClass::ALL {
            if report.recycled_of(class) == 0 {
                continue;
            }
            let event = recycle_event(class, &report);
            // 审计写失败处置：回收已 COMMIT、快照已重建，已生效不可撤——但不可留痕的回收必须
            // 可被运维感知，故据审计结果如实上抛 Err（fail-closed，绝不静默吞错放行）。
            self.record_blocking(event).await?;
        }

        Ok(report)
    }

    /// 把同步 `AuditSink::record` 置于 `spawn_blocking` 边界执行（绝不在 async worker 直接
    /// 阻塞）；写失败或 join 失败一律 fail-closed 折叠为装配层 `Err`（不可留痕的回收须可感知）。
    async fn record_blocking(&self, event: AuditEvent) -> Result<()> {
        let sink = Arc::clone(&self.audit);
        let joined = tokio::task::spawn_blocking(move || sink.record(event)).await;
        match joined {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_write)) => Err(DaemonError::Boot),
            Err(_join) => Err(DaemonError::Boot),
        }
    }

    /// 周期清扫循环（`tokio::time::interval` 节拍）：每拍调 [`tick`](Sweeper::tick)。
    ///
    /// 时序非承重（L-11）：即便某拍延迟/丢拍，过期项在求值时刻已被墙钟二次校验判拒，安全
    /// 语义不破。单拍失败如实上抛由调用方处置（不静默吞错），但不因此放弃后续节拍。
    pub async fn run(&self, period: std::time::Duration) -> Result<()> {
        let mut ticker = tokio::time::interval(period);
        loop {
            ticker.tick().await;
            // 单拍失败如实上抛由调用方处置（不静默吞错放行）；时序非承重（L-11）：过期安全在
            // 求值时刻墙钟二次校验，故丢拍/延迟不破安全，但一拍内的失败仍 fail-closed 上抛。
            self.tick(now_wall()).await?;
        }
    }
}

/// 系统自动机本拍墙钟读数（确定性边界：`run` 周期循环每拍取一次系统钟；`tick` 入参化以便
/// 测试注入定值）。过期谓词为 `expires_at < now`。
fn now_wall() -> Timestamp {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Timestamp::from_unix_ms(ms)
}

/// 组装一条过期回收的系统审计痕（`actor=system`、策略变更类形态）。
///
/// `kind` 取该过期类的留痕分类（[`ExpiryClass::audit_kind`]：`ModeState`→`mode_change`、
/// 其余→`policy_change`）；`decision` 为 [`RECYCLE_DECISION`]（housekeeping 类，**非**请求
/// 放行/拒绝决策，L-11）；`policy_rev` 取同临界区重建后的修订号（对账锚点）。来源以系统本地
/// 占位（经 `Origin` 别名读取，绝不在本子域构造字面 `ConnOrigin` 变体）；本痕无资源/对象语义。
fn recycle_event(class: ExpiryClass, report: &SweepReport) -> AuditEvent {
    let origin: Origin = Origin::UnixPeer { uid: 0, gid: 0 };
    AuditEvent {
        v: 1,
        kind: class.audit_kind().to_string(),
        entry: "system".to_string(),
        origin,
        principal: None,
        resource: ResourceCode::new(""),
        capability: None,
        objects: Vec::new(),
        decision: RECYCLE_DECISION.to_string(),
        stage: None,
        reason: String::new(),
        policy_rev: report.rebuilt_policy_rev,
    }
}

/// 启动周期清扫任务（actor=system）的入口（占位）。
///
/// 装配 [`Sweeper`] 后驱动其 [`run`](Sweeper::run) 周期循环；boot 在 data.sock 开放后挂起本
/// 后台任务（时序非承重，L-11）。
pub async fn run() -> Result<()> {
    todo!("装配 Sweeper 并驱动周期 run（boot 装配点交付清扫缝 + 审计汇）")
}
