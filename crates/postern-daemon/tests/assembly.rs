//! assembly-main 单元行为测试（RED）。
//!
//! 钉住 daemon 的**唯一组装点**在进程级的对外形态契约（模块文档 06 §1 唯一组装点 /
//! §3.8 并发与线程模型 multi-thread / §5.1 进程对外形态 / §8 DoD 九项 + 场景载体）：
//!
//! 1. **boot Err → 非零进程退出码 + data.sock 不 serving**（§8 / 公理二 fail-closed）：
//!    一次 boot `Err` 经 [`boot_exit_code`] 映射为**非零**退出码（`main` 据此非零退出），
//!    且 boot 在 socket 创建前短路（`BootReport::data_plane_open == false`）——data.sock
//!    从未 serving。happy 路径 → 退出码 0、data.sock serving。
//! 2. **三平面各自独立 spawn**（§3.8 multi-thread）：boot 成功后 [`serve_assembled`] 把
//!    数据面 router / 控制面 router / sweeper 周期任务**各自独立** spawn 到运行时；以记录式
//!    Fake [`PlaneSpawner`] 见证「恰好三处 spawn、各拿到 boot 装配出的对应句柄集」。
//! 3. **红线 7.2-2 的进程级见证**：数据面 spawn **绝不**收到 `PolicyRepo` 写句柄；控制面 /
//!    sweeper spawn 才持它。
//! 4. **spawn 失败 → fail-closed Err**（不放行半装配进程形态）。
//! 5. **已装配状态端到端求值（场景载体）**：用纯内存 Fake 全插件装配真实 [`Kernel`]，驱动
//!    一次放行请求 → 经出口脱敏的 `Ok`；一次失败注入 → 带确切 `Stage` 的 `Err(DenyResponse)`。
//!    这是 §8「本 crate 是 verify 九项与场景载体」的端到端载体（组装产物确实能跑出可观察
//!    决策与审计）。
//!
//! 驱动方式（06 §9）：boot/spawn 链以 Fake [`Preconditions`]/[`SocketFactory`]/
//! [`ConnectableUidProbe`]/[`RouterAssembler`]/[`PlaneSpawner`] 注入；kernel 端到端以内存
//! Fake `Authenticator`/`Adapter`/`ConditionPredicate`/`ConnAcquire`/`AuditSink`/`Sanitizer`
//! + 纯内存 `PolicySnapshot` 注入。每条只钉一个行为，失败路径一等公民。
//!
//! 雷区纪律：本文件**零 SQL 标记**；需要 `ConnOrigin` 时以
//! `use postern_core::request::ConnOrigin as Origin` 别名构造（测试在 shells 外，绝不写字面
//! `ConnOrigin::` 变体）；**绝不构造** `ResolvedTarget`/`ResourceCredential`（建连缝直接交还
//! 不透明 `Channel`，Fake 永不触达机密类型）。异步用 `#[tokio::test]`。
//!
//! 实现为 RED 桩（[`boot_exit_code`]/[`serve_assembled`]/`Bootstrap::run`/`Kernel::submit`
//! 体为 `todo!()`），故凡驱动装配/求值链的测试即 panic → 观察到红。纯类型/常量层断言（退出
//! 码非零、HandleKind 分流形状）先于实现成立则单独标注，验组装编排正确。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use postern_core::decision::{Decision, DenyResponse};
use postern_core::domain::{
    Capability, ConditionSpec, ConstraintSpec, CredentialMeta, CredentialTier, CredentialView,
    EvalContext, GrantAction, GrantCell, PolicySnapshot, PresentedCredential, PrincipalId,
    ResourceCode, Role, TierDecl, Timestamp,
};
use postern_core::error::{
    AuditError, AuthError, ClassifyError, ConstraintError, DiscoverError, ExecError,
    PredicateError, Stage, TransportError,
};
use postern_core::eval::evaluator::Evaluator;
use postern_core::id::SnowflakeId;
use postern_core::plugin::sanitize::{MaskRule, SanitizedResponse, Sanitizer, StreamScrubber};
use postern_core::plugin::{
    Adapter, AuditEvent, AuditSink, Authenticator, CapabilitySurface, Channel, ConditionPredicate,
    RawResponse,
};
use postern_core::request::{ClassifiedIntent, Intent, NormalizedRequest, ObjectRef};
// 测试在 shells 外：需要请求来源以别名构造，绝不写字面 ConnOrigin:: 变体（雷区 2 / B-2）。
use postern_core::request::ConnOrigin as Origin;

use postern_daemon::assemble::{
    boot_exit_code, run_assembled, serve_assembled, PlaneSpawner, EXIT_BOOT_FAILED, EXIT_OK,
};
use postern_daemon::boot::{
    BootError, BootReport, BootStage, Bootstrap, ConnectableUidProbe, HandleKind, Preconditions,
    RouterAssembler, SocketFactory,
};
use postern_daemon::error::{DaemonError, Result};
use postern_daemon::kernel::pipeline::ConnAcquire;
use postern_daemon::kernel::Kernel;

// ════════════════════════════════════════════════════════════════════════
//  Part A — 进程级装配缝：boot 编排 Fake（前置/socket/探针/router 装配）
// ════════════════════════════════════════════════════════════════════════

