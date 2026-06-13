//! 启动序列子域（模块文档 06 §8.x / §3.1 / §8 F-1·F-2·L-1·B-2）。
//!
//! 严格依赖顺序即安全顺序：开库 → 重建快照 → 解锁保险箱 → 注册插件 → 先 control.sock(0600)
//! 后 data.sock(0660/组)挂数据面 router（**最后**开放）。任一步 Err 在 data.sock 创建前
//! 短路：进程以非零码退出、data.sock 不存在（公理二 fail-closed）。data.sock 在装配全部就绪
//! 且「可连 uid 自检」通过前绝不出现，杜绝半装配状态被连接（F-1）。
//!
//! 这条链是**单线程顺序启动链**，顺序本身是安全不变量：每步产出下一步的输入。设计取舍——
//! data.sock 必须最后创建：若提前 bind，则快照/保险箱/连接池任一未就绪的窗口内，落到
//! handler 的请求会撞上半装配状态，fail-closed 退化为「先开门再装锁」。因此 boot 把
//! 「创建 data.sock 并 serve」作为整链**唯一收尾动作**（chain 的 single terminal action），
//! 此前任一步返回 Err 都在 socket 创建前短路。
//!
//! 可测缝（06 §9 boot 测试策略：fail-closed 分支靠前置条件可注入）：装配链的四个前置
//! （开库 / 迁移 / 首快照 / 解锁）抽象为 [`Preconditions`]，两个 socket 创建抽象为
//! [`SocketFactory`]，data.sock 可连 uid 集合抽象为 [`ConnectableUidProbe`]——测试以 Fake
//! 注入失败即可观察「进程非零退出 + data.sock 未创建 + 数据面未开放」。真实装配在 [`run`]
//! 内用真实 store/secrets 实现这些 trait（本波次为 RED 桩，体 `todo!()`）。
//!
//! 路径纪律（雷区）：本目录**绝不**出现字面 `ConnOrigin::` 变体（构造点唯一在 shells/）；
//! 需要请求来源时以 `use postern_core::request::ConnOrigin as Origin` 别名读/解构。
//! 本目录**绝不**构造 `ResolvedTarget`/`ResourceCredential`（boot 只解锁保险箱并建 ScrubSet，
//! 凭据/地址物化在 connpool 请求期发生）。本目录零 SQL 标记（开库/迁移/首快照全经 store）。

pub mod selfcheck;
pub mod sockets;

use postern_core::error::Stage;

use crate::boot::selfcheck::connectable_uid_check;
use crate::error::Result;

/// 启动链的步骤标识——每步是依赖顺序中的一个固定位置，失败时据此归因「哪一前置不成立」。
///
/// 顺序即枚举判别值的语义顺序（§3.1 六步链）：开库 → 迁移校验 → 首快照 → 解锁保险箱 →
/// 注册插件 → control.sock → data.sock 可连 uid 自检 → data.sock 挂载（终结动作）。
/// 自检（[`ConnectableUidSelfCheck`](BootStage::ConnectableUidSelfCheck)）排在 data.sock
/// **创建之前**：含自身 uid 即在开放前 fail-closed（F-2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BootStage {
    /// [1] 开 policy.db（WAL）、校验 `user_version`/迁移版本/`settings` 表。
    OpenDb,
    /// [1b] 迁移版本校验（未知高版本 fail-closed）。
    Migrate,
    /// [2] 经 PolicyRepo 在一次事务内物化首份 `Arc<PolicySnapshot>`。
    FirstSnapshot,
    /// [3] 依 MasterKeySource 解锁保险箱、建 ScrubSet、写 lifecycle 审计。
    UnlockVault,
    /// [4] 注册插件（Adapter/Transport/Authenticator/ConditionPredicate）。
    RegisterPlugins,
    /// [5a] 先创建 control.sock（0600）。
    ControlSocket,
    /// [6] data.sock 可连 uid 自检（含自身 uid → fail-closed，data.sock 不创建）。
    ConnectableUidSelfCheck,
    /// [5b] 创建 data.sock（0660/组）并挂数据面 router——整链唯一收尾动作。
    DataSocket,
}

/// 启动失败：归因到失败的 [`BootStage`]，统一映射 [`DaemonError::Boot`](crate::error::DaemonError::Boot)。
///
/// fail-closed：任一前置 Err 都装成本类型，调用方据此进程非零退出；`stage` 仅供观察归因，
/// 不携带任何机密细节（错误串恒为常量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootError {
    /// 触发短路的启动步骤。
    pub stage: BootStage,
}

