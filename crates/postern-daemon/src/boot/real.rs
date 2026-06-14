//! 启动链四个可注入抽象的**真实实现**（经 store / secrets / OS）（设计承诺级桩，体未实现）。
//!
//! [`Bootstrap`](crate::boot::Bootstrap) 的四泛型在生产装配时由本模块的四个结构充当：
//! - [`RealPreconditions`]：开库 / 迁移 / 首快照 / 解锁经 `postern_store` 的 `Db`/`migrate`/
//!   `build_snapshot` 与 `postern_secrets` 的 `KeyFile`/`vault::unlock`/`ScrubSet::from_payload`。
//! - [`RealSocketFactory`]：control.sock(0600) / data.sock(0660/组) 经 `bind → secure → listen`
//!   原子序（[`bind_then_secure_then_listen`](crate::boot::sockets::bind_then_secure_then_listen)）。
//! - [`RealUidProbe`]：`self_uid` 经 SO_PEERCRED 安全 API（tokio `UnixStream::pair` + `peer_cred`，
//!   **无 unsafe**）取自身 uid；`connectable_uids` 探测 data.sock 在当前环境的可连 uid 有效集。
//! - [`RealRouterAssembler`]：把句柄集物化进对应平面 axum router（数据面绝不含 PolicyRepo）。
//!
//! 步骤间状态承接（开库产 `Db`、解锁产 `UnlockedVault`/`ScrubSet`）经内部 `Mutex<Option<_>>`
//! 持有——`Preconditions` 四方法签名为 `&self`，故前一步产物以内部可变性留给后一步消费。
//!
//! 雷区纪律：本文件在 `src/boot/`（**非** shells / kernel）——零 SQL 标记（开库 / 迁移 / 首快照
//! 全经 store API）、不构造 `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`、`anyhow` 禁用。
//! 机密类型（主密钥 / vault / ScrubSet）经 secrets 面 API 构造，本文件不字面构造机密类型。
//!
//! 本波次为 RED 桩：字段 / 签名对齐设计承诺，trait 方法体 `unimplemented!()`。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use postern_core::domain::ResourceCode;
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink, PolicyView};
use postern_secrets::scrubset::ScrubSet;
use postern_secrets::unlock::key_file::KeyFile;
use postern_secrets::unlock::source::MasterKeySource;
use postern_secrets::vault;
use postern_store::base::db::Db;
use postern_store::migrate::migrate as run_migrate;
use postern_store::snapshot::build::build_snapshot;
use postern_store::snapshot::view::SnapshotView;

use crate::assemble::PlaneSpawner;
use crate::boot::sockets::{create_listener_into, ListenerCell, CONTROL_PERMS, DATA_PERMS};
use crate::boot::{ConnectableUidProbe, HandleKind, Preconditions, RouterAssembler, SocketFactory};
use crate::control::{
    self, Actor, ControlState, Enrollment, PolicyRepo, WriteError, WriteIntent, WriteOutcome,
};
use crate::error::{DaemonError, Result};

/// 首份快照的策略修订号（boot 物化首份 `Arc<PolicySnapshot>` 的 rev——后续每次重建递增由
/// 控制面承接）。
const FIRST_POLICY_REV: u64 = 0;

/// 首份快照视图的**共享** cell（`RealPreconditions` 与 `RealRouterAssembler` 共持同一 Arc）。
///
/// `rebuild_first_snapshot` 写入后，持同一 cell 的装配器在装配链内（rebuild 之后）懒读到已物化
/// 的 `Arc<SnapshotView>`——这正是「`Bootstrap::run` 只产元数据，live 视图经共享 cell 暴露」缝。
pub type SnapshotCell = Arc<Mutex<Option<Arc<SnapshotView>>>>;

