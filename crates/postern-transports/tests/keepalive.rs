//! 长连接保活状态机（keepalive 单元）行为测试（RED）。
//!
//! 被测对象：`postern_transports::keepalive::{Keepalive, KeepaliveBackend, KeepaliveOutcome,
//! Renewal, Heartbeat, Clock, FakeClock, Instant}`——绑定单条已建通路生命周期、以「活」
//! 为初态的后台保活状态机（§3.3）。两类手段经**抽象保活后端端口**（`KeepaliveBackend`）
//! 注入：①心跳（固定节律探测）；②协议级续约（`expiry − skew` 触发）。时间依赖收敛到
//! 可注入的 `Clock`（测试用 `FakeClock` 手推逻辑时间，确定可复现，§9），**绝不**用
//! `tokio::time::sleep` 真实墙钟跑。死活事实经 `HealthWriter` 单向写入健康视图。
//!
//! 覆盖 §8 条目（逐条加 `// §8` 注释，断言精确到具体值 / 变体）：
//! - F-2 续约确被发起：续约成功桩 + 推进到 `expiry − skew` → 续约请求次数 0→≥1，
//!   且此后健康仍 `Alive`。
//! - F-3 非长连接不保活：`persistent == false` 路径推进越过任意阈值 → 续约 / 心跳
//!   请求次数恒为 0。
//! - L-4 续约失败→死亡：续约一律失败桩 + 推进到阈值 → 续约失败后健康返回 `Dead`
//!   （不掩盖、不停留 `Alive`），桩后端新建连接尝试次数恒为 0（无自愈重连）。
//! - L-3 无重建边：`Dead` 后继续推进越过任意保活周期 → 健康持续 `Dead`（不翻回
//!   `Alive`），桩后端新建连接尝试次数恒为 0。
//! - 续约刷新 expiry：续约成功后下一次续约触发点基于桩后端返回的**新 expiry** 计算
//!   （推进到旧 expiry 不再触发、推进到新 `expiry − skew` 才触发），证明 expiry 来自
//!   远端而非本域常量。
//! - 构造签名审查（散文级，对齐 L-2/L-3）：本单元无禁用的重建 / 退避 / 重试类符号、
//!   无退避器——见文末 `forbidden_symbols_are_absent_from_unit` 文本级自检。
//!
//! 本单元不构造机密类型（不写 `ResolvedTarget` / `ResourceCredential` / `ConnOrigin`
//! 字面），不嵌裸数据库写标记，不依赖兄弟单元。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use postern_transports::health::{health_view, Health};
use postern_transports::keepalive::{
    FakeClock, Heartbeat, Instant, Keepalive, KeepaliveBackend, KeepaliveOutcome, Renewal,
};

// ── 记录次数的桩保活后端（§9：成功桩 / 失败桩，做行为观察）────────────────
//
// 端口**不接触机密**：方法不收 `ResolvedTarget` / `ResourceCredential`（§7-1/-8）。
// 桩共享 `Arc` 计数器，测试侧持一份句柄读计数、状态机持后端句柄发起动作。
//
// `connect_attempts` 钉死「无自愈、无重建边」的**可观察**判据：状态机里**根本没有**
// 新建连接 / 自愈入口（`KeepaliveBackend` 无此方法、`Keepalive` 无此路径，§7-3 / L-3），
// 故它对端口的唯一可达面是 `renew`/`heartbeat` 两个方法。自愈 / 重建回归在本端口上的
// **唯一可观察形态**就是「死亡判定既出、状态机仍继续敲后端去『复活』」——因此本计数器
// 在桩**已发出过死亡判定（续约失败 / 心跳僵死）之后**，再被状态机调用 `renew`/`heartbeat`
// 时才自增。正确的终态状态机一旦判死即就地停摆、绝不再触端口，故此计数应**恒为 0**；
// 而一条「失败后重建 / 自愈重连」的回归边一定会在判死后继续敲后端 → 计数 > 0 → 红。
// 这样断言 `connect_attempt_count() == 0` 才真正**观察 SUT**，而非对一个谁都无法递增的
// 计数器做恒真判定（trace-1 修复点）。

/// 续约结果配置：成功（携带远端给出的新 expiry）或失败（触发转死亡）。
#[derive(Clone, Copy)]
enum RenewMode {
    /// 续约成功，远端给出固定的新到期点（逻辑时刻）。
    SucceedTo(Instant),
    /// 续约一律失败（L-4 / L-3 桩）。
    AlwaysFail,
}