impl BootError {
    /// 由失败步骤构造启动错误。
    pub fn at(stage: BootStage) -> Self {
        Self { stage }
    }
}

/// 装配链的四个前置（开库 / 迁移 / 首快照 / 解锁）的可注入抽象（06 §9：前置条件可注入）。
///
/// 真实实现经 store `Db::open`/`migrate`/`PolicyRepo` 首快照与 secrets `MasterKeySource::obtain`
/// + `vault::unlock` + `ScrubSet::from_payload`；测试以 Fake 在任一步返回 Err 即驱动 fail-closed。
///   同步 store/secrets 调用在真实 [`run`] 里经 `spawn_blocking` 边界承接（§5），本 trait 只表达
///   「成功/失败」与顺序。
pub trait Preconditions {
    /// [1] 开 policy.db（WAL）并校验 schema。失败 → fail-closed（data.sock 不创建）。
    fn open_db(&self) -> Result<()>;
    /// [1b] 迁移版本校验（未知高版本拒绝加载）。
    fn migrate(&self) -> Result<()>;
    /// [2] 物化首份 `Arc<PolicySnapshot>`（经 PolicyRepo 一次事务）。
    fn rebuild_first_snapshot(&self) -> Result<()>;
    /// [3] 解锁保险箱、建 ScrubSet、写 lifecycle 审计。
    fn unlock_vault(&self) -> Result<()>;
}

/// 两平面 socket 创建的可注入抽象（L-1：control 0600、data 0660/组；先 control 后 data）。
///
/// 每个 create 各自 `bind` 后**立即 chmod/设属组再 listen**（无 umask 竞态窗口，L-1）——
/// 时序由实现保证，本 trait 只暴露「创建成功/失败」。测试 Fake 记录创建调用序，借此断言
/// 「control 早于 data」「早失败时 data 从未创建」。
pub trait SocketFactory {
    /// [5a] 创建 control.sock（0600）。失败 → fail-closed。
    fn create_control(&self) -> Result<()>;
    /// [5b] 创建 data.sock（0660/组）并挂数据面 router——整链唯一收尾动作。
    fn create_data(&self) -> Result<()>;
}

/// data.sock 可连 uid 自检的可注入抽象（F-2 / §3.1·6）。
///
/// 判定形态是**有效集合**测（当前 umask/属组/ACL 下哪些 uid 能 connect），**不是**读自报字段。
/// 返回 data.sock 在当前环境下可连的 uid 集合；[`Bootstrap`] 以此与自身 uid 比对，含自身即
/// fail-closed 拒绝启动（data.sock 不创建）。
pub trait ConnectableUidProbe {
    /// daemon 自身 uid（自检以此为基准）。
    fn self_uid(&self) -> u32;
    /// data.sock 在当前环境下的可连 uid 有效集合（非自报）。
    fn connectable_uids(&self) -> Vec<u32>;
}

/// 两平面 router 装配的可注入抽象（B-2 / L-14 / 红线 7.2-2 的可观察接线点）。
///
/// 这是「哪些句柄被注入哪个平面 router」的**唯一接线点**：[`Bootstrap::run`] 构造每个平面的
/// router 时，把该平面的句柄集**实际交给**对应方法；[`BootReport`] 的句柄集合直接源自本 trait
/// 收到的参数（而非另写一份字面 vec）。如此一来——若某实现把 [`HandleKind::PolicyRepo`] 误注入
/// 数据面 router，`assemble_data_plane` 必然收到它、报告必然反映它（红线 7.2-2 被见证而非空转）；
/// 「往 router 注入 PolicyRepo 却在报告里写干净 vec」这种伪装在结构上不可能。
///
/// 真实实现把句柄集物化为真实 axum router 的 `with_state`/扩展层；测试以记录式 Fake 见证
/// 「数据面 router 收到的句柄集恰不含 PolicyRepo、控制面 router 恰含 PolicyRepo」。
pub trait RouterAssembler {
    /// 装配**数据面** router：注入数据面所需句柄集（红线 7.2-2：绝不含 `PolicyRepo`）。
    /// 实现据传入句柄集构造 data.sock router；本调用即数据面注入集的物化点。
    fn assemble_data_plane(&self, handles: &[HandleKind]) -> Result<()>;
    /// 装配**控制面/清扫器** router：注入控制面所需句柄集（`PolicyRepo` 写句柄只在此）。
    fn assemble_control_plane(&self, handles: &[HandleKind]) -> Result<()>;
}