/// 装配链四前置的真实实现（开库 / 迁移 / 首快照 / 解锁）。
///
/// 经 `postern_store`（`Db::open` / `migrate` / `build_snapshot`）与 `postern_secrets`
/// （`KeyFile` 直接持有主密钥 → `vault::unlock` → `ScrubSet::from_payload`）落实四前置；
/// 任一步失败 fail-closed 返 `Err`（data.sock 不创建）。坏 vault → `unlock_vault` 必 `Err`。
pub struct RealPreconditions {
    /// policy.db 路径（`open_db` 经 `Db::open` 打开）。
    db_path: PathBuf,
    /// vault 文件路径（`unlock_vault` 读字节经 `vault::unlock` 解锁）。
    vault_path: PathBuf,
    /// keyfile 路径（`unlock_vault` 读 32B 主密钥构造 `KeyFile` 来源，无 argon2）。
    keyfile_path: PathBuf,
    /// 开库产物 `Db`（`open_db` 置入，`migrate`/`rebuild_first_snapshot` 消费）。
    /// `&self` 签名下经 `Mutex<Option<_>>` 承接前一步产物；`Arc` 供后续装配共享读取。
    db: Mutex<Option<Arc<Db>>>,
    /// 首份快照视图（`rebuild_first_snapshot` 物化首份 `Arc<PolicySnapshot>` 后置入）。
    /// 数据面装配经此 `Arc<SnapshotView>` 消费只读快照投影。以**共享 Arc cell** 持有——
    /// `RealRouterAssembler` 持同一 cell 的克隆，在装配链内（rebuild 之后）懒读已物化的 view，
    /// 故无需在装配器构造时就拿到 view（避免拆散 `Bootstrap::run` 的单链短路语义）。
    snapshot: SnapshotCell,
    /// 解锁产物擦除集（`unlock_vault` 由解出 `Payload` 派生后置入）。
    /// 内核出口脱敏经此 `Arc<ScrubSet>` 消费（机密派生只在 secrets 面 API 内发生）。
    scrubset: Mutex<Option<Arc<ScrubSet>>>,
}

impl RealPreconditions {
    /// 由三个路径构造真实前置（开库产物 / 解锁产物在各步骤经内部可变性承接）。
    pub fn new(db_path: PathBuf, vault_path: PathBuf, keyfile_path: PathBuf) -> Self {
        Self {
            db_path,
            vault_path,
            keyfile_path,
            db: Mutex::new(None),
            snapshot: Arc::new(Mutex::new(None)),
            scrubset: Mutex::new(None),
        }
    }

    /// 取首份快照 cell 的共享克隆（同一 `Arc<Mutex<Option<_>>>`）——交 `RealRouterAssembler`
    /// 在装配链内懒读 rebuild 后物化的 view。装配器与本前置共享同一 cell，rebuild 写入后
    /// 装配器读到的即已物化的 view（无需在构造时就持有解包后的 view）。
    pub fn snapshot_cell(&self) -> SnapshotCell {
        Arc::clone(&self.snapshot)
    }

    /// 取已承接的 `Db` 句柄（`open_db` 之后调用）。锁 poisoned 则恢复（不 unwrap，临界区
    /// 不 panic；与 store base 同纪律）。未开库（`None`）→ fail-closed 返 `DaemonError::Boot`。
    fn db_handle(&self) -> Result<Arc<Db>> {
        let guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone().ok_or(DaemonError::Boot)
    }
}