/// 记录次数的桩保活后端（成功桩 / 失败桩二合一，按 `renew_mode` 取值）。
#[derive(Clone)]
struct StubBackend {
    /// 已发起的续约请求次数（F-2 / F-3 观察点）。
    renew_calls: Arc<AtomicU64>,
    /// 已发起的心跳请求次数（F-3 观察点）。
    heartbeat_calls: Arc<AtomicU64>,
    /// 「判死后仍被状态机敲后端」的次数——**应恒为 0**（L-3/L-4：无自愈、无重建边）。
    /// 桩在发出过死亡判定后，再收到任何 `renew`/`heartbeat` 调用即自增（见 [`StubBackend`]
    /// 头注：这是自愈 / 重建回归在本端口上的唯一可观察形态，故此自增点即 SUT 观察点）。
    connect_attempts: Arc<AtomicU64>,
    /// 桩是否已发出过「死亡判定」（续约失败 / 心跳僵死）。一旦置真，此后任何端口调用
    /// 都被记为「判死后仍敲后端」（自愈 / 重建嫌疑），自增 `connect_attempts`。
    dead_verdict_issued: Arc<AtomicBool>,
    /// 续约行为：成功（新 expiry）/ 失败。
    renew_mode: RenewMode,
    /// 心跳判定：活 / 僵死。
    heartbeat_verdict: Heartbeat,
}

impl StubBackend {
    /// 续约成功桩：每次续约把 expiry 刷新到远端给出的 `new_expiry`；心跳判定为活。
    fn renew_succeeds_to(new_expiry: Instant) -> Self {
        Self {
            renew_calls: Arc::new(AtomicU64::new(0)),
            heartbeat_calls: Arc::new(AtomicU64::new(0)),
            connect_attempts: Arc::new(AtomicU64::new(0)),
            dead_verdict_issued: Arc::new(AtomicBool::new(false)),
            renew_mode: RenewMode::SucceedTo(new_expiry),
            heartbeat_verdict: Heartbeat::Alive,
        }
    }

    /// 续约一律失败桩：续约触发即失败（→ 转死亡）；心跳判定为活（不干扰续约观察）。
    fn renew_always_fails() -> Self {
        Self {
            renew_calls: Arc::new(AtomicU64::new(0)),
            heartbeat_calls: Arc::new(AtomicU64::new(0)),
            connect_attempts: Arc::new(AtomicU64::new(0)),
            dead_verdict_issued: Arc::new(AtomicBool::new(false)),
            renew_mode: RenewMode::AlwaysFail,
            heartbeat_verdict: Heartbeat::Alive,
        }
    }

    fn renew_count(&self) -> u64 {
        self.renew_calls.load(Ordering::SeqCst)
    }

    fn heartbeat_count(&self) -> u64 {
        self.heartbeat_calls.load(Ordering::SeqCst)
    }

    fn connect_attempt_count(&self) -> u64 {
        self.connect_attempts.load(Ordering::SeqCst)
    }
}