/// 固定 daemon 自身 uid 样本（非 0，避免与 root 特例混淆）。
const SELF_UID: u32 = 1000;

/// 哪个 boot 前置/ socket 步骤被注入失败（`None` = 全成功 happy 路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailAt {
    OpenDb,
    UnlockVault,
    ControlSocket,
}

/// Fake 四前置（开库/迁移/首快照/解锁）：按序记录调用，可在任一步注入失败。
struct FakePreconditions {
    fail_at: Option<FailAt>,
}

impl FakePreconditions {
    fn ok() -> Self {
        Self { fail_at: None }
    }
    fn failing(fail_at: FailAt) -> Self {
        Self {
            fail_at: Some(fail_at),
        }
    }
}

impl Preconditions for FakePreconditions {
    fn open_db(&self) -> Result<()> {
        if self.fail_at == Some(FailAt::OpenDb) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
    fn migrate(&self) -> Result<()> {
        Ok(())
    }
    fn rebuild_first_snapshot(&self) -> Result<()> {
        Ok(())
    }
    fn unlock_vault(&self) -> Result<()> {
        if self.fail_at == Some(FailAt::UnlockVault) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
}

/// Fake 两平面 socket 创建：记录创建调用序，可注入 control 失败（验早失败时 data 从未创建）。
struct FakeSockets {
    fail_at: Option<FailAt>,
    created: Arc<Mutex<Vec<BootStage>>>,
}

impl FakeSockets {
    fn ok() -> Self {
        Self {
            fail_at: None,
            created: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn failing(fail_at: FailAt) -> Self {
        Self {
            fail_at: Some(fail_at),
            created: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl SocketFactory for FakeSockets {
    fn create_control(&self) -> Result<()> {
        self.created.lock().unwrap().push(BootStage::ControlSocket);
        if self.fail_at == Some(FailAt::ControlSocket) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
    fn create_data(&self) -> Result<()> {
        self.created.lock().unwrap().push(BootStage::DataSocket);
        Ok(())
    }
}

/// Fake data.sock 可连 uid 探针：可连集合不含自身 uid（正常路径）。
struct FakeProbe {
    self_uid: u32,
    connectable: Vec<u32>,
}

impl FakeProbe {
    fn safe(self_uid: u32) -> Self {
        Self {
            self_uid,
            connectable: vec![self_uid.wrapping_add(1000)],
        }
    }
}

impl ConnectableUidProbe for FakeProbe {
    fn self_uid(&self) -> u32 {
        self.self_uid
    }
    fn connectable_uids(&self) -> Vec<u32> {
        self.connectable.clone()
    }
}

/// Fake 两平面 router 装配：记录两平面实际收到的句柄集（B-2/L-14 接线点）。
struct FakeAssembler {
    data: Arc<Mutex<Vec<HandleKind>>>,
    control: Arc<Mutex<Vec<HandleKind>>>,
}

impl FakeAssembler {
    fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(Vec::new())),
            control: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl RouterAssembler for FakeAssembler {
    fn assemble_data_plane(&self, handles: &[HandleKind]) -> Result<()> {
        self.data.lock().unwrap().extend_from_slice(handles);
        Ok(())
    }
    fn assemble_control_plane(&self, handles: &[HandleKind]) -> Result<()> {
        self.control.lock().unwrap().extend_from_slice(handles);
        Ok(())
    }
}

/// 装配一个 happy 路径编排器（四前置全成功、socket 全成功、可连集合不含自身 uid）。
fn bootstrap_happy() -> Bootstrap<FakePreconditions, FakeSockets, FakeProbe, FakeAssembler> {
    Bootstrap::new(
        FakePreconditions::ok(),
        FakeSockets::ok(),
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    )
}

// ════════════════════════════════════════════════════════════════════════
//  Fake PlaneSpawner：记录三平面各自被 spawn 的句柄集（§3.8 三处独立 spawn 的接线点）
// ════════════════════════════════════════════════════════════════════════

/// 哪个平面的 spawn 被注入失败（`None` = 三处全成功）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnFailAt {
    DataPlane,
    ControlPlane,
    Sweeper,
}

/// 记录三平面 router/任务**实际被 spawn 时收到的句柄集**——断言读的是「每处 spawn 真正收到
/// 了什么」，而非 run() 另写的字面 vec。若实现把 PolicyRepo 误注入数据面 spawn，`data` 必含
/// 它 → 红（红线 7.2-2 进程级见证）。可在任一平面注入 spawn 失败，验 fail-closed。
struct FakeSpawner {
    fail_at: Option<SpawnFailAt>,
    data: Arc<Mutex<Vec<HandleKind>>>,
    control: Arc<Mutex<Vec<HandleKind>>>,
    sweeper: Arc<Mutex<Vec<HandleKind>>>,
    /// 三处 spawn 的总调用计数（验「恰好三处独立 spawn」）。
    spawned: Arc<AtomicUsize>,
}

impl FakeSpawner {
    fn ok() -> Self {
        Self::new(None)
    }
    fn failing(fail_at: SpawnFailAt) -> Self {
        Self::new(Some(fail_at))
    }
    fn new(fail_at: Option<SpawnFailAt>) -> Self {
        Self {
            fail_at,
            data: Arc::new(Mutex::new(Vec::new())),
            control: Arc::new(Mutex::new(Vec::new())),
            sweeper: Arc::new(Mutex::new(Vec::new())),
            spawned: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn data_handles(&self) -> Vec<HandleKind> {
        self.data.lock().unwrap().clone()
    }
    fn control_handles(&self) -> Vec<HandleKind> {
        self.control.lock().unwrap().clone()
    }
    fn sweeper_handles(&self) -> Vec<HandleKind> {
        self.sweeper.lock().unwrap().clone()
    }
    fn spawn_count(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }
}

impl PlaneSpawner for FakeSpawner {
    fn spawn_data_plane(&self, handles: &[HandleKind]) -> Result<()> {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        self.data.lock().unwrap().extend_from_slice(handles);
        if self.fail_at == Some(SpawnFailAt::DataPlane) {
            return Err(DaemonError::Listener);
        }
        Ok(())
    }
    fn spawn_control_plane(&self, handles: &[HandleKind]) -> Result<()> {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        self.control.lock().unwrap().extend_from_slice(handles);
        if self.fail_at == Some(SpawnFailAt::ControlPlane) {
            return Err(DaemonError::Listener);
        }
        Ok(())
    }
    fn spawn_sweeper(&self, handles: &[HandleKind]) -> Result<()> {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        self.sweeper.lock().unwrap().extend_from_slice(handles);
        if self.fail_at == Some(SpawnFailAt::Sweeper) {
            return Err(DaemonError::Listener);
        }
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════
//  §8 / 公理二：boot Err → 非零进程退出码 + data.sock 不 serving
// ════════════════════════════════════════════════════════════════════════

// §8：开库失败（最早一步）→ boot Err → 退出码非零（EXIT_BOOT_FAILED）。fail-closed。
#[tokio::test]
async fn boot_open_db_failure_maps_to_nonzero_exit_code() {
    let boot = Bootstrap::new(
        FakePreconditions::failing(FailAt::OpenDb),
        FakeSockets::ok(),
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );
    let result = boot.run();
    assert!(result.is_err(), "开库失败 → boot Err（fail-closed）");
    let code = boot_exit_code(&result);
    assert_ne!(
        code, EXIT_OK,
        "boot 失败的退出码必为非零（§8：boot Err → 非零进程退出）"
    );
    assert_eq!(
        code, EXIT_BOOT_FAILED,
        "boot 失败恒映射到 EXIT_BOOT_FAILED（非零、可被 systemd/容器判为启动失败）"
    );
}

// §8：解锁保险箱失败（链中段）→ boot Err 且 data.sock 从未创建（不 serving）。fail-closed。
#[tokio::test]
async fn boot_unlock_failure_yields_nonzero_exit_and_data_sock_not_serving() {
    // 解锁失败发生在 socket 创建之前：用前置失败 + 全 OK socket，断言 data.sock 从未创建。
    let socket_recorder = FakeSockets::ok();
    let created_handle = socket_recorder.created.clone();
    let boot = Bootstrap::new(
        FakePreconditions::failing(FailAt::UnlockVault),
        socket_recorder,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );
    let result = boot.run();
    assert!(result.is_err(), "解锁失败 → boot Err");
    // data.sock 不 serving：socket 创建调用序里恒不含 DataSocket（boot 在其创建前短路）。
    let created = created_handle.lock().unwrap().clone();
    assert!(
        !created.contains(&BootStage::DataSocket),
        "boot 在 data.sock 创建前短路：data.sock 从未创建（不 serving，F-1）"
    );
    // 退出码非零（§8）。
    assert_ne!(
        boot_exit_code(&result),
        EXIT_OK,
        "中段失败同样映射为非零退出码"
    );
}

// §8：control.sock 创建失败 → boot Err，data.sock 从未创建（control 先于 data）。
#[tokio::test]
async fn boot_control_socket_failure_keeps_data_sock_uncreated() {
    let socket_recorder = FakeSockets::failing(FailAt::ControlSocket);
    let created_handle = socket_recorder.created.clone();
    let boot = Bootstrap::new(
        FakePreconditions::ok(),
        socket_recorder,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );
    let result = boot.run();
    assert!(result.is_err(), "control.sock 创建失败 → boot Err");
    let created = created_handle.lock().unwrap().clone();
    assert!(
        created.contains(&BootStage::ControlSocket),
        "control.sock 创建确已被尝试（先于 data）"
    );
    assert!(
        !created.contains(&BootStage::DataSocket),
        "control 失败后 data.sock 从未创建（control 先于 data，data 不 serving）"
    );
    assert_ne!(boot_exit_code(&result), EXIT_OK, "退出码非零");
}

// §8：happy 路径 → boot Ok、data.sock serving（data_plane_open）、退出码 0。
#[tokio::test]
async fn boot_success_maps_to_zero_exit_and_data_sock_serving() {
    let boot = bootstrap_happy();
    let result = boot.run();
    let report = match &result {
        Ok(r) => r.clone(),
        Err(e) => panic!("happy 路径应 boot 成功，got Err({e:?})"),
    };
    assert!(
        report.data_plane_open,
        "happy 路径：data.sock 已挂数据面 router（serving）"
    );
    assert!(
        report.executed.contains(&BootStage::DataSocket),
        "happy 路径：DataSocket 为整链终结动作，确已执行"
    );
    assert_eq!(
        boot_exit_code(&result),
        EXIT_OK,
        "boot 成功 → 退出码 0（§8：全装配就绪进程正常 serving）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §3.8 multi-thread：boot 成功后三平面各自独立 spawn（数据面/控制面 router + sweeper）
// ════════════════════════════════════════════════════════════════════════

// §3.8 / §8：boot 成功后 serve_assembled 把数据面 router / 控制面 router / sweeper 三者
// **各自独立** spawn —— 恰好三处 spawn。
#[tokio::test]
async fn serve_assembled_spawns_three_independent_planes() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy boot");
    let spawner = FakeSpawner::ok();
    let outcome = serve_assembled(&report, &spawner).await;
    assert!(outcome.is_ok(), "三平面 spawn 全成功 → serve_assembled Ok");
    assert_eq!(
        spawner.spawn_count(),
        3,
        "恰好三处独立 spawn：数据面 router + 控制面 router + sweeper 周期任务（§3.8 multi-thread）"
    );
}

// §8 / 红线 7.2-2：数据面 spawn 收到的句柄集**绝不含** PolicyRepo；控制面/sweeper spawn 才持它。
#[tokio::test]
async fn data_plane_spawn_never_receives_policy_repo_handle() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy boot");
    let spawner = FakeSpawner::ok();
    serve_assembled(&report, &spawner).await.expect("spawn ok");

    let data = spawner.data_handles();
    assert!(
        !data.contains(&HandleKind::PolicyRepo),
        "数据面 spawn 的句柄集绝不含 PolicyRepo 写句柄（红线 7.2-2 进程级见证）"
    );
    // 数据面 spawn 恰拿到数据面句柄（只读投影 + 连接池 + Sanitizer + 登记册）。
    assert!(
        data.contains(&HandleKind::PolicyView)
            && data.contains(&HandleKind::ConnPool)
            && data.contains(&HandleKind::Sanitizer)
            && data.contains(&HandleKind::Registries),
        "数据面 spawn 恰拿到数据面注入集（PolicyView/ConnPool/Sanitizer/Registries）"
    );

    // 控制面 spawn 才持 PolicyRepo 写句柄。
    let control = spawner.control_handles();
    assert!(
        control.contains(&HandleKind::PolicyRepo),
        "控制面 spawn 的句柄集含 PolicyRepo 写句柄（写句柄只进控制面/sweeper）"
    );
    // sweeper spawn 与控制面共用 PolicyRepo 写锁，故其句柄集亦含 PolicyRepo。
    let sweeper = spawner.sweeper_handles();
    assert!(
        sweeper.contains(&HandleKind::PolicyRepo),
        "sweeper spawn 与控制面共用 PolicyRepo 写锁（系统协调写，actor=system）"
    );
}

// §8：boot 成功后三平面 spawn 的句柄集**恰好源自 boot 装配产物**（BootReport 的两套句柄集），
// 而非另写字面 vec —— 数据面 spawn 的句柄集逐元素等于 report.data_plane_handles。
#[tokio::test]
async fn spawned_handle_sets_come_from_boot_report() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy boot");
    let spawner = FakeSpawner::ok();
    serve_assembled(&report, &spawner).await.expect("spawn ok");
    assert_eq!(
        spawner.data_handles(),
        report.data_plane_handles,
        "数据面 spawn 收到的句柄集恰为 boot 装配出的 data_plane_handles（接线点见证，非字面 vec）"
    );
    assert_eq!(
        spawner.control_handles(),
        report.control_plane_handles,
        "控制面 spawn 收到的句柄集恰为 boot 装配出的 control_plane_handles"
    );
}

// §8 / 公理二：任一平面 spawn 失败 → serve_assembled Err（不放行半装配进程形态）。
#[tokio::test]
async fn data_plane_spawn_failure_fails_closed() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy boot");
    let spawner = FakeSpawner::failing(SpawnFailAt::DataPlane);
    let outcome = serve_assembled(&report, &spawner).await;
    assert!(
        outcome.is_err(),
        "数据面 spawn 失败 → serve_assembled fail-closed Err（不放行半装配进程）"
    );
}

// §8 / 公理二：sweeper spawn 失败 → serve_assembled Err（三平面任一失败整体 fail-closed）。
#[tokio::test]
async fn sweeper_spawn_failure_fails_closed() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy boot");
    let spawner = FakeSpawner::failing(SpawnFailAt::Sweeper);
    let outcome = serve_assembled(&report, &spawner).await;
    assert!(
        outcome.is_err(),
        "sweeper spawn 失败 → serve_assembled fail-closed Err"
    );
}

// §8 / 公理二：**控制面** spawn 失败 → serve_assembled Err（补全三分支：数据面/控制面/sweeper
// 任一 spawn 失败都整体 fail-closed；控制面在 `?` 链中段，若被改成吞错继续则此处变红）。
#[tokio::test]
async fn control_plane_spawn_failure_fails_closed() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy boot");
    let spawner = FakeSpawner::failing(SpawnFailAt::ControlPlane);
    let outcome = serve_assembled(&report, &spawner).await;
    assert!(
        outcome.is_err(),
        "控制面 spawn 失败 → serve_assembled fail-closed Err（三平面任一失败整体 fail-closed）"
    );
}

// §8 / 公理二（fail-closed 守门）：boot 失败 / 半装配 report（data_plane_open == false）驱动
// serve_assembled 时**绝不** spawn 任何平面，且返回 Err——杜绝「先开门再装锁」。默认 BootReport
// 的 data_plane_open 为 false（boot 在 data.sock 创建前短路即此态），以它驱动断言「零 spawn + Err」。
#[tokio::test]
async fn serve_assembled_with_unopened_data_plane_spawns_nothing() {
    // 默认 report 即 boot 在 data.sock 创建前短路的形态：data_plane_open == false。
    let report = BootReport::default();
    assert!(
        !report.data_plane_open,
        "默认 report 的 data_plane_open 为 false（boot 短路 / 半装配态）"
    );
    let spawner = FakeSpawner::ok();
    let outcome = serve_assembled(&report, &spawner).await;
    assert!(
        outcome.is_err(),
        "data_plane_open == false → serve_assembled fail-closed Err（不放行半装配进程形态）"
    );
    assert_eq!(
        spawner.spawn_count(),
        0,
        "data.sock 未开放时绝不 spawn 任何平面（零 spawn：先开门再装锁不可发生，F-1）"
    );
}

// §8 item 4 端到端（main 路由的退出码语义）：run_assembled 把一次 boot 结果接线为进程退出码。
// 这是 main 实际调用的缝——boot Err → 非零退出码 **且** 零 spawn（data.sock 不 serving）；boot
// 成功 + 三平面 spawn 成功 → 退出码 0。无需起真实二进制即见证「boot Err → 非零进程退出」契约。
#[tokio::test]
async fn run_assembled_boot_err_yields_nonzero_exit_and_zero_spawn() {
    // 一次 boot Err（归因到最早一步 OpenDb）。
    let boot = Bootstrap::new(
        FakePreconditions::failing(FailAt::OpenDb),
        FakeSockets::ok(),
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );
    let result = boot.run();
    assert!(result.is_err(), "开库失败 → boot Err");
    let spawner = FakeSpawner::ok();
    let code = run_assembled(result, &spawner).await;
    assert_ne!(
        code, EXIT_OK,
        "boot Err → run_assembled 返回非零退出码（§8 item 4：main 据此 process::exit 非零）"
    );
    assert_eq!(
        code, EXIT_BOOT_FAILED,
        "boot Err 恒映射到 EXIT_BOOT_FAILED（systemd/容器据此判启动失败）"
    );
    assert_eq!(
        spawner.spawn_count(),
        0,
        "boot Err → 一处平面也不 spawn（data.sock 不 serving，fail-closed）"
    );
}

// §8 item 4 端到端（happy）：boot 成功 + 三平面 spawn 全成功 → run_assembled 返回退出码 0
// （进程正常 serving）。与 boot-Err 分支共同钉住「main 路由的退出码语义」两端。
#[tokio::test]
async fn run_assembled_boot_ok_yields_zero_exit_and_three_spawns() {
    let boot = bootstrap_happy();
    let result = boot.run();
    assert!(result.is_ok(), "happy → boot Ok");
    let spawner = FakeSpawner::ok();
    let code = run_assembled(result, &spawner).await;
    assert_eq!(
        code, EXIT_OK,
        "boot Ok + 三平面 spawn 成功 → 退出码 0（进程正常 serving）"
    );
    assert_eq!(
        spawner.spawn_count(),
        3,
        "boot Ok happy 路径恰三处独立 spawn（数据面 + 控制面 + sweeper）"
    );
}

// §8 item 4 端到端（spawn 失败）：boot 成功但某平面 spawn 失败 → run_assembled 返回非零退出码
// （不放行半装配进程形态，公理二）。验「boot Ok 也可因 spawn 失败而非零退出」。
#[tokio::test]
async fn run_assembled_spawn_failure_yields_nonzero_exit() {
    let boot = bootstrap_happy();
    let result = boot.run();
    assert!(result.is_ok(), "happy → boot Ok");
    let spawner = FakeSpawner::failing(SpawnFailAt::ControlPlane);
    let code = run_assembled(result, &spawner).await;
    assert_ne!(
        code, EXIT_OK,
        "boot Ok 但平面 spawn 失败 → 非零退出码（不放行半装配进程形态，公理二）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 退出码常量层（先于实现成立）：成功码恒 0、失败码恒非零
// ════════════════════════════════════════════════════════════════════════

// §8：退出码常量的承重事实——成功码恒为 0，boot 失败码恒**非零**（systemd/容器据此判定）。
#[test]
fn exit_code_constants_are_zero_and_nonzero() {
    assert_eq!(EXIT_OK, 0, "成功退出码恒为 0");
    assert_ne!(
        EXIT_BOOT_FAILED, 0,
        "boot 失败退出码恒非零（fail-closed 可观察）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  Part B — 场景载体：已装配状态（真实 Kernel + 内存 Fake 全插件）端到端求值
// ════════════════════════════════════════════════════════════════════════

const RESOURCE: &str = "db-main";
const AUTH_KIND: &str = "api_key";
const COND_KIND: &str = "always";

/// 共享调用探针：按序追加管线阶段标记，供「出口经脱敏」「未建连即 deny」等序断言。
#[derive(Default)]
struct CallLog {
    events: Mutex<Vec<&'static str>>,
}

impl CallLog {
    fn record(&self, tag: &'static str) {
        self.events.lock().unwrap().push(tag);
    }
    fn snapshot(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }
}

fn principal(raw: u64) -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(raw))
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_700_000_000_000)
}

fn unix_origin() -> Origin {
    Origin::UnixPeer {
        uid: 1000,
        gid: 1000,
    }
}

// ── Fake Authenticator ──
struct FakeAuth {
    outcome: std::result::Result<PrincipalId, AuthError>,
    log: Arc<CallLog>,
}

impl Authenticator for FakeAuth {
    fn kind(&self) -> &'static str {
        AUTH_KIND
    }
    fn authenticate(
        &self,
        _presented: &PresentedCredential,
        _origin: &Origin,
        _creds: &CredentialView,
        _now: Timestamp,
    ) -> std::result::Result<PrincipalId, AuthError> {
        self.log.record("auth");
        self.outcome.clone()
    }
}

// ── Fake ConditionPredicate ──
struct FakePredicate {
    verdict: std::result::Result<bool, PredicateError>,
    log: Arc<CallLog>,
}

impl ConditionPredicate for FakePredicate {
    fn kind(&self) -> &'static str {
        COND_KIND
    }
    fn eval(
        &self,
        _ctx: &EvalContext,
        _spec: &serde_json::Value,
    ) -> std::result::Result<bool, PredicateError> {
        self.log.record("condition");
        self.verdict.clone()
    }
}

// ── Fake Adapter ──
struct FakeAdapter {
    classify: std::result::Result<ClassifiedIntent, ClassifyError>,
    constraint: std::result::Result<bool, ConstraintError>,
    execute: Mutex<Option<std::result::Result<RawResponse, ExecError>>>,
    log: Arc<CallLog>,
}

impl FakeAdapter {
    fn classified(capability: Capability) -> ClassifiedIntent {
        ClassifiedIntent {
            capability,
            objects: vec![ObjectRef::new("obj:probe")],
        }
    }
}

#[async_trait]
impl Adapter for FakeAdapter {
    fn protocol(&self) -> &'static str {
        "fake"
    }
    fn capabilities(&self) -> &'static [Capability] {
        &[
            Capability::Observe,
            Capability::Query,
            Capability::Mutate,
            Capability::Execute,
            Capability::Manage,
            Capability::Destroy,
        ]
    }
    fn engine_enforced(&self) -> bool {
        false
    }
    fn classify(&self, _intent: &Intent) -> std::result::Result<ClassifiedIntent, ClassifyError> {
        self.log.record("classify");
        self.classify.clone()
    }
    fn check_constraint(
        &self,
        _spec: &ConstraintSpec,
        _ci: &ClassifiedIntent,
    ) -> std::result::Result<bool, ConstraintError> {
        self.log.record("check_constraint");
        self.constraint.clone()
    }
    async fn execute(
        &self,
        _ch: &mut Channel,
        _intent: &Intent,
    ) -> std::result::Result<RawResponse, ExecError> {
        self.log.record("execute");
        self.execute
            .lock()
            .unwrap()
            .take()
            .expect("execute exercised at most once")
    }
    async fn discover(
        &self,
        _ch: &mut Channel,
    ) -> std::result::Result<CapabilitySurface, DiscoverError> {
        // 数据面 kernel 永不调用 discover（控制面路径）。
        unreachable!("discover 不被数据面 kernel 触达")
    }
}

// ── Fake ConnAcquire（建连缝；绝不构造机密类型，直接交还不透明 Channel）──
struct FakeAcquire {
    outcome: Mutex<Option<std::result::Result<(), TransportError>>>,
    log: Arc<CallLog>,
}

impl FakeAcquire {
    fn ok(log: Arc<CallLog>) -> Self {
        Self {
            outcome: Mutex::new(Some(Ok(()))),
            log,
        }
    }
    fn failing(err: TransportError, log: Arc<CallLog>) -> Self {
        Self {
            outcome: Mutex::new(Some(Err(err))),
            log,
        }
    }
}

impl ConnAcquire for FakeAcquire {
    fn acquire<'a>(
        &'a self,
        _resource: &'a ResourceCode,
        _tier: &'a CredentialTier,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Channel, TransportError>> + Send + 'a>>
    {
        self.log.record("acquire");
        let outcome = self
            .outcome
            .lock()
            .unwrap()
            .take()
            .expect("acquire exercised at most once");
        Box::pin(async move {
            outcome.map(|()| Channel {
                handle: Box::new(()),
            })
        })
    }
}

// ── Fake AuditSink ──
struct FakeAudit {
    events: Mutex<Vec<(String, Option<Stage>)>>,
    log: Arc<CallLog>,
}

impl FakeAudit {
    fn ok(log: Arc<CallLog>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            log,
        }
    }
    fn recorded(&self) -> Vec<(String, Option<Stage>)> {
        self.events.lock().unwrap().clone()
    }
}

impl AuditSink for FakeAudit {
    fn record(&self, event: AuditEvent) -> std::result::Result<(), AuditError> {
        self.log.record("record");
        self.events
            .lock()
            .unwrap()
            .push((event.decision.clone(), event.stage));
        Ok(())
    }
}

// ── Fake Sanitizer（原样返回；只钉「每条出口都过 sanitize」）──
struct FakeSanitizer {
    log: Arc<CallLog>,
}

impl Sanitizer for FakeSanitizer {
    fn scrub(&self, payload: RawResponse, _declared: &[MaskRule]) -> SanitizedResponse {
        self.log.record("scrub");
        SanitizedResponse {
            payload: payload.payload,
        }
    }
    fn scrub_stream(&self, _declared: &[MaskRule]) -> Box<dyn StreamScrubber> {
        Box::new(PassthroughScrubber)
    }
}

struct PassthroughScrubber;

impl StreamScrubber for PassthroughScrubber {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        chunk.to_vec()
    }
    fn finish(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

/// 纯内存放行快照：principal `p` 在 (RESOURCE, capability) 有一个 Allow 格（挂 always 条件
/// + 一条 constraint spec）；RESOURCE 的 tier 声明承载该动词。
fn allow_snapshot(p: PrincipalId, capability: Capability, tier: &str) -> PolicySnapshot {
    let resource = ResourceCode::new(RESOURCE);
    let cell = GrantCell {
        resource: resource.clone(),
        capability,
        role: Role::new("operator"),
        action: GrantAction::Allow,
        constraints: vec![ConstraintSpec {
            kind: "object_allow".into(),
            spec: r#"{"allow":["obj:probe"]}"#.into(),
        }],
        conditions: vec![ConditionSpec {
            kind: COND_KIND.into(),
            spec: "{}".into(),
        }],
    };
    let mut per_principal = BTreeMap::new();
    per_principal.insert((resource.clone(), capability), cell);
    let mut grants = BTreeMap::new();
    grants.insert(p, per_principal);

    let mut tiers = BTreeMap::new();
    tiers.insert(
        resource.clone(),
        vec![TierDecl {
            tier: CredentialTier::new(tier),
            carries: vec![capability],
        }],
    );

    PolicySnapshot {
        policy_rev: 7,
        grants,
        tiers,
        credentials: CredentialView {
            credentials: vec![CredentialMeta {
                principal: p,
                kind: AUTH_KIND.into(),
                secret_hash: "hash".into(),
                expires_at: None,
                revoked_at: None,
            }],
        },
        deny_notes: BTreeMap::new(),
        grantable: BTreeMap::new(),
        modes: BTreeMap::new(),
    }
}

fn evaluator(auth: FakeAuth, pred: FakePredicate) -> Evaluator {
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(AUTH_KIND, Box::new(auth));
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert(COND_KIND, Box::new(pred));
    Evaluator::new(auths, preds)
}

fn request() -> NormalizedRequest {
    NormalizedRequest {
        presented: PresentedCredential::new(AUTH_KIND, b"secret".to_vec()),
        origin: unix_origin(),
        resource: ResourceCode::new(RESOURCE),
        intent: Intent::new(b"probe".to_vec()),
    }
}

/// 装配一个真实 Kernel（已装配状态）+ 共享探针 + 审计句柄（端到端场景载体）。
struct Assembled {
    kernel: Kernel,
    log: Arc<CallLog>,
    audit: Arc<FakeAudit>,
}

fn assembled(
    capability: Capability,
    auth_principal: std::result::Result<PrincipalId, AuthError>,
    predicate: std::result::Result<bool, PredicateError>,
    constraint: std::result::Result<bool, ConstraintError>,
    execute: std::result::Result<RawResponse, ExecError>,
    acquire: FakeAcquire,
) -> Assembled {
    let log = Arc::new(CallLog::default());
    let p = principal(42);
    let snapshot = Arc::new(allow_snapshot(p, capability, "readonly"));
    let auth = FakeAuth {
        outcome: auth_principal,
        log: log.clone(),
    };
    let pred = FakePredicate {
        verdict: predicate,
        log: log.clone(),
    };
    let adapter = FakeAdapter {
        classify: Ok(FakeAdapter::classified(capability)),
        constraint,
        execute: Mutex::new(Some(execute)),
        log: log.clone(),
    };
    let eval = Arc::new(evaluator(auth, pred));
    let adapters = Arc::new(postern_daemon::registry::AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>,
    ]));
    let audit = Arc::new(FakeAudit::ok(log.clone()));
    let kernel = Kernel::new(
        eval,
        adapters,
        Arc::new(acquire) as Arc<dyn ConnAcquire>,
        audit.clone() as Arc<dyn AuditSink>,
        Arc::new(FakeSanitizer { log: log.clone() }) as Arc<dyn Sanitizer>,
        snapshot,
        now(),
    );
    Assembled { kernel, log, audit }
}

/// 全过的端到端装配（read 动词，全部成功）。
fn passing_assembled() -> Assembled {
    let log = Arc::new(CallLog::default());
    assembled(
        Capability::Query,
        Ok(principal(42)),
        Ok(true),
        Ok(true),
        Ok(RawResponse {
            payload: b"rows".to_vec(),
        }),
        FakeAcquire::ok(log),
    )
}

// ════════════════════════════════════════════════════════════════════════
//  §8 场景载体：已装配 Kernel 端到端 —— 放行请求经出口脱敏回 Ok
// ════════════════════════════════════════════════════════════════════════

// §8：已装配状态（真实 Kernel + 内存 Fake 全插件）对一次放行请求 → 走完管线、出口经同一
// Sanitizer、回 Ok（SanitizedResponse）。这是「组装产物确实能跑出可观察决策」的端到端载体。
#[tokio::test]
async fn assembled_kernel_allows_and_egresses_through_sanitizer() {
    let a = passing_assembled();
    let out = a.kernel.submit(request()).await;
    assert!(out.is_ok(), "全过的放行请求 → Ok（已装配状态端到端跑通）");
    let log = a.log.snapshot();
    assert!(
        log.contains(&"execute"),
        "放行路径确已 execute（在 acquire 之后）"
    );
    assert!(
        log.contains(&"scrub"),
        "放行出口经同一 Sanitizer（F-10：统一出口脱敏）"
    );
    // read 动词单条结果痕（decision=allow）。
    assert!(
        a.audit.recorded().iter().any(|(d, _s)| d == "allow"),
        "放行 read 动词落单条 allow 结果痕（两阶段审计：read 无意图痕）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 场景载体（失败路径一等公民）：建连不可建 → stage=connect deny，execute 绝不被调用
// ════════════════════════════════════════════════════════════════════════

// §8 L-5：已装配 Kernel 在建连失败时 → Deny{stage=transport(=connect)}，execute 从未被调用。
#[tokio::test]
async fn assembled_kernel_connect_failure_denies_at_transport_stage_without_execute() {
    let log = Arc::new(CallLog::default());
    let a = assembled(
        Capability::Query,
        Ok(principal(42)),
        Ok(true),
        Ok(true),
        Ok(RawResponse {
            payload: b"unused".to_vec(),
        }),
        FakeAcquire::failing(TransportError::ConnectFailed, log.clone()),
    );
    let out = a.kernel.submit(request()).await;
    let deny = match out {
        Err(deny) => deny,
        Ok(_) => panic!("建连不可建 → 必 deny"),
    };
    assert_eq!(deny.decision, "deny", "建连失败 → 普通 deny");
    assert!(
        !a.log.snapshot().contains(&"execute"),
        "建连不可建即 deny：execute 绝不被调用（L-5）"
    );
    // 承重断言（区别 transport-deny 与 escalate_denied）：DenyResponse.decision 字段恒为常量
    // "deny"（deny.rs::assemble 硬置），与产生它的 stage/路径无关，故对它断言无鉴别力——真正
    // 携带「这是哪种 deny」的是审计事件的 decision 词（pipeline.rs::deny_event 写入 escalate_denied
    // vs deny）。建连失败必落「Transport 阶 + 普通 deny 词」：既验 stage=connect，又验该 deny **不是**
    // escalate 误折叠（若 escalate 被错路由到 transport-deny，此处审计词将是 escalate_denied → 红）。
    let transport_deny = a
        .audit
        .recorded()
        .into_iter()
        .find(|(_d, s)| *s == Some(Stage::Transport));
    let (transport_word, _stage) =
        transport_deny.expect("建连失败 deny 审计 stage 必为 Transport（= connect 拒绝阶）");
    assert_eq!(
        transport_word, "deny",
        "建连失败 deny 审计词必为普通 deny，绝非 escalate_denied（捕捉 escalate 误折叠到 transport 的回归）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 场景载体（失败路径）：认证失败 → stage=auth deny，绝不建连/执行
// ════════════════════════════════════════════════════════════════════════

// §8 L-5：已装配 Kernel 在认证失败时 → Deny{stage=auth}，绝不建连、绝不 execute（最早短路）。
#[tokio::test]
async fn assembled_kernel_auth_failure_denies_at_auth_stage() {
    let log = Arc::new(CallLog::default());
    let a = assembled(
        Capability::Query,
        Err(AuthError::InvalidCredential),
        Ok(true),
        Ok(true),
        Ok(RawResponse {
            payload: b"unused".to_vec(),
        }),
        FakeAcquire::ok(log.clone()),
    );
    let out = a.kernel.submit(request()).await;
    assert!(out.is_err(), "认证失败 → deny");
    let snapshot = a.log.snapshot();
    assert!(!snapshot.contains(&"acquire"), "认证失败：绝不建连");
    assert!(!snapshot.contains(&"execute"), "认证失败：绝不 execute");
    assert!(
        a.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(Stage::Auth)),
        "认证失败 deny 审计 stage 必为 Auth"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  决策类型形状锚点（先于实现成立）：Allow 携带 grant+tier、Deny 携带结构化响应
// ════════════════════════════════════════════════════════════════════════

// 决策类型形状锚点：kernel 据 Decision 三值分流出口（Allow→建连执行、Deny→结构化 deny）。
#[test]
fn decision_shape_anchor() {
    fn is_allow(d: &Decision) -> bool {
        matches!(d, Decision::Allow { .. })
    }
    fn is_deny(d: &Decision) -> bool {
        matches!(d, Decision::Deny(_))
    }
    let _ = (
        is_allow as fn(&Decision) -> bool,
        is_deny as fn(&Decision) -> bool,
    );
    // 保留 DenyResponse / BootError 命名引用（避免 dead import 警告）。
    let _shape = |d: &DenyResponse| d.decision;
    let _boot: fn(BootStage) -> BootError = BootError::at;
}