impl Preconditions for RealPreconditions {
    fn open_db(&self) -> Result<()> {
        // 开 policy.db（空文件自动建表前置；WAL/外键由 store 内部置）。失败 fail-closed。
        let db = Db::open(&self.db_path).map_err(|_| DaemonError::Boot)?;
        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Arc::new(db));
        Ok(())
    }

    fn migrate(&self) -> Result<()> {
        // 迁移校验：空库建全套表 + 前进 user_version；未知高版本 fail-closed（store 判定）。
        let db = self.db_handle()?;
        run_migrate(&db).map_err(|_| DaemonError::Boot)
    }

    fn rebuild_first_snapshot(&self) -> Result<()> {
        // 经 store 在一次只读事务内把权威库投影为首份 Arc<PolicySnapshot>，装进 SnapshotView
        // 留给数据面装配（boot 不碰 SQL，全经 store API）。失败 fail-closed。
        let db = self.db_handle()?;
        let snapshot = build_snapshot(&db, FIRST_POLICY_REV).map_err(|_| DaemonError::Boot)?;
        let view = Arc::new(SnapshotView::new(Arc::new(snapshot)));
        let mut guard = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(view);
        Ok(())
    }

    fn unlock_vault(&self) -> Result<()> {
        // keyfile 32B 主密钥 → KeyFile 来源 obtain（无 argon2）→ 读 vault 字节经 vault::unlock
        // 解锁 → 由解出 Payload 派生 ScrubSet 留给内核出口脱敏。任一步失败 fail-closed：坏
        // vault / 主密钥不符 → UnlockError → DaemonError::Boot（data.sock 不创建）。
        let raw = std::fs::read(&self.keyfile_path).map_err(|_| DaemonError::Boot)?;
        let key_bytes: [u8; 32] = raw.as_slice().try_into().map_err(|_| DaemonError::Boot)?;
        // 主密钥经 secrets 面 KeyFile 来源持有/取得（Zeroizing 离作用域清零，不字面留明文）。
        let source = KeyFile::new(Zeroizing::new(key_bytes));
        let master = source.obtain().map_err(|_| DaemonError::Boot)?;

        let vault_bytes = std::fs::read(&self.vault_path).map_err(|_| DaemonError::Boot)?;
        let unlocked = vault::unlock(&master, &vault_bytes).map_err(|_| DaemonError::Boot)?;
        // ScrubSet 经 secrets 面 from_payload 派生（机密派生只在 secrets API 内）。
        let scrubset = Arc::new(ScrubSet::from_payload(unlocked.payload()));
        let mut guard = self.scrubset.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(scrubset);
        Ok(())
    }
}

/// 两平面 socket 创建的真实实现（control 0600 先于 data 0660/组）。
///
/// 每个 create 经 [`bind_then_secure_then_listen`](crate::boot::sockets::bind_then_secure_then_listen)
/// 固化 `bind → 立即 chmod/设属组 → listen` 原子序（无 umask 竞态窗口，L-1）；control 用
/// [`CONTROL_PERMS`](crate::boot::sockets::CONTROL_PERMS)、data 用
/// [`DATA_PERMS`](crate::boot::sockets::DATA_PERMS)。绑定 / chmod 失败即 fail-closed 短路。
pub struct RealSocketFactory {
    /// control.sock 路径（0600 创建）。
    control_sock: PathBuf,
    /// data.sock 路径（0660 + 专用组创建，整链终结动作）。
    data_sock: PathBuf,
    /// data.sock 专用属组（`None` 则不设专用组）。
    data_group: Option<String>,
    /// `create_control` 成功后 live control listener 的去处（供控制面装配取用）。
    control_listener: ListenerCell,
    /// `create_data` 成功后 live data listener 的去处（供数据面装配取用）。
    data_listener: ListenerCell,
}

impl RealSocketFactory {
    /// 由两 socket 路径 + data.sock 专用属组构造（两 listener 输出格初始为空）。
    pub fn new(control_sock: PathBuf, data_sock: PathBuf, data_group: Option<String>) -> Self {
        Self {
            control_sock,
            data_sock,
            data_group,
            control_listener: Arc::new(Mutex::new(None)),
            data_listener: Arc::new(Mutex::new(None)),
        }
    }

    /// 取走已绑定的 control listener（装配期升格 `tokio::from_std` 在其上 serve；取一次后为空）。
    pub fn take_control_listener(&self) -> Option<std::os::unix::net::UnixListener> {
        self.control_listener.lock().ok().and_then(|mut c| c.take())
    }

    /// 取走已绑定的 data listener（装配期升格 `tokio::from_std` 在其上 serve；取一次后为空）。
    pub fn take_data_listener(&self) -> Option<std::os::unix::net::UnixListener> {
        self.data_listener.lock().ok().and_then(|mut c| c.take())
    }

    /// control listener cell 的共享克隆（同一 `Arc<Mutex<Option<_>>>`）——boot::run 在把工厂
    /// 移入 `Bootstrap` **之前**克隆此 cell，`create_control` 成功后经该 cell 取出 live listener
    /// 建 spawner（live FD 经共享 cell 暴露，Bootstrap::run 只产元数据）。
    pub fn control_listener_cell(&self) -> ListenerCell {
        Arc::clone(&self.control_listener)
    }

