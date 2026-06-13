//! boot 单元行为测试（RED）。
//!
//! 钉死启动序列子域（模块文档 06 §3.1 / §8 F-1·F-2·L-1·B-2）：单线程顺序启动链
//! 「开库+校验 → 重建首份 `Arc<PolicySnapshot>` → 解锁保险箱+lifecycle 审计 → 注册插件 →
//! 先 control.sock(0600) 后 data.sock(0660/组)挂数据面 router」；**依赖顺序即安全顺序**，
//! 任一前置不成立 → 进程非零退出、data.sock 未创建、数据面未开放（公理二 fail-closed）。
//!
//! 驱动方式（06 §9 boot 测试策略：fail-closed 分支靠**前置条件可注入**）：把装配链的四个
//! 前置（开库/迁移/首快照/解锁）抽象为 Fake [`Preconditions`]、两平面 socket 创建抽象为 Fake
//! [`SocketFactory`]、data.sock 可连 uid 集合抽象为 Fake [`ConnectableUidProbe`]。分别注入
//! 「某前置失败」「socket 创建早于装配 / 失败」「data.sock 可连 uid 含自身 uid」，断言**恰为**
//! 「进程非零退出（`Err(BootError{stage})`）+ data.sock 未创建 + 数据面未开放」。
//!
//! 失败路径一等公民：每条只钉一个行为，断言精确到失败 [`BootStage`]、socket 创建调用序、
//! data.sock 是否存在、注入集合成员。happy 路径断言整链顺序恰为 §3.1 六步、data.sock 是
//! 链的**唯一终结动作**（single terminal action）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（开库/迁移/首快照全经 store，boot 不碰 SQL）；需要请求
//! 来源时以 `use postern_core::request::ConnOrigin as Origin` 别名读/构造（测试在 shells 外，
//! 绝不写字面 `ConnOrigin::` 变体）；**绝不构造** `ResolvedTarget`/`ResourceCredential`
//! （boot 只解锁保险箱并建 ScrubSet，凭据/地址物化在 connpool 请求期，不在 boot）。
//!
//! 实现为 RED 桩（`Bootstrap::run` / `connectable_uid_check` / socket 原语体为 `todo!()`），
//! 故凡驱动启动链的测试调用即 panic → 观察到红。纯类型/常量层断言（socket 权限常量、
//! `SelfCheck` 语义、`stage_of` 穷尽、`BootError` 归因）先于实现成立则单独标注，验编排正确。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// 测试在 shells 外：需要请求来源时以别名读/构造，绝不写字面 ConnOrigin:: 变体（雷区 / B-2）。
use postern_core::request::ConnOrigin as Origin;

use postern_core::error::Stage;

use postern_daemon::boot::selfcheck::{connectable_uid_check, SelfCheck};
use postern_daemon::boot::sockets::{
    bind_then_secure_then_listen, SockPerms, SocketEffects, SocketSubStep, CONTROL_PERMS,
    DATA_PERMS,
};
use postern_daemon::boot::{
    stage_of, BootError, BootReport, BootStage, Bootstrap, ConnectableUidProbe, HandleKind,
    Preconditions, RouterAssembler, SocketFactory,
};
use postern_daemon::error::{DaemonError, Result};

// ════════════════════════════════════════════════════════════════════════
//  注入开关：每个前置/socket 步骤的成功/失败配置（前置条件可注入，06 §9）
// ════════════════════════════════════════════════════════════════════════

/// 哪个启动步骤被注入失败（`None` = 全成功 happy 路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailAt {
    OpenDb,
    Migrate,
    FirstSnapshot,
    UnlockVault,
    ControlSocket,
    DataSocket,
}

// ════════════════════════════════════════════════════════════════════════
//  Fake Preconditions：四前置（开库/迁移/首快照/解锁）按序记录调用，可在任一步注入失败
// ════════════════════════════════════════════════════════════════════════

/// 记录四前置实际被调用的有序序列（验「依赖顺序即安全顺序」与失败处短路）。
struct FakePreconditions {
    fail_at: Option<FailAt>,
    /// 调用序记录（每步压入其 `BootStage`）。
    calls: Arc<Mutex<Vec<BootStage>>>,
}