impl StubBackend {
    /// 端口被状态机触达的统一入口记账：若桩**已发出过死亡判定**，则本次调用属于
    /// 「判死后仍敲后端」——自愈 / 重建回归在本端口上的唯一可观察形态——自增
    /// `connect_attempts`（L-3/L-4 观察点）。正确的终态状态机判死后绝不再触端口，
    /// 故此自增点恒不触发、计数恒 0。
    fn record_touch_after_death(&self) {
        if self.dead_verdict_issued.load(Ordering::SeqCst) {
            self.connect_attempts.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait]
impl KeepaliveBackend for StubBackend {
    async fn renew(&self) -> Result<Renewal, ()> {
        self.record_touch_after_death();
        self.renew_calls.fetch_add(1, Ordering::SeqCst);
        match self.renew_mode {
            RenewMode::SucceedTo(new_expiry) => Ok(Renewal { new_expiry }),
            RenewMode::AlwaysFail => {
                // 续约失败即一次死亡判定：此后任何端口调用都算「判死后仍敲后端」。
                self.dead_verdict_issued.store(true, Ordering::SeqCst);
                Err(())
            }
        }
    }

    async fn heartbeat(&self) -> Heartbeat {
        self.record_touch_after_death();
        self.heartbeat_calls.fetch_add(1, Ordering::SeqCst);
        if self.heartbeat_verdict == Heartbeat::Dead {
            // 心跳判定僵死即一次死亡判定：此后任何端口调用都算「判死后仍敲后端」。
            self.dead_verdict_issued.store(true, Ordering::SeqCst);
        }
        self.heartbeat_verdict
    }
}

// 逻辑时间轴常量（§9）：到期点 / 提前量 / 心跳节律，落在 `FakeClock` 同一时间轴。
const SECS: Duration = Duration::from_secs(1);

fn instant_secs(s: u64) -> Instant {
    Instant(Duration::from_secs(s))
}

// ── F-2：长连接保活续约确被发起 ──────────────────────────────────────────

/// §8 F-2：续约成功桩 + 推进到「临近续约阈值」（`expiry − skew`）的 Fake 时钟驱动
/// 一条 `persistent == true` 通路 → 桩后端记录的续约请求次数从 0 增至 ≥1（续约确被
/// 发起），且此后健康查询仍返回 `Alive`。
#[tokio::test]
async fn f2_persistent_renewal_is_initiated_and_stays_alive() {
    // expiry=100s，skew=10s → 续约触发点 = 90s。续约成功后刷新到 200s。
    let backend = StubBackend::renew_succeeds_to(instant_secs(200));
    let clock = FakeClock::new();
    let (writer, reader) = health_view();

    let mut ka = Keepalive::persistent(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100), // expiry：建立时由「后端」给出（非本域常量），此处模拟初始到期点
        10 * SECS,         // skew
        30 * SECS,         // heartbeat_interval
    );

    // 续约前：续约次数为 0（尚未发起）。
    assert_eq!(backend.renew_count(), 0, "no renewal before threshold");

    // 推进逻辑时间到续约触发点（90s = expiry − skew），再 tick 驱动状态机。
    clock.advance(90 * SECS);
    let outcome = ka.tick().await;

    // 续约确被发起：续约请求次数 0 → ≥1。
    assert!(
        backend.renew_count() >= 1,
        "renewal must be initiated at expiry-skew threshold, got {}",
        backend.renew_count()
    );
    // 续约成功 → 状态机维持「活」，本 tick 结果为 Renewed。
    assert_eq!(outcome, KeepaliveOutcome::Renewed);
    // 此后健康查询仍返回 Alive（续约成功不写死亡）。
    assert_eq!(reader.get(), Health::Alive);
    // 续约成功**不**新建连接（续约是协议级机制、不是重建，§3.3）。
    assert_eq!(backend.connect_attempt_count(), 0);
}

// ── 续约刷新 expiry：下一触发点基于远端返回的新 expiry（证明 expiry 来自远端）──

/// §8 续约刷新 expiry：续约成功后，下一次续约触发点基于桩后端返回的**新 expiry**
/// 计算——推进到**旧** expiry−skew 之后的时刻（旧触发点已过）不再因旧到期点触发；
/// 唯有推进到**新** `expiry − skew` 才再次触发续约。证明 expiry 来自远端而非本域常量。
#[tokio::test]
async fn renewal_refreshes_expiry_from_remote_given_value() {
    // 初始 expiry=100s，skew=10s → 旧触发点 90s；续约成功刷新到新 expiry=200s →
    // 新触发点 190s。
    let backend = StubBackend::renew_succeeds_to(instant_secs(200));
    let clock = FakeClock::new();
    let (writer, _reader) = health_view();

    let mut ka = Keepalive::persistent(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100),
        10 * SECS,
        30 * SECS,
    );

    // 第一次：推进到旧触发点 90s，续约触发并刷新 expiry。
    clock.advance(90 * SECS);
    assert_eq!(ka.tick().await, KeepaliveOutcome::Renewed);
    assert_eq!(backend.renew_count(), 1);

    // expiry 已被远端返回的新到期点刷新到 200s → 新触发点为 190s。
    assert_eq!(
        ka.expiry(),
        instant_secs(200),
        "expiry must be refreshed to the remote-given new_expiry"
    );
    assert_eq!(
        ka.renew_at(),
        instant_secs(190),
        "next renewal trigger must be derived from the new (remote) expiry, not a local constant"
    );

    // 推进到 120s（已越过**旧** 90s 触发点，但远未到**新** 190s 触发点）→ 不再续约。
    clock.advance(30 * SECS); // now = 120s
    let outcome = ka.tick().await;
    assert_eq!(
        outcome,
        KeepaliveOutcome::Idle,
        "must NOT renew again at old expiry-skew; expiry came from remote"
    );
    assert_eq!(
        backend.renew_count(),
        1,
        "renewal count stays 1: old threshold must not re-trigger after refresh"
    );