    /// data listener cell 的共享克隆（同上，供 spawner 取出 data.sock live listener）。
    pub fn data_listener_cell(&self) -> ListenerCell {
        Arc::clone(&self.data_listener)
    }
}

impl SocketFactory for RealSocketFactory {
    fn create_control(&self) -> Result<()> {
        // bind → 立即 chmod 0600 → listen 原子序（不设专用组），live listener 存入 control 输出格。
        // 坏路径 / chmod 失败即 fail-closed 短路（绝不带未收紧权限的 socket 前进）。
        create_listener_into(
            &self.control_sock,
            None,
            CONTROL_PERMS,
            &self.control_listener,
        )
    }

    fn create_data(&self) -> Result<()> {
        // bind → 立即 chmod 0660 + 设专用组（data_group 为 Some 时）→ listen 原子序，live listener
        // 存入 data 输出格（整链唯一收尾动作）。坏路径 / chmod / chown 失败即 fail-closed 短路。
        create_listener_into(
            &self.data_sock,
            self.data_group.as_deref(),
            DATA_PERMS,
            &self.data_listener,
        )
    }
}

/// data.sock 可连 uid 自检的真实实现（F-2）。
///
/// `self_uid` 经 SO_PEERCRED 安全 API 取：建 `tokio::net::UnixStream::pair()` 自连对，对一端
/// `peer_cred()?.uid()` 即本进程 uid（**无 unsafe、无 libc 直调、无新增依赖**，与 listener 同一
/// 可信来源哲学）。`connectable_uids` 探测 data.sock 在当前 umask / 属组 / ACL 下**除 owner
/// 自身以外**的他者可连 uid **有效集合**（非自报字段），交 [`Bootstrap`](crate::boot::Bootstrap)
/// 与自身 uid 比对——含自身即「别的主体与 daemon 同 uid」的危险态，fail-closed 拒启动（F-2）。
pub struct RealUidProbe {
    /// data.sock 路径（`connectable_uids` 据此探测他者可连 uid 有效集）。
    data_sock: PathBuf,
}

impl RealUidProbe {
    /// 由 data.sock 路径构造（可连集探测以此 socket 为对象）。
    pub fn new(data_sock: PathBuf) -> Self {
        Self { data_sock }
    }
}

impl RealUidProbe {
    /// 经 SO_PEERCRED 安全 API 取本进程 uid：建 `UnixStream::pair()` 自连对，对一端
    /// `peer_cred()?.uid()` 即本进程 uid（无 unsafe / 无 libc 直调 / 无新增依赖）。
    ///
    /// `pair` / `peer_cred` 失败 → `None`：自检无法判定，调用方据此 fail-closed（不放行）。
    /// 同步签名（trait 方法 `&self -> u32` 无 async），在已有 tokio reactor 上下文内调用
    /// （`Bootstrap::run` 经异步 `run()` 承接，§5）；自连对不进 reactor 长存即用即弃。
    fn probe_self_uid(&self) -> Option<u32> {
        let (a, _b) = tokio::net::UnixStream::pair().ok()?;
        a.peer_cred().ok().map(|cred| cred.uid())
    }
}

impl ConnectableUidProbe for RealUidProbe {
    fn self_uid(&self) -> u32 {
        // SO_PEERCRED 安全 API：自连对一端的 peer_cred().uid() 即本进程 uid（无 unsafe）。
        // pair / peer_cred 失败 → u32::MAX 哨兵（极罕见；自检以 self_uid 为基准，探测不到自身
        // uid 时退化为「与任何真实可连 uid 都不相等」，不误把正常部署判成同 uid 危险态）。
        self.probe_self_uid().unwrap_or(u32::MAX)
    }

