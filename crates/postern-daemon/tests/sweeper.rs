//! sweeper 单元行为测试（RED）。
//!
//! 钉死系统自动机子域（模块文档 06 §3.6 sweeper、§8 F-8 / L-11 / L-15、§6.2 PolicyRepo
//! system writes、02 base::write 的系统协调写 / `Actor::System`（store `SYSTEM_ACTOR='system'`））。
//!
//! 驱动方式（06 §9 测试策略）：**内存 Fake 全插件注入** —— Fake `SweepRepo`（store
//! `PolicyRepo` 系统协调写 + 同临界区快照重建的注入缝）+ Fake `AuditSink`（按序留痕）。
//! 每条只钉一个行为，断言「给定一拍 → 回收/审计/快照重建恰为某可观察结果」。失败路径一等
//! 公民：注入清扫缝失败触发 fail-closed 分支再观察（不留痕、不放半截状态）。
//!
//! §8 逐条覆盖（加 // §8 注释）：
//! - F-8：一条 `expires_at < now` 的 `temp_grants` 经一拍 → `ended_at` 非空且
//!   `end_reason=='expired'`、四类到期项均经事务回收、`created_by==updated_by=='system'`、
//!   对应落 `policy_change`/`mode_change` 审计、快照在**同一写锁临界区**重建（重建后不含该过期项）。
//! - L-15：sweeper 写经与人写**同一** `PolicyRepo` 事务路径、`actor=system`（写路径集中化）、
//!   **不走乐观锁**（幂等谓词驱动）。
//! - L-11（交叉校验）：sweeper 时序**非承重**——过期安全归 kernel/evaluate 求值时刻墙钟，
//!   本单元只验 housekeeping（可见性回收 + 留痕），不在 sweeper 放任何过期 SAFETY。
//!
//! 雷区纪律：本文件**零 SQL 标记**；不构造 `ConnOrigin`/机密字面（本子域无此需要）；异步用
//! `#[tokio::test]`。实现为 RED 桩（`Sweeper::tick`/`run` 体为 `todo!()`），故驱动一拍即
//! panic → 观察到红；纯类型层断言（`ExpiryClass::ALL`/`audit_kind`、`SweepReport::recycled_of`）
//! 先于实现成立则单独标注，验编排正确。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use postern_core::domain::Timestamp;
use postern_core::error::AuditError;
use postern_core::plugin::{AuditEvent, AuditSink};

use postern_store::base::write::{Actor, SYSTEM_ACTOR};

use postern_daemon::sweeper::{ExpiryClass, SweepRepo, SweepReport, Sweeper, END_REASON_EXPIRED};

// ════════════════════════════════════════════════════════════════════════════
//  共享探针：清扫缝调用序 / 审计事件序
// ════════════════════════════════════════════════════════════════════════════

/// 各触达点按序追加的标记，供「调用序」断言比对（sweep→record 序、回收先于留痕等）。
#[derive(Default)]
struct CallLog {
    events: Mutex<Vec<&'static str>>,
}

impl CallLog {
    fn record(&self, tag: &'static str) {
        self.events.lock().expect("call log not poisoned").push(tag);
    }

    fn snapshot(&self) -> Vec<&'static str> {
        self.events.lock().expect("call log not poisoned").clone()
    }
}

/// 固定墙钟（确定性：谓词用入参 now，不读系统钟）。
fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_700_000_000_000)
}

// ════════════════════════════════════════════════════════════════════════════
//  Fake SweepRepo（清扫缝）：注入「一拍成功→某 SweepReport」或「失败→Err」
// ════════════════════════════════════════════════════════════════════════════

/// 清扫缝 Fake：记录 sweep 触达 + 命中的 `now`，按注入语义返回成功报告或失败。
///
/// 成功报告刻意以 `Actor::System` + 四类全回收 + `end_reason='expired'` + 同临界区重建
/// 组装——使「实现把 actor/某类/快照重建漏掉」时对应断言跑红，而非靠桩 panic 兜底。
struct FakeSweepRepo {
    report: Mutex<Option<Result<SweepReport, ()>>>,
    seen_now: Mutex<Option<Timestamp>>,
    log: Arc<CallLog>,
}