    // 推进到新触发点 190s → 再次续约（基于远端给出的新 expiry 计算）。
    clock.advance(70 * SECS); // now = 190s
    let outcome = ka.tick().await;
    assert_eq!(outcome, KeepaliveOutcome::Renewed);
    assert_eq!(
        backend.renew_count(),
        2,
        "renewal re-triggers only at the NEW remote-given expiry-skew"
    );
}

// ── L-4：续约失败 → 转死亡（不掩盖、不停留 Alive、无自愈重连）──────────────

/// §8 L-4：续约一律失败桩 + 推进到续约阈值的 Fake 时钟 → 续约失败后健康视图返回
/// `Dead`（不掩盖、不停留 `Alive`），且桩后端记录的新建连接尝试次数恒为 0（无自愈、无重建）。
#[tokio::test]
async fn l4_renewal_failure_transitions_to_dead_no_new_connection() {
    let backend = StubBackend::renew_always_fails();
    let clock = FakeClock::new();
    let (writer, reader) = health_view();

    let mut ka = Keepalive::persistent(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100),
        10 * SECS,
        30 * SECS,
    );

    // 失败前：健康为 Alive。
    assert_eq!(reader.get(), Health::Alive);

    // 推进到续约触发点（90s），tick 驱动 → 续约被发起且失败。
    clock.advance(90 * SECS);
    let outcome = ka.tick().await;

    // 续约确被发起（失败也算发起了一次）。
    assert_eq!(backend.renew_count(), 1);
    // 续约失败 → 状态机一次性转死亡（本 tick 结果为 Died，精确到变体）。
    assert_eq!(outcome, KeepaliveOutcome::Died);
    // 健康视图返回 Dead——不掩盖、不停留 Alive。
    assert_eq!(reader.get(), Health::Dead);
    assert_ne!(reader.get(), Health::Alive);
    // 判死后再敲后端的次数恒为 0：续约失败即终态停摆，绝不继续触端口去「复活」
    // （无自愈重连，L-4 核心）。本断言现观察 SUT —— 任何判死后续约 / 心跳回归都会自增。
    assert_eq!(
        backend.connect_attempt_count(),
        0,
        "renewal failure must NOT trigger any self-healing new connection"
    );
    // 续约失败的本 tick 即就地停摆：本次之外不得再触端口（恰 1 次续约、0 次心跳）。
    assert_eq!(
        backend.renew_count(),
        1,
        "exactly one renewal attempt, then terminal stop"
    );
    assert_eq!(
        backend.heartbeat_count(),
        0,
        "renewal-failure path must not also probe"
    );
}

// ── L-3：无重建边（Dead 后继续推进恒 Dead，新建连接尝试恒 0）────────────────

/// §8 L-3：状态机 `Dead` 后，继续推进 Fake 时钟越过任意保活周期，健康持续 `Dead`
/// （不翻回 `Alive`），桩后端新建连接尝试次数恒为 0（无后台重建 / 无静默切换）。
#[tokio::test]
async fn l3_no_rebuilding_edge_after_dead() {
    let backend = StubBackend::renew_always_fails();
    let clock = FakeClock::new();
    let (writer, reader) = health_view();

    let mut ka = Keepalive::persistent(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100),
        10 * SECS,
        30 * SECS,
    );

    // 先把状态机推进到死亡（续约失败）。
    clock.advance(90 * SECS);
    assert_eq!(ka.tick().await, KeepaliveOutcome::Died);
    assert_eq!(reader.get(), Health::Dead);
    let renew_at_death = backend.renew_count();
    let heartbeat_at_death = backend.heartbeat_count();

    // Dead 后继续推进越过多个保活周期，反复 tick：健康持续 Dead、绝不翻回 Alive。
    for _ in 0..5 {
        clock.advance(1000 * SECS); // 越过任意续约 / 心跳周期
        let outcome = ka.tick().await;
        assert_eq!(
            outcome,
            KeepaliveOutcome::Died,
            "after death the state machine is terminal: no revival, no rebuilding edge"
        );
        assert_eq!(reader.get(), Health::Dead);
        assert_ne!(
            reader.get(),
            Health::Alive,
            "Dead must never flip back to Alive"
        );
    }

    // 判死后再敲后端的次数恒为 0：无后台重建、无静默切换到其他通路（L-3 核心）。
    // 本断言现观察 SUT —— 死后任何对端口的续约 / 心跳调用都会自增此计数。
    assert_eq!(
        backend.connect_attempt_count(),
        0,
        "no new-connection attempt may ever be issued by the keepalive unit"
    );
    // 死亡终态后不再发起新续约（停摆，不在死后继续敲后端）。
    assert_eq!(
        backend.renew_count(),
        renew_at_death,
        "no further renewal after death: terminal stop, not a retry loop"
    );
    // 死亡终态后也不再发起新心跳（停摆覆盖到第二类保活手段，不是只停续约）。
    assert_eq!(
        backend.heartbeat_count(),
        heartbeat_at_death,
        "no further heartbeat after death: terminal stop covers probing too"
    );
}