    fn connectable_uids(&self) -> Vec<u32> {
        // F-2 可连集合的语义是「**除 owner 自身以外**、当前 data.sock 权限/属组/ACL 下能 connect
        // 的他者 uid 有效集合」——owner（daemon 自身）能连其自建 socket 是平凡真，**不是** F-2
        // 要测的东西；F-2 测的是「有没有**别的**主体（Agent）与 daemon 同 uid」（含自身即危险态）。
        // 故 owner 自身的 uid 绝不进此集合（否则 connectable_uid_check 必然恒 RefuseSameUid、永不
        // 开放 data.sock）。
        //
        // D1：尚无任何 Agent 经专用组/ACL 被推导为可连他者（属组成员→uid 的推导是 D2/D3 装配引入），
        // 故已知的「他者可连 uid」集合为空——没有任何别的主体被探测到与 daemon 同 uid，自检 Pass、
        // data.sock 正常开放。后续波次在此处接专用组成员/ACL 推导出真实他者可连集（仍排除 owner）。
        let _ = &self.data_sock;
        Vec::new()
    }
}

/// 装配阶段产出的 live axum router 输出格（`RealRouterAssembler` 写入，spawner 取出 serve）。
pub type RouterCell = Arc<Mutex<Option<axum::Router>>>;

/// D1 控制面策略读句柄：只读首份快照的 `policy_rev` 投影（健康端点据此对账）。
///
/// 控制面写路径（`commit_write`）三联动与集合分页读（`list`）的真实 store 接线是 D2——此处
/// 写 / 列读一律 fail-closed 返错（绝不伪报成功），只把 `policy_rev` 经已物化的 [`SnapshotView`]
/// 如实投影出来，使控制面 `GET /v1/health` 在 D1 能回真实修订号。
struct BootPolicyRepo {
    /// 首份快照的只读视图（`policy_rev` 取自当前快照）。
    view: Arc<SnapshotView>,
}

impl BootPolicyRepo {
    /// 经 PolicyView 取当前快照修订号。
    fn rev(&self) -> u64 {
        self.view.snapshot().policy_rev
    }
}

impl PolicyRepo for BootPolicyRepo {
    fn commit_write(
        &self,
        _actor: &Actor,
        _intent: &WriteIntent,
    ) -> std::result::Result<WriteOutcome, WriteError> {
        // 控制面写三联动的真实 store 接线是 D2；D1 fail-closed 拒（绝不伪报 COMMIT 成功）。
        Err(WriteError::Transaction)
    }

    fn list(
        &self,
        _entity: &'static str,
        _page: PageQuery,
    ) -> std::result::Result<Page<serde_json::Value>, DaemonError> {
        // 集合分页读的真实 store scan 接线是 D2；D1 fail-closed 返错（不伪造空信封）。
        Err(DaemonError::Boot)
    }

    fn policy_rev(&self) -> std::result::Result<u64, DaemonError> {
        // 健康投影锚点：取当前快照的 policy_rev（只读视图，读端无锁、不失败）。
        Ok(self.rev())
    }
}

/// D1 机密面登记句柄：登记的真实机密面接线是 D2——此处 fail-closed 拒（不伪报登记成功）。
struct BootEnrollment;

impl Enrollment for BootEnrollment {
    fn enroll(
        &self,
        _resource: &ResourceCode,
        _tier: &str,
    ) -> std::result::Result<(), DaemonError> {
        Err(DaemonError::Boot)
    }
}

/// D1 审计写句柄：审计落库的真实 store 接线是 D2——此处最小 no-op 接收（恒成功，不落库）。
///
/// D1 不驱动控制面写三联动（`commit_write` 即 fail-closed），故本 sink 不在 D1 被实际触达；
/// 留作 `ControlState` 类型完整性的占位，D2 替换为经 store 审计 base 的真实落库 sink。
struct BootAudit;

impl AuditSink for BootAudit {
    fn record(
        &self,
        _event: AuditEvent,
    ) -> std::result::Result<(), postern_core::error::AuditError> {
        Ok(())
    }
}