impl FakeSweepRepo {
    /// 一拍成功：四类全回收（各 1 行）、临时格终态 `expired`、快照同临界区重建。
    fn ok(log: Arc<CallLog>) -> Self {
        let report = SweepReport {
            actor: Actor::System,
            recycled: ExpiryClass::ALL.iter().map(|c| (*c, 1usize)).collect(),
            temp_grant_end_reason: END_REASON_EXPIRED,
            snapshot_rebuilt_in_critical_section: true,
            rebuilt_policy_rev: 8,
        };
        Self {
            report: Mutex::new(Some(Ok(report))),
            seen_now: Mutex::new(None),
            log,
        }
    }

    /// 显式报告（覆写四类回收/快照重建/actor 以钉单一行为）。
    fn with_report(report: SweepReport, log: Arc<CallLog>) -> Self {
        Self {
            report: Mutex::new(Some(Ok(report))),
            seen_now: Mutex::new(None),
            log,
        }
    }

    /// 一拍失败：事务/重建失败（fail-closed：不留痕、不放半截状态）。
    fn failing(log: Arc<CallLog>) -> Self {
        Self {
            report: Mutex::new(Some(Err(()))),
            seen_now: Mutex::new(None),
            log,
        }
    }
}

impl SweepRepo for FakeSweepRepo {
    fn sweep_expired<'a>(
        &'a self,
        now: Timestamp,
    ) -> Pin<Box<dyn Future<Output = postern_daemon::error::Result<SweepReport>> + Send + 'a>> {
        self.log.record("sweep");
        *self.seen_now.lock().expect("now slot ok") = Some(now);
        let outcome = self
            .report
            .lock()
            .expect("report slot not poisoned")
            .take()
            .expect("sweep should be exercised at most once per tick");
        Box::pin(async move {
            // 失败映射为 daemon 装配层错误（fail-closed）；成功交还报告。
            outcome.map_err(|()| postern_daemon::error::DaemonError::Pool)
        })
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Fake AuditSink：把每条 record 的 (kind, decision) 按序留痕；可注入第 N 次写失败
// ════════════════════════════════════════════════════════════════════════════

/// 审计汇 Fake：留每条 record 的 (kind, decision, policy_rev)（沿用核心 AuditEvent 形状）。
/// 捕获 `policy_rev` 以便钉死「tick 把同临界区重建后的修订号透传进留痕」（非 Fake 自读自证）。
struct FakeAudit {
    events: Mutex<Vec<(String, String)>>,
    revs: Mutex<Vec<u64>>,
    fail_on_call: Option<usize>,
    calls: AtomicUsize,
    log: Arc<CallLog>,
}