// ── F-3：非长连接不保活（persistent == false 立即空转）────────────────────

/// §8 F-3：非长连接路径不启动保活——`persistent == false` 的状态机对推进立即空转；
/// 推进时钟越过任意阈值后桩后端续约 / 心跳请求次数恒为 0。
#[tokio::test]
async fn f3_ephemeral_never_keepalives() {
    let backend = StubBackend::renew_succeeds_to(instant_secs(200));
    let clock = FakeClock::new();
    let (writer, reader) = health_view();

    let mut ka = Keepalive::ephemeral(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100),
        10 * SECS,
        30 * SECS,
    );

    // 推进越过续约阈值（90s）与多个心跳节律，反复 tick。
    for _ in 0..10 {
        clock.advance(50 * SECS);
        let outcome = ka.tick().await;
        // 非长连接路径立即空转：本 tick 既不续约也不心跳。
        assert_eq!(
            outcome,
            KeepaliveOutcome::Idle,
            "ephemeral (persistent==false) path must never keepalive"
        );
    }

    // 续约 / 心跳请求次数恒为 0（建立期间不发起任何保活，F-3）。
    assert_eq!(
        backend.renew_count(),
        0,
        "ephemeral path must issue no renewal"
    );
    assert_eq!(
        backend.heartbeat_count(),
        0,
        "ephemeral path must issue no heartbeat"
    );
    assert_eq!(backend.connect_attempt_count(), 0);
    // 非长连接不因保活转死亡：未触发任何保活判定，健康保持 Alive。
    assert_eq!(reader.get(), Health::Alive);
}

// ── 心跳节律：未到续约阈值但到心跳节律 → 发心跳、维持活 ────────────────────

/// §8 F-2 辅证（心跳手段）：长连接在未到续约阈值、但到达心跳节律时发起心跳探测，
/// 应答为活 → 维持 `Alive`，本 tick 结果为 `Probed`。钉死「心跳是续约之外的第二类
/// 保活手段」且其成功不写死亡。
#[tokio::test]
async fn heartbeat_at_interval_keeps_alive() {
    // expiry=100s，skew=10s → 续约触发点 90s；heartbeat_interval=30s。
    let backend = StubBackend::renew_succeeds_to(instant_secs(200));
    let clock = FakeClock::new();
    let (writer, reader) = health_view();

    let mut ka = Keepalive::persistent(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100),
        10 * SECS,
        30 * SECS,
    );

    // 推进到 30s：未到续约触发点（90s），但到心跳节律 → 发心跳。
    clock.advance(30 * SECS);
    let outcome = ka.tick().await;

    assert_eq!(
        outcome,
        KeepaliveOutcome::Probed,
        "before renewal threshold, the heartbeat rhythm must drive a probe"
    );
    assert_eq!(
        backend.heartbeat_count(),
        1,
        "heartbeat must be initiated at its interval"
    );
    // 续约阈值未到 → 不续约。
    assert_eq!(
        backend.renew_count(),
        0,
        "renewal must not fire before its threshold"
    );
    // 心跳应答为活 → 健康维持 Alive。
    assert_eq!(reader.get(), Health::Alive);
}

// ── 心跳判定僵死 → 转死亡（失败路径一等公民）─────────────────────────────