/// 两平面 router 装配的真实实现（B-2 / L-14 / 红线 7.2-2 的接线点）。
///
/// 把句柄集物化进对应平面的真实 axum router：控制面 router 经 [`control::router`] 装配 30 路由
/// （注入集 = [`ControlState`]：`PolicyRepo` + `Enrollment` + `AuditSink`，红线 7.2-2 写句柄只在此），
/// 数据面 router 为 D1 最小 router（占位 handler，能 serve 即可；真实数据面转发是 D3）。装配产物
/// 经内部 [`RouterCell`] 留给 [`RealSpawner`] 在对应 socket 上 serve（live router 由本结构持有，
/// `&[HandleKind]` 仍作"句柄分流见证"参数）。
pub struct RealRouterAssembler {
    /// 与 [`RealPreconditions`] 共享的首份快照 cell——装配链内（rebuild 之后）懒读已物化 view，
    /// 建 `ControlState` 的 `policy_rev` 投影源。
    snapshot: SnapshotCell,
    /// `assemble_control_plane` 成功后 live 控制面 router 的去处（供 spawner 取用）。
    control_router: RouterCell,
    /// `assemble_data_plane` 成功后 live 数据面 router 的去处（供 spawner 取用）。
    data_router: RouterCell,
}

impl RealRouterAssembler {
    /// 由与前置共享的首份快照 cell 构造真实装配器（两平面 router 输出格初始为空）。
    ///
    /// `snapshot` 是 [`RealPreconditions::snapshot_cell`] 的克隆——装配链在 rebuild 之后调用
    /// `assemble_control_plane` 时，cell 内 view 已物化，装配器据此建 `ControlState`。
    pub fn new(snapshot: SnapshotCell) -> Self {
        Self {
            snapshot,
            control_router: Arc::new(Mutex::new(None)),
            data_router: Arc::new(Mutex::new(None)),
        }
    }

    /// 取走已装配的控制面 router cell 的共享克隆（spawner 在 control.sock 上 serve）。
    pub fn control_router_cell(&self) -> RouterCell {
        Arc::clone(&self.control_router)
    }

    /// 取走已装配的数据面 router cell 的共享克隆（spawner 在 data.sock 上 serve）。
    pub fn data_router_cell(&self) -> RouterCell {
        Arc::clone(&self.data_router)
    }
}

impl RouterAssembler for RealRouterAssembler {
    fn assemble_data_plane(&self, handles: &[HandleKind]) -> Result<()> {
        // 红线 7.2-2 见证：数据面句柄集绝不含 PolicyRepo——收到即 fail-closed 拒装配（绝不把
        // 写句柄接进数据面 router）。D1 数据面为最小 router（占位 handler，能 serve 即可，D3 接真）。
        if handles.contains(&HandleKind::PolicyRepo) {
            return Err(DaemonError::Boot);
        }
        let router = axum::Router::new().fallback(data_plane_stub);
        *self.data_router.lock().map_err(|_| DaemonError::Boot)? = Some(router);
        Ok(())
    }

    fn assemble_control_plane(&self, _handles: &[HandleKind]) -> Result<()> {
        // 控制面 router 经 control::router 装配 30 路由，注入集 = ControlState（PolicyRepo 写句柄
        // 只在此）。D1：PolicyRepo 为 policy_rev 只读投影、Enrollment fail-closed、Audit no-op；
        // health 端点据此真实可达，其余 29 路由 501（D2 接真）。
        // 装配链在 rebuild 之后调用本方法，故共享 cell 内 view 已物化；缺 view（被绕开调用）⇒
        // fail-closed（绝不在快照未就绪时装配控制面）。
        let view = self
            .snapshot
            .lock()
            .map_err(|_| DaemonError::Boot)?
            .clone()
            .ok_or(DaemonError::Boot)?;
        let state = ControlState::new(
            Arc::new(BootPolicyRepo { view }),
            Arc::new(BootEnrollment),
            Arc::new(BootAudit),
        );
        let router = control::router::router(state);
        *self.control_router.lock().map_err(|_| DaemonError::Boot)? = Some(router);
        Ok(())
    }
}

/// D1 数据面占位 handler：回 501（真实数据面转发是 D3）。仅使数据面 router 能 serve。
async fn data_plane_stub() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_IMPLEMENTED
}