impl FakePreconditions {
    fn new(fail_at: Option<FailAt>) -> Self {
        Self {
            fail_at,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn ok() -> Self {
        Self::new(None)
    }
    fn record(&self, stage: BootStage) {
        self.calls.lock().unwrap().push(stage);
    }
}

impl Preconditions for FakePreconditions {
    fn open_db(&self) -> Result<()> {
        self.record(BootStage::OpenDb);
        if self.fail_at == Some(FailAt::OpenDb) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
    fn migrate(&self) -> Result<()> {
        self.record(BootStage::Migrate);
        if self.fail_at == Some(FailAt::Migrate) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
    fn rebuild_first_snapshot(&self) -> Result<()> {
        self.record(BootStage::FirstSnapshot);
        if self.fail_at == Some(FailAt::FirstSnapshot) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
    fn unlock_vault(&self) -> Result<()> {
        self.record(BootStage::UnlockVault);
        if self.fail_at == Some(FailAt::UnlockVault) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake SocketFactory：记录 control/data 创建调用序，可在任一 socket 注入失败
// ════════════════════════════════════════════════════════════════════════

/// 记录 socket 创建调用序（验「control 早于 data」「早失败时 data 从未创建」）。
struct FakeSockets {
    fail_at: Option<FailAt>,
    /// 创建调用序记录（control / data 各压入其 `BootStage`）。
    created: Arc<Mutex<Vec<BootStage>>>,
}

impl FakeSockets {
    fn new(fail_at: Option<FailAt>) -> Self {
        Self {
            fail_at,
            created: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn ok() -> Self {
        Self::new(None)
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
        if self.fail_at == Some(FailAt::DataSocket) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake ConnectableUidProbe：可配置 data.sock 可连 uid 有效集合 + 自身 uid（F-2）
// ════════════════════════════════════════════════════════════════════════

/// data.sock 可连 uid 探针：自身 uid 固定，可连集合按测试场景注入。
struct FakeProbe {
    self_uid: u32,
    connectable: Vec<u32>,
    /// `connectable_uids` 被调用次数（验自检在 data.sock 创建前真正执行了一次）。
    probes: Arc<AtomicUsize>,
}

impl FakeProbe {
    fn new(self_uid: u32, connectable: Vec<u32>) -> Self {
        Self {
            self_uid,
            connectable,
            probes: Arc::new(AtomicUsize::new(0)),
        }
    }
    /// 可连集合**不含**自身 uid（正常路径：data.sock 可正常开放）。
    fn safe(self_uid: u32) -> Self {
        // Agent 用一个**不同于** daemon 自身的 uid 连接（专用组放行的他者 uid）。
        Self::new(self_uid, vec![self_uid.wrapping_add(1000)])
    }
    /// 可连集合**含**自身 uid（同 uid 危险态：必须 fail-closed 拒绝启动）。
    fn same_uid(self_uid: u32) -> Self {
        Self::new(self_uid, vec![self_uid.wrapping_add(1000), self_uid])
    }
}

impl ConnectableUidProbe for FakeProbe {
    fn self_uid(&self) -> u32 {
        self.self_uid
    }
    fn connectable_uids(&self) -> Vec<u32> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        self.connectable.clone()
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake RouterAssembler：记录两平面 router **实际收到**的句柄集（B-2/L-14 的可观察接线点）
// ════════════════════════════════════════════════════════════════════════

/// 记录两平面 router 装配时实际被注入的句柄集——断言读的是「router 真正收到了什么」，
/// 而非 run() 另写的一份字面 vec。若实现把 PolicyRepo 误注入数据面 router，`data` 必含它 → 红。
struct FakeAssembler {
    /// 数据面 router 实际收到的句柄集（红线 7.2-2：绝不含 PolicyRepo）。
    data: Arc<Mutex<Vec<HandleKind>>>,
    /// 控制面/清扫器 router 实际收到的句柄集（PolicyRepo 写句柄只在此）。
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

// ════════════════════════════════════════════════════════════════════════
//  Fake SocketEffects：按调用顺序记录单 socket 的 bind/secure/listen 三子步（L-1 原子序）
// ════════════════════════════════════════════════════════════════════════

/// 记录 `bind_then_secure_then_listen` 实际派发的子步顺序——见证「secure 恒先于 listen」
/// （消除 umask 竞态窗口）。可在任一子步注入失败，验任一子步 Err 即短路、不进入后续子步。
struct FakeEffects {
    /// 在哪个子步注入失败（`None` = 全成功）。
    fail_at: Option<SocketSubStep>,
    /// 子步调用序记录（bind/secure/listen 各压入其标识）。
    steps: Arc<Mutex<Vec<SocketSubStep>>>,
}

impl FakeEffects {
    fn new(fail_at: Option<SocketSubStep>) -> Self {
        Self {
            fail_at,
            steps: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn ok() -> Self {
        Self::new(None)
    }
}

impl SocketEffects for FakeEffects {
    fn bind(&self) -> Result<()> {
        self.steps.lock().unwrap().push(SocketSubStep::Bind);
        if self.fail_at == Some(SocketSubStep::Bind) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
    fn secure(&self, _perms: SockPerms) -> Result<()> {
        self.steps.lock().unwrap().push(SocketSubStep::Secure);
        if self.fail_at == Some(SocketSubStep::Secure) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
    fn listen(&self) -> Result<()> {
        self.steps.lock().unwrap().push(SocketSubStep::Listen);
        if self.fail_at == Some(SocketSubStep::Listen) {
            return Err(DaemonError::Boot);
        }
        Ok(())
    }
}

// 固定 daemon 自身 uid 样本（非 0，避免与 root 特例混淆）。
const SELF_UID: u32 = 1000;

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
//  §8 F-1：happy 路径——启动链顺序恰为 §3.1 六步，data.sock 是唯一终结动作
// ════════════════════════════════════════════════════════════════════════

// §8 F-1：全前置成功 + 可连 uid 不含自身 → 启动链跑完，data.sock 最后创建、数据面开放。
// 触达 Bootstrap::run（todo!()）即 panic → 红。
#[test]
fn happy_path_opens_data_plane_after_full_assembly() {
    let boot = bootstrap_happy();
    let report: BootReport = boot.run().expect("全前置成功时启动链应跑完并开放数据面");

    // 数据面在装配全部就绪后开放（F-1）。
    assert!(
        report.data_plane_open,
        "全前置成功时数据面必须开放（data.sock 已创建并挂数据面 router）"
    );
    // 执行序恰为 §3.1 顺序：开库 → 迁移 → 首快照 → 解锁 → 注册插件 → control.sock →
    // 可连 uid 自检 → data.sock（终结动作）。
    assert_eq!(
        report.executed,
        vec![
            BootStage::OpenDb,
            BootStage::Migrate,
            BootStage::FirstSnapshot,
            BootStage::UnlockVault,
            BootStage::RegisterPlugins,
            BootStage::ControlSocket,
            BootStage::ConnectableUidSelfCheck,
            BootStage::DataSocket,
        ],
        "启动链顺序必须恰为 §3.1 六步链（依赖顺序即安全顺序），data.sock 排在最后"
    );
}

// §8 F-1（single terminal action）：data.sock 创建是整链的**最后一步**——执行序里 DataSocket
// 必须是末元素，且其后再无任何步骤（杜绝「先开门再装锁」）。
#[test]
fn data_socket_is_the_single_terminal_action() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy 路径应跑完");
    assert_eq!(
        report.executed.last(),
        Some(&BootStage::DataSocket),
        "data.sock 创建必须是启动链唯一收尾动作（终结于此，其后无步骤）"
    );
    // data.sock 之前 control.sock 必须已创建（先 control 后 data，L-1）。
    let pos_ctrl = report
        .executed
        .iter()
        .position(|s| *s == BootStage::ControlSocket)
        .expect("control.sock 必须在链中创建");
    let pos_data = report
        .executed
        .iter()
        .position(|s| *s == BootStage::DataSocket)
        .expect("data.sock 必须在链中创建");
    assert!(
        pos_ctrl < pos_data,
        "control.sock 必须先于 data.sock 创建（L-1 时序）"
    );
}

// §8 F-2：可连 uid 自检必须排在 data.sock **创建之前**——执行序里自检步紧邻且早于 DataSocket。
#[test]
fn self_check_runs_before_data_socket_creation() {
    let boot = bootstrap_happy();
    let report = boot.run().expect("happy 路径应跑完");
    let pos_check = report
        .executed
        .iter()
        .position(|s| *s == BootStage::ConnectableUidSelfCheck)
        .expect("可连 uid 自检必须在链中执行");
    let pos_data = report
        .executed
        .iter()
        .position(|s| *s == BootStage::DataSocket)
        .expect("data.sock 必须在链中创建");
    assert!(
        pos_check < pos_data,
        "可连 uid 自检必须在 data.sock 创建之前执行（F-2：含自身即开放前 fail-closed）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-1：四前置任一失败 → 进程非零退出（Err）、data.sock 未创建、数据面未开放
// ════════════════════════════════════════════════════════════════════════

// §8 F-1：开库失败 → 短路在 OpenDb，data.sock 从未创建、数据面未开放。
#[test]
fn open_db_failure_aborts_before_any_socket() {
    let pre = FakePreconditions::new(Some(FailAt::OpenDb));
    let sockets = FakeSockets::ok();
    let data_created = sockets.created.clone();
    let boot = Bootstrap::new(
        pre,
        sockets,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );

    match boot.run() {
        Ok(_) => panic!("开库失败时启动链绝不应成功（fail-closed）"),
        Err(e) => assert_eq!(
            e,
            BootError::at(BootStage::OpenDb),
            "开库失败必须短路并归因到 OpenDb"
        ),
    }
    // data.sock 从未创建（短路在 socket 创建之前）。
    assert!(
        !data_created
            .lock()
            .unwrap()
            .contains(&BootStage::DataSocket),
        "开库失败时 data.sock 绝不应被创建（数据面未开放）"
    );
}

// §8 F-1：迁移版本校验失败 → 短路在 Migrate，data.sock 未创建。
#[test]
fn migrate_failure_aborts_before_data_socket() {
    let pre = FakePreconditions::new(Some(FailAt::Migrate));
    let sockets = FakeSockets::ok();
    let probe_data = sockets.created.clone();
    let boot = Bootstrap::new(
        pre,
        sockets,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );

    match boot.run() {
        Ok(_) => panic!("迁移校验失败时启动链绝不应成功（fail-closed）"),
        Err(e) => assert_eq!(
            e,
            BootError::at(BootStage::Migrate),
            "迁移失败归因到 Migrate"
        ),
    }
    assert!(
        !probe_data.lock().unwrap().contains(&BootStage::DataSocket),
        "迁移失败时 data.sock 绝不应被创建"
    );
}

// §8 F-1：首快照重建失败 → 短路在 FirstSnapshot，data.sock 未创建、数据面未开放。
#[test]
fn first_snapshot_failure_aborts_before_data_socket() {
    let pre = FakePreconditions::new(Some(FailAt::FirstSnapshot));
    let sockets = FakeSockets::ok();
    let created = sockets.created.clone();
    let boot = Bootstrap::new(
        pre,
        sockets,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );

    match boot.run() {
        Ok(_) => panic!("首快照重建失败时启动链绝不应成功（fail-closed）"),
        Err(e) => assert_eq!(
            e,
            BootError::at(BootStage::FirstSnapshot),
            "首快照失败归因到 FirstSnapshot"
        ),
    }
    assert!(
        !created.lock().unwrap().contains(&BootStage::DataSocket),
        "首快照失败时 data.sock 绝不应被创建"
    );
}

// §8 F-1：保险箱解锁失败 → 短路在 UnlockVault，data.sock 未创建、数据面未开放。
#[test]
fn vault_unlock_failure_aborts_before_data_socket() {
    let pre = FakePreconditions::new(Some(FailAt::UnlockVault));
    let sockets = FakeSockets::ok();
    let created = sockets.created.clone();
    let boot = Bootstrap::new(
        pre,
        sockets,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );

    match boot.run() {
        Ok(_) => panic!("保险箱解锁失败时启动链绝不应成功（fail-closed）"),
        Err(e) => assert_eq!(
            e,
            BootError::at(BootStage::UnlockVault),
            "解锁失败归因到 UnlockVault"
        ),
    }
    assert!(
        !created.lock().unwrap().contains(&BootStage::DataSocket),
        "解锁失败时 data.sock 绝不应被创建（数据面未开放）"
    );
}

// §8 F-1（依赖顺序即安全顺序）：前置失败时**绝不执行后续前置**——开库失败后，迁移/首快照/
// 解锁都不应被调用（短路语义，不是「跑完全部再判」）。
#[test]
fn precondition_failure_short_circuits_remaining_steps() {
    let pre = FakePreconditions::new(Some(FailAt::OpenDb));
    let calls = pre.calls.clone();
    let boot = Bootstrap::new(
        pre,
        FakeSockets::ok(),
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );

    let _ = boot.run();

    let executed = calls.lock().unwrap().clone();
    // 仅 OpenDb 被调用；其后的 Migrate/FirstSnapshot/UnlockVault 全部未触达（短路）。
    assert_eq!(
        executed,
        vec![BootStage::OpenDb],
        "开库失败必须短路：其后的迁移/首快照/解锁前置绝不执行（依赖顺序即安全顺序）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-2：data.sock 可连 uid 含自身 → fail-closed 拒绝启动；不含自身 → 正常开放
// ════════════════════════════════════════════════════════════════════════

// §8 F-2：可连 uid 集合**含 daemon 自身 uid**（Agent 与 daemon 同 uid）→ 启动被拒
// （Err，归因到 ConnectableUidSelfCheck）、data.sock 未创建、数据面未开放。
#[test]
fn connectable_uids_containing_self_refuses_startup() {
    let pre = FakePreconditions::ok();
    let sockets = FakeSockets::ok();
    let created = sockets.created.clone();
    // 可连集合含自身 uid（危险态）。
    let boot = Bootstrap::new(
        pre,
        sockets,
        FakeProbe::same_uid(SELF_UID),
        FakeAssembler::new(),
    );

    match boot.run() {
        Ok(_) => panic!("可连 uid 含自身时绝不应开放数据面（同 uid 即拒启动，F-2）"),
        Err(e) => assert_eq!(
            e,
            BootError::at(BootStage::ConnectableUidSelfCheck),
            "同 uid 自检失败必须归因到 ConnectableUidSelfCheck"
        ),
    }
    // data.sock 在自检之后才创建：自检拒绝 → data.sock 从未创建。
    assert!(
        !created.lock().unwrap().contains(&BootStage::DataSocket),
        "可连 uid 含自身时 data.sock 绝不应被创建（自检在创建前 fail-closed，F-2）"
    );
}

// §8 F-2（对偶）：可连 uid 集合**不含**自身 uid（Agent 与 daemon 不同 uid，专用组放行他者）→
// 自检通过、data.sock 正常开放、数据面开放。
#[test]
fn connectable_uids_without_self_opens_normally() {
    let boot = bootstrap_happy(); // safe probe：可连集合不含自身 uid
    let report = boot.run().expect("可连 uid 不含自身时应正常开放数据面");
    assert!(
        report.data_plane_open,
        "可连 uid 不含自身 uid 时数据面必须正常开放（F-2 正常路径）"
    );
    assert!(
        report.executed.contains(&BootStage::DataSocket),
        "可连 uid 不含自身时 data.sock 必须被创建"
    );
}

// §8 F-2（判定形态，纯函数层）：自检是**有效集合**成员测——含自身 → RefuseSameUid，
// 不含 → Pass。触达 connectable_uid_check（todo!()）即红。
#[test]
fn self_check_refuses_when_set_contains_self_uid() {
    // 集合含自身 uid → 拒绝（同 uid 危险态）。
    let with_self = connectable_uid_check(SELF_UID, &[SELF_UID + 1, SELF_UID]);
    assert_eq!(
        with_self,
        SelfCheck::RefuseSameUid,
        "可连集合含自身 uid → 自检必须 RefuseSameUid（F-2）"
    );
    assert!(!with_self.is_pass(), "含自身 uid 时自检绝不放行");

    // 集合不含自身 uid → 放行（他者 uid 经专用组连接）。
    let without_self = connectable_uid_check(SELF_UID, &[SELF_UID + 1, SELF_UID + 2]);
    assert_eq!(
        without_self,
        SelfCheck::Pass,
        "可连集合不含自身 uid → 自检 Pass（F-2 正常路径）"
    );
    assert!(without_self.is_pass(), "不含自身 uid 时自检放行");
}

// §8 F-2：自检在 data.sock 开放前**真正探测了可连集合**（不是读自报字段）——
// happy 路径里探针的 connectable_uids 至少被调用一次。
#[test]
fn self_check_actually_probes_effective_uid_set() {
    let probe = FakeProbe::safe(SELF_UID);
    let probes = probe.probes.clone();
    let boot = Bootstrap::new(
        FakePreconditions::ok(),
        FakeSockets::ok(),
        probe,
        FakeAssembler::new(),
    );

    let _ = boot.run().expect("happy 路径应跑完");
    assert!(
        probes.load(Ordering::SeqCst) >= 1,
        "可连 uid 自检必须真正探测有效集合（调用 connectable_uids），而非读请求自报字段（F-2）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-1：两平面 socket 创建次序——先 control 后 data；control socket 失败 → data 不创建
// ════════════════════════════════════════════════════════════════════════

// §8 L-1：control.sock 创建失败 → 短路在 ControlSocket，data.sock 绝不创建（先 control 后 data，
// control 不成则 data 不开）。
#[test]
fn control_socket_failure_means_data_socket_never_created() {
    let pre = FakePreconditions::ok();
    let sockets = FakeSockets::new(Some(FailAt::ControlSocket));
    let order = sockets.created.clone();
    let boot = Bootstrap::new(
        pre,
        sockets,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );

    match boot.run() {
        Ok(_) => panic!("control.sock 创建失败时绝不应继续开放 data.sock（fail-closed）"),
        Err(e) => assert_eq!(
            e,
            BootError::at(BootStage::ControlSocket),
            "control.sock 失败归因到 ControlSocket"
        ),
    }
    let created = order.lock().unwrap().clone();
    assert!(
        created.contains(&BootStage::ControlSocket),
        "control.sock 创建被尝试过（失败）"
    );
    assert!(
        !created.contains(&BootStage::DataSocket),
        "control.sock 失败时 data.sock 绝不应被创建（先 control 后 data，L-1）"
    );
}

// §8 F-1（终结动作失败 → 数据面未开放）：data.sock 创建步骤**自身**失败（FakeSockets 注入
// FailAt::DataSocket）→ 进程非零退出（Err 归因到 DataSocket）、数据面未开放。这是整链唯一
// 终结动作的失败边界——若实现回归为「先置 data_plane_open 再 create_data」（先开门再装锁），
// data.sock 创建失败时 run() 仍须返回 Err、绝不返回 data_plane_open=true 的成功报告。
#[test]
fn data_socket_creation_failure_aborts_with_non_zero_exit() {
    let pre = FakePreconditions::ok();
    let sockets = FakeSockets::new(Some(FailAt::DataSocket));
    let boot = Bootstrap::new(
        pre,
        sockets,
        FakeProbe::safe(SELF_UID),
        FakeAssembler::new(),
    );

    match boot.run() {
        // 终结动作失败时绝不应返回成功报告（无论 data_plane_open 取值）——必须 Err 退出。
        Ok(report) => panic!(
            "data.sock 创建失败时 run() 绝不应成功返回（fail-closed）；得到 data_plane_open={}",
            report.data_plane_open
        ),
        // 失败必须归因到 DataSocket 这一终结步骤（非零退出，§8 F-1）。
        Err(e) => assert_eq!(
            e,
            BootError::at(BootStage::DataSocket),
            "data.sock 创建失败必须归因到 DataSocket（终结动作失败 → 数据面未开放）"
        ),
    }
}

// §8 L-1：两平面权限隔离常量——control.sock 恰 0600（仅属主、不设组）、data.sock 恰 0660（设专用组）。
// 常量层，先于实现可绿，钉死权限隔离不被偷改。
#[test]
fn socket_perms_constants_isolate_two_planes() {
    assert_eq!(
        CONTROL_PERMS.mode, 0o600,
        "control.sock 模式位必须恰为 0600（仅属主）"
    );
    const {
        assert!(
            !CONTROL_PERMS.set_group,
            "control.sock 不设专用组（仅属主可达）"
        )
    };
    assert_eq!(DATA_PERMS.mode, 0o660, "data.sock 模式位必须恰为 0660");
    const {
        assert!(
            DATA_PERMS.set_group,
            "data.sock 设专用组（Agent 经专用组放行）"
        )
    };
    // 两平面权限不同（隔离不被抹平）。
    assert_ne!(
        CONTROL_PERMS, DATA_PERMS,
        "两平面 socket 权限必须不同（L-1 权限隔离）"
    );
}

// §8 L-1（权限类型存在性）：SockPerms 携带 mode + set_group 两字段（权限+属组两维度），
// 类型层即表达 control/data 的权限差异。
#[test]
fn sock_perms_carries_mode_and_group() {
    let p = SockPerms {
        mode: 0o600,
        set_group: false,
    };
    assert_eq!(p.mode, 0o600);
    assert!(!p.set_group);
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-1（no umask race window）：单 socket 内 bind → secure(chmod/设组) → listen 原子序
// ════════════════════════════════════════════════════════════════════════

// §8 L-1（消除 TOCTOU 竞态窗口）：bind_then_secure_then_listen 必须按 bind → secure → listen
// 派发三子步——secure（chmod/设属组）**恒先于** listen。若实现把 listen 排在 secure 之前
// （bind 后到 chmod 前的默认 umask 可连窗口），此断言变红。这正是承载竞态窗口消除的唯一单元。
#[test]
fn bind_secures_before_listen_no_umask_race_window() {
    let eff = FakeEffects::ok();
    let steps = eff.steps.clone();
    bind_then_secure_then_listen(&eff, DATA_PERMS).expect("全子步成功时原子序应跑完");

    let order = steps.lock().unwrap().clone();
    // 恰为三子步、恰按 bind → secure → listen 次序（顺序即安全不变量）。
    assert_eq!(
        order,
        vec![
            SocketSubStep::Bind,
            SocketSubStep::Secure,
            SocketSubStep::Listen,
        ],
        "单 socket 创建必须恰按 bind → secure(chmod/设组) → listen 原子序（无 umask 竞态窗口，L-1）"
    );
    // 显式钉死 secure 早于 listen（即便上面整体相等改写，此关键不等式也独立成立）。
    let pos_secure = order
        .iter()
        .position(|s| *s == SocketSubStep::Secure)
        .expect("secure 必须被派发");
    let pos_listen = order
        .iter()
        .position(|s| *s == SocketSubStep::Listen)
        .expect("listen 必须被派发");
    assert!(
        pos_secure < pos_listen,
        "chmod/设属组（secure）必须在 listen 之前——否则 bind 后存在默认 umask 下短暂可连窗口（L-1）"
    );
}

// §8 L-1（fail-closed 子步短路）：secure（chmod/设属组）失败 → 原子序短路，**绝不**进入 listen
// （绝不带未收紧权限去 accept 连接）。
#[test]
fn secure_failure_short_circuits_before_listen() {
    let eff = FakeEffects::new(Some(SocketSubStep::Secure));
    let steps = eff.steps.clone();

    let result = bind_then_secure_then_listen(&eff, DATA_PERMS);
    assert!(
        result.is_err(),
        "secure 失败时原子序必须 fail-closed 返回 Err"
    );

    let order = steps.lock().unwrap().clone();
    assert!(
        order.contains(&SocketSubStep::Secure),
        "secure 被尝试过（并失败）"
    );
    assert!(
        !order.contains(&SocketSubStep::Listen),
        "secure 失败时 listen 绝不应被派发（绝不带未收紧权限去 listen，L-1 fail-closed）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 B-2 / L-14：PolicyRepo 句柄只进 control/sweeper，绝不进数据面 router 注入集合
// ════════════════════════════════════════════════════════════════════════

// §8 B-2 / L-14（红线 7.2-2）：boot 装配后，PolicyRepo 写句柄**只**在控制面/清扫器注入集合，
// **绝不**出现在数据面 router 的注入集合里。
#[test]
fn policy_repo_handle_absent_from_data_plane_injection_set() {
    // 持有 assembler 的两平面记录 Arc：断言读「数据面 router 实际收到了什么」（真接线点），
    // 不止读 run() 写进 report 的字面 vec——见证红线 7.2-2 而非空转（assertion-1 修复核心）。
    let assembler = FakeAssembler::new();
    let data_received = assembler.data.clone();
    let control_received = assembler.control.clone();
    let boot = Bootstrap::new(
        FakePreconditions::ok(),
        FakeSockets::ok(),
        FakeProbe::safe(SELF_UID),
        assembler,
    );
    let report = boot.run().expect("happy 路径应跑完");

    // 【真断言】数据面 router **实际收到的**句柄集绝不含 PolicyRepo（红线 7.2-2）。
    // 任何把 PolicyRepo 注入数据面 router 的实现，assemble_data_plane 必收到它 → 此断言红。
    assert!(
        !data_received
            .lock()
            .unwrap()
            .contains(&HandleKind::PolicyRepo),
        "PolicyRepo 写句柄绝不进数据面 router（assemble_data_plane 收到的句柄集不含它，红线 7.2-2）"
    );
    // 【真断言】控制面/清扫器 router **实际收到**了 PolicyRepo 写句柄（句柄不悬空、只此一处）。
    assert!(
        control_received
            .lock()
            .unwrap()
            .contains(&HandleKind::PolicyRepo),
        "PolicyRepo 写句柄必须进控制面/清扫器 router（assemble_control_plane 收到它）"
    );
    // report 与 router 实际收到的逐元素一致（报告不得与接线点背离）。
    assert_eq!(
        report.data_plane_handles,
        *data_received.lock().unwrap(),
        "数据面 report 句柄集必须恰等于 router 实际收到的集合（报告不与接线点背离）"
    );
    assert_eq!(
        report.control_plane_handles,
        *control_received.lock().unwrap(),
        "控制面 report 句柄集必须恰等于 router 实际收到的集合（报告不与接线点背离）"
    );
    // 兼容旧断言（只增不减）：报告侧亦不含/含 PolicyRepo。
    assert!(
        !report.data_plane_handles.contains(&HandleKind::PolicyRepo),
        "PolicyRepo 写句柄绝不进数据面 router 注入集合（红线 7.2-2 / B-2 / L-14）"
    );
    assert!(
        report
            .control_plane_handles
            .contains(&HandleKind::PolicyRepo),
        "PolicyRepo 写句柄必须进控制面/清扫器注入集合（句柄不悬空）"
    );
}

// §8 B-2 / L-14：数据面注入集合只含数据面所需句柄（PolicyView 只读投影 + 连接池 + Sanitizer +
// 登记册），不含任何写句柄——证明数据面经 PolicyView::snapshot 消费只读投影，而非持写句柄。
#[test]
fn data_plane_injection_set_is_read_only_view_not_write_handle() {
    let assembler = FakeAssembler::new();
    let data_received = assembler.data.clone();
    let boot = Bootstrap::new(
        FakePreconditions::ok(),
        FakeSockets::ok(),
        FakeProbe::safe(SELF_UID),
        assembler,
    );
    let report = boot.run().expect("happy 路径应跑完");

    // 【真断言】数据面 router **实际收到**的句柄集是只读投影集（含 PolicyView，不含 PolicyRepo）。
    let data = data_received.lock().unwrap().clone();
    assert!(
        data.contains(&HandleKind::PolicyView),
        "数据面 router 必须经 PolicyView 只读投影消费策略（assemble_data_plane 收到 PolicyView）"
    );
    assert!(
        !data.contains(&HandleKind::PolicyRepo),
        "数据面 router 收到的句柄集绝不含 PolicyRepo 写句柄（只读投影，非写句柄，红线 7.2-2）"
    );
    // 兼容旧断言（只增不减）：报告侧亦含 PolicyView、不含 PolicyRepo。
    assert!(
        report.data_plane_handles.contains(&HandleKind::PolicyView),
        "数据面必须经 PolicyView 只读投影消费策略（注入集含 PolicyView）"
    );
    assert!(
        !report.data_plane_handles.contains(&HandleKind::PolicyRepo),
        "数据面注入集绝不含 PolicyRepo 写句柄（只读投影，非写句柄）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  归因层（纯函数 / 类型）：BootStage→Stage 穷尽映射、BootError 归因（编排正确性）
// ════════════════════════════════════════════════════════════════════════

// stage_of 对每个 BootStage 都有显式归因（无 _ => 兜底）——逐一调用确保全枚举可映射，
// 且建连相关步骤折叠到 connect 阶段（Stage::Transport）。先于实现可绿（纯 match）。
#[test]
fn stage_of_maps_every_boot_stage() {
    // 每个 BootStage 都能映射（穷尽，无 panic）。
    for stage in [
        BootStage::OpenDb,
        BootStage::Migrate,
        BootStage::FirstSnapshot,
        BootStage::UnlockVault,
        BootStage::RegisterPlugins,
        BootStage::ControlSocket,
        BootStage::ConnectableUidSelfCheck,
        BootStage::DataSocket,
    ] {
        let _s: Stage = stage_of(stage);
    }
    // 自检/socket 步骤归 connect 阶段（建连面），data.sock 自检同理。
    assert_eq!(
        stage_of(BootStage::ConnectableUidSelfCheck),
        Stage::Transport,
        "可连 uid 自检失败归 connect 阶段（Stage::Transport）"
    );
    assert_eq!(
        stage_of(BootStage::DataSocket),
        Stage::Transport,
        "data.sock 创建失败归 connect 阶段"
    );
}

// BootError 归因：携带触发短路的 BootStage，且相等性按 stage 区分（不同步骤的失败可区分）。
#[test]
fn boot_error_attributes_failing_stage() {
    let e_open = BootError::at(BootStage::OpenDb);
    let e_vault = BootError::at(BootStage::UnlockVault);
    assert_eq!(e_open.stage, BootStage::OpenDb, "BootError 携带失败步骤");
    assert_ne!(
        e_open, e_vault,
        "不同步骤的启动失败必须可区分（按 stage 归因）"
    );
}

// BootReport 默认值即「未开放数据面」（fail-closed 默认：构造空报告时数据面恒未开放）。
#[test]
fn boot_report_default_is_data_plane_closed() {
    let report = BootReport::default();
    assert!(
        !report.data_plane_open,
        "BootReport 默认值必须是数据面未开放（fail-closed 默认）"
    );
    assert!(report.executed.is_empty(), "默认报告无已执行步骤");
}

// ════════════════════════════════════════════════════════════════════════
//  雷区见证：boot 测试在 shells 外——以 Origin 别名引用请求来源类型，绝不写字面 ConnOrigin::
//  变体（B-2 构造点唯一在 shells/）。此处仅作类型存在性引用，boot 不采集来源。
// ════════════════════════════════════════════════════════════════════════

#[test]
fn conn_origin_referenced_via_alias_only() {
    // 仅以别名作类型存在性引用（boot 不构造 ConnOrigin——来源采集是 shells listener 职责）。
    fn _takes(_o: &Origin) {}
    let _ = _takes as fn(&Origin);
}