/// 启动报告：记录实际执行的步骤序与「data.sock 是否已创建/数据面是否开放」，供测试观察
/// 「失败时短路在 data.sock 创建之前」「成功时 data.sock 为整链终结动作」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootReport {
    /// 已执行步骤的有序序列（顺序即依赖顺序，F-1）。
    pub executed: Vec<BootStage>,
    /// data.sock 是否已创建并挂载数据面 router（fail-closed 短路时恒为 false）。
    pub data_plane_open: bool,
    /// 注入给**数据面 router** 的句柄种类集合（B-2/L-14：PolicyRepo 句柄绝不在此集合内）。
    pub data_plane_handles: Vec<HandleKind>,
    /// 注入给 **control/sweeper** 的句柄种类集合（PolicyRepo 写句柄只在此）。
    pub control_plane_handles: Vec<HandleKind>,
}

/// 装配产出的句柄种类（B-2/L-14：用于断言 PolicyRepo 只进控制面注入集、不进数据面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    /// 策略只读视图句柄（`PolicyView`）——数据面经此消费只读快照投影。
    PolicyView,
    /// 连接池句柄——数据面建连用。
    ConnPool,
    /// Sanitizer 句柄——数据面出口脱敏用。
    Sanitizer,
    /// 插件登记册句柄——数据面 kernel/connpool 选型用。
    Registries,
    /// PolicyRepo 事务写句柄——**只**进控制面/清扫器，绝不进数据面注入集（红线 7.2-2）。
    PolicyRepo,
}

/// 启动链编排器：按固定依赖顺序驱动可注入的前置/socket/自检，fail-closed 短路。
///
/// `run` 顺序恰为 §3.1 六步链；任一前置 Err 在 data.sock 创建前短路（[`BootReport::data_plane_open`]
/// 保持 false、`executed` 不含 `DataSocket`）。自检含自身 uid 时同样在 data.sock 创建前 fail-closed
/// （F-2）。整链 single terminal action 是 [`SocketFactory::create_data`]。
pub struct Bootstrap<P, S, U, A> {
    /// 四个装配前置（开库/迁移/首快照/解锁）。
    pre: P,
    /// 两平面 socket 创建（control 先于 data）。
    sockets: S,
    /// data.sock 可连 uid 自检。
    probe: U,
    /// 两平面 router 装配（句柄注入的唯一接线点，B-2/L-14）。
    assembler: A,
}