impl FakeAudit {
    fn ok(log: Arc<CallLog>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            revs: Mutex::new(Vec::new()),
            fail_on_call: None,
            calls: AtomicUsize::new(0),
            log,
        }
    }

    /// 第 `nth` 次 record（1-based）返回写失败，其余成功。
    fn fail_nth(nth: usize, log: Arc<CallLog>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            revs: Mutex::new(Vec::new()),
            fail_on_call: Some(nth),
            calls: AtomicUsize::new(0),
            log,
        }
    }

    fn recorded(&self) -> Vec<(String, String)> {
        self.events.lock().expect("audit log ok").clone()
    }

    /// 每条留痕承载的 `policy_rev`（钉「tick 把重建后修订号透传进审计」）。
    fn revs(&self) -> Vec<u64> {
        self.revs.lock().expect("audit rev log ok").clone()
    }

    fn kinds(&self) -> Vec<String> {
        self.recorded().into_iter().map(|(k, _d)| k).collect()
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AuditSink for FakeAudit {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.log.record("record");
        self.events
            .lock()
            .expect("audit log ok")
            .push((event.kind.clone(), event.decision.clone()));
        self.revs
            .lock()
            .expect("audit rev log ok")
            .push(event.policy_rev);
        match self.fail_on_call {
            Some(target) if target == n => Err(AuditError::WriteFailed),
            _ => Ok(()),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  夹具组装：清扫缝 + 审计汇 + 共享探针
// ════════════════════════════════════════════════════════════════════════════

/// 一束注入件 + 探针，持有以便测试在 tick 后读回 CallLog / 审计记录。
struct Harness {
    sweeper: Sweeper,
    log: Arc<CallLog>,
    repo: Arc<FakeSweepRepo>,
    audit: Arc<FakeAudit>,
}

/// 组装一个 sweeper：清扫缝与审计汇的成功/失败语义由调用方先构造好传入。
fn harness(repo: FakeSweepRepo, audit: FakeAudit, log: Arc<CallLog>) -> Harness {
    let repo = Arc::new(repo);
    let audit = Arc::new(audit);
    let sweeper = Sweeper::new(
        repo.clone() as Arc<dyn SweepRepo>,
        audit.clone() as Arc<dyn AuditSink>,
    );
    Harness {
        sweeper,
        log,
        repo,
        audit,
    }
}

/// 默认「一拍全成功」工厂：四类全回收、终态 `expired`、快照同临界区重建、审计全 Ok。
fn passing_harness() -> Harness {
    let log = Arc::new(CallLog::default());
    harness(
        FakeSweepRepo::ok(log.clone()),
        FakeAudit::ok(log.clone()),
        log,
    )
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-8：一拍回收的承重终态 —— temp_grants ended_at 非空 + end_reason=='expired'
// ════════════════════════════════════════════════════════════════════════════

// §8 F-8：一条 expires_at < now 的 temp_grants 经一拍 → 其终态留痕 end_reason=='expired'
// （承重事实：ended_at 非空由 end_reason 终态成立背书）。一拍走清扫缝、回收报告承载该终态。
#[tokio::test]
async fn tick_recycles_temp_grant_with_end_reason_expired() {
    let h = passing_harness();
    let report = h
        .sweeper
        .tick(now())
        .await
        .expect("一拍成功应回一份回收报告");
    // §8 F-8：临时格回收的承重终态——end_reason 恒为 'expired'。
    assert_eq!(
        report.temp_grant_end_reason, END_REASON_EXPIRED,
        "temp_grants 过期回收的承重终态必为 end_reason=='expired'（F-8）"
    );
    assert_eq!(
        END_REASON_EXPIRED, "expired",
        "终态常量必为字面 'expired'（F-8 验收口径）"
    );
    // 该过期临时格确被回收（≥1 行）——回收即「可见性回收」，重建后快照不再含它。
    assert!(
        report.recycled_of(ExpiryClass::TempGrant) >= 1,
        "expires_at < now 的 temp_grants 必经一拍回收（F-8）"
    );
    // 一拍把回收**唯一**经清扫缝 SweepRepo::sweep_expired 这一道注入缝路由，且恰一次——这是
    // daemon 这层能行为观察到的写路径承诺：sweeper 不另起旁路、不在本 crate 拼装持久化语句，
    // 整拍回收收敛到单一缝（与控制面人写**汇于同一** PolicyRepo 事务路径这一**跨路同一性**由
    // 静态契约 DB_WRITE_PATH_CENTRALIZED 绿背书 + 写路径调用集人工审查，见 L-15【行为观察+人工】；
    // 本行为测试不僭称证明跨路同一，只钉 daemon 侧「回收唯一收敛到清扫缝、恰一次」）。
    assert_eq!(
        h.log.snapshot().iter().filter(|t| **t == "sweep").count(),
        1,
        "一拍回收必唯一收敛到清扫缝 SweepRepo::sweep_expired 且恰一次（daemon 侧写路径收敛，L-15）"
    );
    // 落库写入者恒为系统（System），区别于人写操作者标识——L-15 的 actor 维度（行为可观察）。
    assert_eq!(
        report.actor,
        Actor::System,
        "sweeper 回收落库 actor 必为 Actor::System（L-15：系统自动机写）"
    );
    assert_eq!(
        *h.repo.seen_now.lock().expect("now slot ok"),
        Some(now()),
        "清扫缝谓词必用入参 now（确定性：不读系统钟）"
    );
}

// §8 F-8：四类到期项（temp_grants / credentials / mode_state / 审批超时）均经**同一事务**回收。
#[tokio::test]
async fn tick_recycles_all_four_expiry_classes_in_one_transaction() {
    let h = passing_harness();
    let report = h.sweeper.tick(now()).await.expect("一拍成功");
    for class in ExpiryClass::ALL {
        assert!(
            report.recycled_of(class) >= 1,
            "四类到期项均须经同一事务回收（F-8）：{class:?} 未被回收"
        );
    }
    // 一拍只走**一次**清扫缝（四类在同一事务内回收，而非四次独立事务）。
    assert_eq!(
        h.log.snapshot().iter().filter(|t| **t == "sweep").count(),
        1,
        "四类回收在**同一**事务内（一拍仅一次 sweep_expired，F-8/L-15：单事务）"
    );
}

// §8 F-8 / L-15：回收落库写入者恒为 Actor::System —— created_by==updated_by=='system'。
#[tokio::test]
async fn tick_writes_as_system_actor() {
    let h = passing_harness();
    let report = h.sweeper.tick(now()).await.expect("一拍成功");
    // §8 L-15：系统自动机落库 actor 恒为 System（区别于人写的操作者标识）。
    assert_eq!(
        report.actor,
        Actor::System,
        "sweeper 回收落库 actor 必为 Actor::System（L-15）"
    );
    // created_by==updated_by=='system'：Actor::System 的落库标识恰为 store SYSTEM_ACTOR。
    assert_eq!(
        report.actor.label(),
        SYSTEM_ACTOR,
        "Actor::System 落 created_by/updated_by 必为 store SYSTEM_ACTOR='system'（F-8/L-15）"
    );
    assert_eq!(
        SYSTEM_ACTOR, "system",
        "store SYSTEM_ACTOR 字面必为 'system'"
    );
}

// §8 F-8：回收后快照须在**同一写锁临界区**内被重建。本测试钉死的是 **tick 对该不变量的
// 主动负责** + **重建修订号的真实透传**，而非透传清扫缝自报的布尔常量：
//  (a) tick 接受「自称同临界区重建」的成功报告并产出留痕（正向门，与 (b) 的拒绝门对偶）；
//  (b) tick 把**报告承载的 rebuilt_policy_rev** 原样透传进每条审计的 policy_rev（对账锚点
//      在出口可见，非 Fake 自读自证——故意用区别于默认的 rev 值，证明确为该值被线穿出去）。
// 注：「重建后快照不再含过期项」属快照内容事实，归 store PolicyRepo 系统写实现侧的集成验收
// （F-8【行为观察】场景 06 §4.1 A）；daemon 这层注入缝下无真实快照内容可断言，本测试不僭称。
#[tokio::test]
async fn tick_rebuilds_snapshot_in_same_critical_section() {
    let log = Arc::new(CallLog::default());
    // 区别于默认 rev=8 的特征值：若 tick 不透传报告里的 rev，propagation 断言即跑红。
    let rebuilt_rev = 4242u64;
    let report = SweepReport {
        actor: Actor::System,
        recycled: ExpiryClass::ALL.iter().map(|c| (*c, 1usize)).collect(),
        temp_grant_end_reason: END_REASON_EXPIRED,
        snapshot_rebuilt_in_critical_section: true,
        rebuilt_policy_rev: rebuilt_rev,
    };
    let h = harness(
        FakeSweepRepo::with_report(report, log.clone()),
        FakeAudit::ok(log.clone()),
        log,
    );
    let report = h.sweeper.tick(now()).await.expect("一拍成功");
    // §8 F-8：tick 接受同临界区重建的成功报告（与 sweep_denies_when_snapshot_not_rebuilt
    // 的拒绝门对偶——后者证明 tick 在该字段为 false 时 fail-closed，二者合钉「tick 据此字段裁决」）。
    assert!(
        report.snapshot_rebuilt_in_critical_section,
        "回收 COMMIT 后须在**同一写锁临界区**内重建快照（F-8/§3.6）"
    );
    // tick 把同临界区重建后的 policy_rev **透传**进每条留痕（对账锚点在审计出口可见）。
    // 非 Fake 自读自证：断言审计里出现的恰是报告承载的特征 rev，证明该值确被 tick 线穿出去。
    let revs = h.audit.revs();
    assert!(
        !revs.is_empty(),
        "一拍回收须落留痕（policy_rev 透传的载体，F-8）"
    );
    assert!(
        revs.iter().all(|r| *r == rebuilt_rev),
        "每条审计 policy_rev 须为重建后修订号 {rebuilt_rev}（tick 据 report.rebuilt_policy_rev 透传，对账锚点）；实测 revs={revs:?}"
    );
}

// §8 F-8 / L-14（三联动完整性）：清扫缝返回「自称回收成功」却**未在同一写锁临界区重建快照**
// （snapshot_rebuilt_in_critical_section==false）的报告 → tick **绝不**透传成功、绝不留痕，
// fail-closed 折叠为 Err。回收已写却未重建可见快照即半截状态（L-14：事务+快照重建+审计三联动
// 任一缺失整体失败）。本测试钉死 tick 对该不变量的**主动负责**——证明上面正向门读到的 true
// 是 tick 据以裁决的真实判据，而非被动透传的恒真布尔。先跑红（实现未校验该字段时透传成功）。
#[tokio::test]
async fn sweep_denies_when_snapshot_not_rebuilt() {
    let log = Arc::new(CallLog::default());
    // 四类均「回收」，但快照未同临界区重建——三联动残缺。
    let half_done = SweepReport {
        actor: Actor::System,
        recycled: ExpiryClass::ALL.iter().map(|c| (*c, 1usize)).collect(),
        temp_grant_end_reason: END_REASON_EXPIRED,
        snapshot_rebuilt_in_critical_section: false,
        rebuilt_policy_rev: 0,
    };
    let h = harness(
        FakeSweepRepo::with_report(half_done, log.clone()),
        FakeAudit::ok(log.clone()),
        log,
    );
    let out = h.sweeper.tick(now()).await;
    assert!(
        out.is_err(),
        "未同临界区重建快照的一拍 → 三联动残缺，tick 必 fail-closed 上抛 Err（L-14/F-8）"
    );
    // 半截状态绝不留痕：未重建即不落 policy_change/mode_change（不放半截、不机械齐发）。
    assert_eq!(
        h.audit.call_count(),
        0,
        "快照未重建的一拍绝不落审计留痕（三联动残缺整体失败，不留半截痕，L-14）"
    );
    assert!(
        !h.log.snapshot().contains(&"record"),
        "快照未重建的一拍绝不触达审计写（不放半截状态）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 F-8：回收后落 policy_change / mode_change 审计（与人写同形态、actor=system）
// ════════════════════════════════════════════════════════════════════════════

// §8 F-8 / L-15：一拍回收后落审计 —— temp_grants/credentials/审批 → policy_change，
// mode_state → mode_change。审计在回收 COMMIT **之后**（留痕打扫语义：先回收再留痕）。
#[tokio::test]
async fn tick_emits_policy_change_and_mode_change_audit() {
    let h = passing_harness();
    let _report = h.sweeper.tick(now()).await.expect("一拍成功");

    let kinds = h.audit.kinds();
    // §8 F-8：四类回收对应落 policy_change/mode_change 审计——两类 kind 都须出现。
    assert!(
        kinds.iter().any(|k| k == "policy_change"),
        "temp_grants/credentials/审批超时回收须落 policy_change 审计（F-8）；实测 kinds={kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k == "mode_change"),
        "mode_state 回收须落 mode_change 审计（F-8）；实测 kinds={kinds:?}"
    );
    // 留痕打扫语义：审计写在回收（sweep）**之后**（先回收 COMMIT，再落留痕）。
    let seq = h.log.snapshot();
    let pos_sweep = seq.iter().position(|t| *t == "sweep").expect("sweep 触达");
    let pos_first_record = seq
        .iter()
        .position(|t| *t == "record")
        .expect("回收后须落审计留痕");
    assert!(
        pos_sweep < pos_first_record,
        "审计留痕必在回收 COMMIT 之后（F-8/§3.6：先回收再留痕）"
    );
    // 至少落了两条留痕（policy_change + mode_change 各一），审计被真实触达。
    assert!(
        h.audit.call_count() >= 2,
        "一拍回收四类须落 policy_change + mode_change 至少两条审计留痕（F-8）"
    );
}

// §8 F-8 / L-15：sweeper 审计与人写**同形态**——回收落的 policy_change/mode_change 是
// 「策略变更」类痕（decision 非 deny/escalate_denied），区别于数据面请求事件。
#[tokio::test]
async fn tick_audit_is_policy_change_shape_not_request_event() {
    let h = passing_harness();
    let _ = h.sweeper.tick(now()).await.expect("一拍成功");
    let recorded = h.audit.recorded();
    assert!(!recorded.is_empty(), "一拍回收须落策略变更类审计（F-8）");
    for (kind, _decision) in &recorded {
        assert!(
            kind == "policy_change" || kind == "mode_change",
            "sweeper 回收审计 kind 必为 policy_change/mode_change（与人写同形态，L-15）；实测 kind={kind}"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 失败路径一等公民：清扫缝失败 → 不留痕、不放半截状态（fail-closed，L-14）
// ════════════════════════════════════════════════════════════════════════════

// §8 L-14：清扫缝（事务/重建）失败 → 整体失败，**绝不**落 policy_change/mode_change 审计
// （写=事务+快照重建+审计三联动，任一失败整体失败：不 COMMIT、不重建、不留痕）。
#[tokio::test]
async fn sweep_failure_denies_tick_and_emits_no_audit() {
    let log = Arc::new(CallLog::default());
    let h = harness(
        FakeSweepRepo::failing(log.clone()),
        FakeAudit::ok(log.clone()),
        log,
    );
    let out = h.sweeper.tick(now()).await;
    assert!(
        out.is_err(),
        "清扫缝（事务/重建）失败 → 一拍整体失败（fail-closed，L-14）"
    );
    // 三联动：回收失败即不留痕——绝不落策略变更审计（不放半截状态）。
    assert_eq!(
        h.audit.call_count(),
        0,
        "回收失败后绝不落 policy_change/mode_change 审计（L-14：任一失败整体失败，不留半截痕）"
    );
    assert!(
        !h.log.snapshot().contains(&"record"),
        "回收失败的一拍绝不触达审计写（不留痕、不放半截状态）"
    );
}

// §8 F-8（部分类缺失即不过）：某一类到期项未被回收（如 mode_state 漏回收）→ 该类审计缺失，
// 验收按 F-8「任一项不满足即不过」——本测试钉死「mode_state 未回收 ⇒ 无 mode_change 痕」。
// 这是回归护栏：若实现无论是否回收都机械落两类审计，本断言失败（审计须如实反映回收事实）。
#[tokio::test]
async fn missing_mode_state_recycle_yields_no_mode_change_audit() {
    let log = Arc::new(CallLog::default());
    // 只回收三类（漏 mode_state），其余 happy。
    let partial = SweepReport {
        actor: Actor::System,
        recycled: vec![
            (ExpiryClass::TempGrant, 1),
            (ExpiryClass::Credential, 1),
            (ExpiryClass::ApprovalTimeout, 1),
        ],
        temp_grant_end_reason: END_REASON_EXPIRED,
        snapshot_rebuilt_in_critical_section: true,
        rebuilt_policy_rev: 8,
    };
    let h = harness(
        FakeSweepRepo::with_report(partial, log.clone()),
        FakeAudit::ok(log.clone()),
        log,
    );
    let report = h.sweeper.tick(now()).await.expect("一拍成功（部分回收）");
    assert_eq!(
        report.recycled_of(ExpiryClass::ModeState),
        0,
        "本拍 mode_state 未回收（构造前提）"
    );
    // mode_state 无回收 ⇒ 不应落 mode_change 痕（审计如实反映回收，不机械齐发）。
    assert!(
        !h.audit.kinds().iter().any(|k| k == "mode_change"),
        "mode_state 未回收时不得落 mode_change 审计（审计须如实反映回收事实，F-8）"
    );
    // policy_change 仍应落（三类已回收）。
    assert!(
        h.audit.kinds().iter().any(|k| k == "policy_change"),
        "已回收的三类（temp_grants/credentials/审批）须落 policy_change（F-8）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-3 / 审计写失败：回收已 COMMIT 后审计写失败的处置（留痕降级，绝不回滚已回收）
// ════════════════════════════════════════════════════════════════════════════

// §8（留痕失败处置）：回收已 COMMIT、快照已重建后，policy_change 审计写失败 →
// 一拍如实上抛失败（fail-closed：不可留痕的回收须可被运维感知），但**绝不**回滚已 COMMIT
// 的回收（已生效不可撤），区别于「sweep 前失败」（那是整体失败）。本测试钉死：审计写被真实
// 触达（call_count≥1）且一拍据审计写结果上抛 Err（不静默吞错当成功）。
#[tokio::test]
async fn audit_write_failure_after_recycle_is_surfaced_not_swallowed() {
    let log = Arc::new(CallLog::default());
    let h = harness(
        FakeSweepRepo::ok(log.clone()),
        FakeAudit::fail_nth(1, log.clone()), // 第 1 条留痕写失败
        log,
    );
    let out = h.sweeper.tick(now()).await;
    // 回收确已发生（sweep 在审计之前），审计写被真实触达。
    assert!(
        h.log.snapshot().contains(&"sweep"),
        "审计失败处置前回收（sweep）确已发生"
    );
    assert!(
        h.audit.call_count() >= 1,
        "回收后须尝试落审计留痕（审计写被真实触达）"
    );
    // 留痕失败不静默吞错：一拍据审计结果上抛 Err（fail-closed，不可留痕须可感知）。
    assert!(
        out.is_err(),
        "回收后审计留痕写失败 → 一拍如实上抛失败（不静默吞错放行；L-3 留痕纪律）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  §8 L-11（交叉校验）：sweeper 时序非承重 —— 行为归 kernel/evaluate，本单元只验 housekeeping
// ════════════════════════════════════════════════════════════════════════════

// §8 L-11：sweeper 是 housekeeping（可见性回收 + 留痕），**不**承载过期 SAFETY。即便一拍被
// 延迟/丢拍，过期项在求值时刻已被墙钟二次校验判拒（行为归 kernel/evaluate）。本单元的承重锚
// 点：sweeper 的产出是「回收 + 重建 + 留痕」，而非任何「放行/拒绝」决策——回报告里无 Decision
// 字段、无 deny/allow 语义，证明 sweeper 不做安全判定（L-11 把判定与打扫解耦）。
#[tokio::test]
async fn sweeper_is_housekeeping_only_no_safety_decision() {
    let h = passing_harness();
    let report = h.sweeper.tick(now()).await.expect("一拍成功");
    // sweeper 的产出恰为「回收 + 重建 + 留痕」三件 housekeeping 事实，无任何 deny/allow 决策。
    // （SweepReport 的字段全是回收/重建摘要；审计 decision 是策略变更痕，不是请求放行/拒绝。）
    for (_kind, decision) in h.audit.recorded() {
        assert_ne!(
            decision, "deny",
            "sweeper 留痕不是请求拒绝决策（过期 SAFETY 归 kernel/evaluate，L-11）"
        );
        assert_ne!(
            decision, "escalate_denied",
            "sweeper 不做审批裁决式拒绝（housekeeping only，L-11）"
        );
    }
    // 回收量是 housekeeping 计数（可见性回收行数），不是安全判定——验其确为四类回收摘要。
    let total: usize = ExpiryClass::ALL
        .iter()
        .map(|c| report.recycled_of(*c))
        .sum();
    assert!(
        total >= ExpiryClass::ALL.len(),
        "一拍回收量是 housekeeping 计数（四类各≥1），非安全判定（L-11）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  类型层锚点（先于实现成立）：ExpiryClass 全集 / 审计 kind 映射 / 报告取值器
// ════════════════════════════════════════════════════════════════════════════

// §8 F-8：四类过期项全集恰为 {TempGrant, Credential, ModeState, ApprovalTimeout}，且 ModeState
// 落 mode_change、其余落 policy_change（审计 kind 映射的判别根；纯类型层，先于实现可绿）。
#[test]
fn expiry_classes_and_audit_kind_mapping() {
    assert_eq!(
        ExpiryClass::ALL.len(),
        4,
        "过期类全集恰四类（F-8：temp_grants/credentials/mode_state/审批超时）"
    );
    assert_eq!(ExpiryClass::TempGrant.audit_kind(), "policy_change");
    assert_eq!(ExpiryClass::Credential.audit_kind(), "policy_change");
    assert_eq!(ExpiryClass::ApprovalTimeout.audit_kind(), "policy_change");
    assert_eq!(
        ExpiryClass::ModeState.audit_kind(),
        "mode_change",
        "mode_state 回收落 mode_change（F-8：mode 变更与策略变更分类留痕）"
    );
}

// SweepReport::recycled_of 取值器形状锚点：命中类回其行数、缺类回 0（纯类型层，先于实现可绿）。
#[test]
fn sweep_report_recycled_of_lookup() {
    let report = SweepReport {
        actor: Actor::System,
        recycled: vec![(ExpiryClass::TempGrant, 3), (ExpiryClass::ModeState, 1)],
        temp_grant_end_reason: END_REASON_EXPIRED,
        snapshot_rebuilt_in_critical_section: true,
        rebuilt_policy_rev: 8,
    };
    assert_eq!(report.recycled_of(ExpiryClass::TempGrant), 3);
    assert_eq!(report.recycled_of(ExpiryClass::ModeState), 1);
    assert_eq!(
        report.recycled_of(ExpiryClass::Credential),
        0,
        "缺类回收数为 0（未回收该类）"
    );
    // 落库 actor 形状锚点：System 的标识恰为 store SYSTEM_ACTOR。
    assert_eq!(report.actor.label(), SYSTEM_ACTOR);
}