/// §8 L-4 / §3.3：心跳判定僵死时通路转「死亡」如实呈现，不掩盖、不自愈重连。
/// 用「心跳判定僵死」的桩 + 推进到心跳节律 → 健康返回 `Dead`，新建连接尝试恒 0。
#[tokio::test]
async fn heartbeat_dead_verdict_transitions_to_dead() {
    // 心跳判定僵死的桩：续约不触发（未到阈值），仅心跳判定死。
    let backend = StubBackend {
        renew_calls: Arc::new(AtomicU64::new(0)),
        heartbeat_calls: Arc::new(AtomicU64::new(0)),
        connect_attempts: Arc::new(AtomicU64::new(0)),
        dead_verdict_issued: Arc::new(AtomicBool::new(false)),
        renew_mode: RenewMode::SucceedTo(instant_secs(200)),
        heartbeat_verdict: Heartbeat::Dead,
    };
    let clock = FakeClock::new();
    let (writer, reader) = health_view();

    let mut ka = Keepalive::persistent(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100),
        10 * SECS,
        30 * SECS,
    );

    assert_eq!(reader.get(), Health::Alive);

    // 推进到心跳节律（30s，未到续约阈值 90s）→ 心跳判定僵死。
    clock.advance(30 * SECS);
    let outcome = ka.tick().await;

    assert_eq!(
        backend.heartbeat_count(),
        1,
        "heartbeat probe must be initiated"
    );
    assert_eq!(
        outcome,
        KeepaliveOutcome::Died,
        "heartbeat dead verdict must transition the state machine to Dead"
    );
    assert_eq!(reader.get(), Health::Dead);
    assert_ne!(reader.get(), Health::Alive);
    // 心跳判定死也不自愈重连。
    assert_eq!(backend.connect_attempt_count(), 0);
}

// ── 续约「临界点」：宁可早续不可晚续（阈值=expiry−skew 触发，非到 expiry 才触发）──

/// §8 F-2 / §3.3「宁可早续不可晚续」：续约触发点恰为 `expiry − skew`，而非到 `expiry`
/// 才续约。推进到**恰** `expiry − skew`（90s）即触发续约（早于硬过期 100s）。钉死
/// 提前量留足续约往返、晚续即死亡的 fail-closed 取舍。
#[tokio::test]
async fn renewal_fires_at_expiry_minus_skew_not_at_expiry() {
    let backend = StubBackend::renew_succeeds_to(instant_secs(200));
    let clock = FakeClock::new();
    let (writer, _reader) = health_view();

    let mut ka = Keepalive::persistent(
        clock.clone(),
        backend.clone(),
        writer,
        instant_secs(100),
        10 * SECS,
        30 * SECS,
    );

    // renew_at == expiry − skew == 90s（不是 expiry==100s）。
    assert_eq!(ka.renew_at(), instant_secs(90));

    // 推进到 89s：恰未到续约触发点 → 不续约（钉死「触发点是 90s 不是更早」）。
    clock.advance(89 * SECS);
    assert_eq!(ka.tick().await, KeepaliveOutcome::Idle);
    assert_eq!(
        backend.renew_count(),
        0,
        "must not renew before expiry-skew"
    );

    // 推进到恰 90s（expiry − skew）→ 触发续约（早于硬过期 100s，宁可早续）。
    clock.advance(SECS); // now = 90s
    assert_eq!(ka.tick().await, KeepaliveOutcome::Renewed);
    assert_eq!(
        backend.renew_count(),
        1,
        "renewal must fire exactly at expiry-skew, ahead of the hard expiry"
    );
}

// ── 构造签名审查（散文级，对齐 L-2/L-3）：本单元无禁用符号 / 无退避器 ──────

/// §8 L-2/L-3 `构造签名审查`：被测保活实现单元（`keepalive/{mod,renew,clock}.rs`）的
/// 源码文本中不出现任何重建 / 故障切换 / 退避 / 重试计数类符号——文本级钉死「保活
/// 状态机无失败后重建转移边、无退避器」这一本 crate 最核心纪律（§3.3 / §7-3 / L-2/L-3）。
///
/// 审查面是实现源码（经 `include_str!` 内联），不是测试夹具自身；拆分关键字以免审查器
/// 把本断言自身误判为违规标记。仓库级契约门与 clippy 门禁兜底同款审查，此处把它落成
/// 一条可在 `cargo test` 内复现的红线观察。
#[test]
fn forbidden_symbols_are_absent_from_keepalive_impl() {
    let impl_sources: &[&str] = &[
        include_str!("../src/keepalive/mod.rs"),
        include_str!("../src/keepalive/renew.rs"),
        include_str!("../src/keepalive/clock.rs"),
    ];
    let forbidden = [
        concat!("recon", "nect"),
        concat!("reb", "uild"),
        concat!("fail", "over"),
        concat!("back", "off"),
        concat!("retry", "_count"),
    ];
    for src in impl_sources {
        for needle in forbidden {
            assert!(
                !src.contains(needle),
                "keepalive impl must contain no rebuild/backoff/retry symbol: {needle}"
            );
        }
    }
}