impl<P, S, U, A> Bootstrap<P, S, U, A>
where
    P: Preconditions,
    S: SocketFactory,
    U: ConnectableUidProbe,
    A: RouterAssembler,
{
    /// 由四个可注入组件装配编排器。
    pub fn new(pre: P, sockets: S, probe: U, assembler: A) -> Self {
        Self {
            pre,
            sockets,
            probe,
            assembler,
        }
    }

    /// 驱动启动链：按 §3.1 顺序逐步执行，任一步 Err 在 data.sock 创建前短路。
    ///
    /// 成功返回记录了完整步骤序与「数据面已开放」的 [`BootReport`]；失败返回 [`BootError`]
    /// （归因到失败 [`BootStage`]）且 data.sock 从未创建。data.sock 创建是整链唯一收尾动作，
    /// 在「可连 uid 自检」通过之后才发生（F-1/F-2）。
    pub fn run(&self) -> std::result::Result<BootReport, BootError> {
        let mut report = BootReport::default();

        // [1] 开 policy.db（WAL）并校验 schema（经 store，boot 不碰 SQL）。
        self.step(&mut report, BootStage::OpenDb, |p| p.pre.open_db())?;
        // [1b] 迁移版本校验（未知高版本 fail-closed）。
        self.step(&mut report, BootStage::Migrate, |p| p.pre.migrate())?;
        // [2] 经 PolicyRepo 在一次事务内物化首份 Arc<PolicySnapshot>。
        self.step(&mut report, BootStage::FirstSnapshot, |p| {
            p.pre.rebuild_first_snapshot()
        })?;
        // [3] 解锁保险箱、建 ScrubSet、写 lifecycle 审计。
        self.step(&mut report, BootStage::UnlockVault, |p| {
            p.pre.unlock_vault()
        })?;

        // [4] 注册插件（Adapter/Transport/Authenticator/ConditionPredicate）并装配两平面 router。
        // 装配句柄分流：PolicyRepo 写句柄只进控制面/清扫器（红线 7.2-2），数据面只拿只读投影。
        // 句柄集**实际交给** RouterAssembler 物化进对应平面 router；报告的句柄集合源自此处实际
        // 注入的内容（见下），故「注入 PolicyRepo 进数据面却报告干净」在结构上不可能（B-2/L-14）。
        report.executed.push(BootStage::RegisterPlugins);
        let data_plane_handles = [
            HandleKind::PolicyView,
            HandleKind::ConnPool,
            HandleKind::Sanitizer,
            HandleKind::Registries,
        ];
        let control_plane_handles = [HandleKind::PolicyRepo];
        // 数据面 router 装配：注入集物化点（红线 7.2-2 在此见证——PolicyRepo 绝不入此集）。
        self.assembler
            .assemble_data_plane(&data_plane_handles)
            .map_err(|_| BootError::at(BootStage::RegisterPlugins))?;
        // 控制面/清扫器 router 装配：PolicyRepo 写句柄只在此注入。
        self.assembler
            .assemble_control_plane(&control_plane_handles)
            .map_err(|_| BootError::at(BootStage::RegisterPlugins))?;
        // 报告的句柄集合源自实际注入数组（与 router 收到的逐元素一致），非另写一份字面 vec。
        report
            .data_plane_handles
            .extend_from_slice(&data_plane_handles);
        report
            .control_plane_handles
            .extend_from_slice(&control_plane_handles);

        // [5a] 先创建 control.sock（0600）。
        self.step(&mut report, BootStage::ControlSocket, |p| {
            p.sockets.create_control()
        })?;

        // [6] data.sock 可连 uid 自检——在 data.sock 创建之前（F-2）。
        report.executed.push(BootStage::ConnectableUidSelfCheck);
        let connectable = self.probe.connectable_uids();
        if !connectable_uid_check(self.probe.self_uid(), &connectable).is_pass() {
            return Err(BootError::at(BootStage::ConnectableUidSelfCheck));
        }

        // [5b] 创建 data.sock（0660/组）并挂数据面 router——整链唯一收尾动作。
        // data_plane_open 只能由 create_data 成功**返回后**置真：`step` 的 `?` 在 create_data
        // 失败时即短路返回 Err（report 连同未置真的 data_plane_open 一并丢弃），故「先开门再装锁」
        // （在 create 前置 open）无从发生——open 严格是 create 成功的下游（F-1 fail-closed）。
        self.step(&mut report, BootStage::DataSocket, |p| {
            p.sockets.create_data()
        })?;
        report.data_plane_open = true;

        Ok(report)
    }

    /// 执行一步：记录其 `BootStage`，调用下游动作；Err → 归因到该步并 fail-closed 短路。
    fn step<F>(
        &self,
        report: &mut BootReport,
        stage: BootStage,
        action: F,
    ) -> std::result::Result<(), BootError>
    where
        F: FnOnce(&Self) -> Result<()>,
    {
        report.executed.push(stage);
        action(self).map_err(|_| BootError::at(stage))
    }
}

/// 「下游装配错误 → 启动归因 stage」的观察辅助（仅供归因展示，不改 fail-closed 语义）。
///
/// boot 失败统一以 [`DaemonError::Boot`](crate::error::DaemonError::Boot) 上抛进程非零退出；
/// 此函数把 [`BootStage`] 映射到 core `Stage` 仅用于可观察归因（无 `_ =>` 兜底，新增 stage
/// 必须显式补臂）。建连相关步骤折叠到 `Stage::Transport`（connect 阶段）。
pub fn stage_of(stage: BootStage) -> Stage {
    match stage {
        BootStage::OpenDb => Stage::Audit,
        BootStage::Migrate => Stage::Audit,
        BootStage::FirstSnapshot => Stage::Rbac,
        BootStage::UnlockVault => Stage::Transport,
        BootStage::RegisterPlugins => Stage::Classify,
        BootStage::ControlSocket => Stage::Transport,
        BootStage::ConnectableUidSelfCheck => Stage::Transport,
        BootStage::DataSocket => Stage::Transport,
    }
}

/// 进程启动入口：以真实 store/secrets 装配 [`Bootstrap`] 并驱动启动链，最后开放 data.sock。
///
/// main.rs 唯一调用点；任一步失败在 socket 创建前短路并向上抛 Err（进程非零退出）。
/// 真实前置（开库/迁移/首快照/解锁）的同步 store/secrets 调用经 `spawn_blocking` 边界承接（§5）。
pub async fn run() -> Result<()> {
    todo!()
}