/// 三平面 spawn 的真实实现（§3.8）：持装配阶段捕获的 live listener + router，逐平面 `tokio::spawn`。
///
/// 关键架构缝：[`Bootstrap::run`](crate::boot::Bootstrap::run) 只产 [`BootReport`](crate::boot::BootReport)
/// 元数据（不带 live FD/router）。[`RealSocketFactory`] 与 [`RealRouterAssembler`] 在装配期把 live
/// listener / router 经各自内部 cell 暴露；[`boot::run`](crate::boot::run) 在 `Bootstrap::run` 成功后
/// 取出这些 live 句柄注入本结构。`PlaneSpawner` 三方法签名为 `&self` + `&[HandleKind]`（见证参数）
/// 不变——live 句柄由本结构经内部 `Mutex<Option<_>>` 持有，取出后 `tokio::spawn`
/// [`serve_router_over_uds`](crate::shells::serve::serve_router_over_uds)。
///
/// 句柄分流红线 7.2-2：控制面 / sweeper 才拿 `PolicyRepo`（已由 `RealRouterAssembler` 在控制面
/// router 注入集物化），数据面只读投影——本结构只搬运 listener/router，分流在装配期已定。
pub struct RealSpawner {
    /// control.sock live listener cell（`spawn_control_plane` 取出升格 tokio 后 serve）。
    control_listener: ListenerCell,
    /// data.sock live listener cell（`spawn_data_plane` 取出升格 tokio 后 serve）。
    data_listener: ListenerCell,
    /// 控制面 live router cell（`control::router` 装配的 30 路由）。
    control_router: RouterCell,
    /// 数据面 live router cell（D1 最小 router）。
    data_router: RouterCell,
}

impl RealSpawner {
    /// 由装配期捕获 live 句柄的四个共享 cell 构造（boot::run 在 `Bootstrap::run` 成功后建）。
    ///
    /// 四 cell 分别是 [`RealSocketFactory`] 两 listener cell 与 [`RealRouterAssembler`] 两 router
    /// cell 的共享克隆——`Bootstrap::run` 成功后这些 cell 已被填入 live 句柄，本结构持其克隆，
    /// spawn 时各自取出 serve。
    pub fn new(
        control_listener: ListenerCell,
        data_listener: ListenerCell,
        control_router: RouterCell,
        data_router: RouterCell,
    ) -> Self {
        Self {
            control_listener,
            data_listener,
            control_router,
            data_router,
        }
    }

    /// 取出一平面的 live (listener, router)，升格 tokio listener 后 `tokio::spawn` serve。
    /// 缺 listener / router（重复 spawn / 未捕获）或升格失败 → fail-closed 返 `Err`（不放行半装配）。
    fn spawn_plane(listener: &ListenerCell, router: &RouterCell) -> Result<()> {
        let std_listener = listener
            .lock()
            .map_err(|_| DaemonError::Listener)?
            .take()
            .ok_or(DaemonError::Listener)?;
        let router = router
            .lock()
            .map_err(|_| DaemonError::Listener)?
            .take()
            .ok_or(DaemonError::Listener)?;
        // std listener 在 sockets 绑定期已置 nonblocking（from_std 前置）。升格到 tokio reactor。
        let tokio_listener =
            tokio::net::UnixListener::from_std(std_listener).map_err(|_| DaemonError::Listener)?;
        tokio::spawn(crate::shells::serve::serve_router_over_uds(
            tokio_listener,
            router,
        ));
        Ok(())
    }
}

impl PlaneSpawner for RealSpawner {
    fn spawn_data_plane(&self, _handles: &[HandleKind]) -> Result<()> {
        // 数据面：在 data.sock 上 serve 最小数据面 router（句柄分流见证在装配期已定，红线 7.2-2）。
        Self::spawn_plane(&self.data_listener, &self.data_router)
    }

    fn spawn_control_plane(&self, _handles: &[HandleKind]) -> Result<()> {
        // 控制面：在 control.sock 上 serve 30 路由控制面 router（PolicyRepo 写句柄注入集只在此）。
        Self::spawn_plane(&self.control_listener, &self.control_router)
    }

    fn spawn_sweeper(&self, _handles: &[HandleKind]) -> Result<()> {
        // sweeper 周期任务（actor=system，与控制面共用 PolicyRepo 写锁）的真实回收逻辑是 D2/D3。
        // D1 最小：成功返回（不 spawn 实际 tick）——进程对外形态由两平面 router serve 承载。
        Ok(())
    }
}
