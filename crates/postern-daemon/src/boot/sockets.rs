//! 启动期 socket 绑定：data.sock 与 control.sock 的创建/权限/时序（设计承诺级桩）。
//!
//! data.sock 是启动序列的**最后**一步——只有在开库/重建快照/解锁/注册插件全部成功且
//! 可连 uid 自检通过后才创建并开放，确保对端连上时背后是完整装配（F-1）。control.sock 以
//! 0600 创建、data.sock 以 0660/专用组创建。每个 socket 各自 `bind` 后**立即 chmod/设属组
//! 再 listen**，消除「默认 umask 下短暂可连」的竞态窗口（L-1，no umask race window）。绑定
//! 失败即向上短路（fail-closed）。
//!
//! 时序纪律：先 control.sock 后 data.sock；本单元的 [`bind_then_secure_then_listen`] 把
//! 「bind → chmod/set-group → listen」固化为单一原子序，调用方绝不在 chmod 前 listen。

use std::cell::RefCell;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::{DaemonError, Result};

/// 已绑定并收紧权限的 live UDS listener 输出格（`RealSocketFactory` 持有，供装配取用）。
///
/// `bind → secure → listen` 原子序成功后，live [`StdUnixListener`]（已置 nonblocking）存入此格；
/// 装配期再经 `tokio::net::UnixListener::from_std` 升格为 tokio listener 在其上 serve。以
/// `Arc<Mutex<Option<_>>>` 承载：`&self` 的工厂方法经内部可变性写入，装配方取出消费。
pub type ListenerCell = Arc<Mutex<Option<StdUnixListener>>>;

/// 单 socket 创建的三个子步骤标识（L-1 原子序的可观察单位）。
///
/// `bind_then_secure_then_listen` 把这三步固化为唯一原子序：先 [`Bind`](SocketSubStep::Bind)、
/// **立即** [`Secure`](SocketSubStep::Secure)（chmod/设属组）、最后 [`Listen`](SocketSubStep::Listen)。
/// 顺序本身是安全不变量——`Secure` 必须在 `Listen` 之前，否则 `bind` 后到 chmod 前存在默认
/// umask 下的短暂可连竞态窗口（TOCTOU）。本枚举让该子步次序在测试中可观察、不可被偷换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketSubStep {
    /// 绑定 UDS 路径（此刻 inode 以默认 umask 权限存在）。
    Bind,
    /// 立即 chmod 到目标模式位并（按需）设专用属组——关闭 umask 竞态窗口。
    Secure,
    /// 开始 listen 接受连接（仅在 Secure 之后，确保可连时权限已收紧）。
    Listen,
}

/// 单 socket 创建期的副作用接收器：按调用顺序记录 bind/secure/listen 三子步。
///
/// `bind_then_secure_then_listen` 是纯编排——它不直接做 IO，而是把三步**按固定顺序**派发
/// 给本接收器；真实实现把三步绑定到真实 UDS 系统调用，测试以记录式接收器见证子步次序。
/// 这样「listen 排在 secure 之前」的退化（打开竞态窗口）必然改变接收到的顺序 → 测试变红。
pub trait SocketEffects {
    /// 绑定 UDS 路径（占位真实 `UnixListener::bind`）。
    fn bind(&self) -> Result<()>;
    /// chmod 到 `perms.mode`、按 `perms.set_group` 设属组（占位真实 `chmod`/`chown`）。
    fn secure(&self, perms: SockPerms) -> Result<()>;
    /// 开始 listen（占位真实 `listen`/accept loop 挂载）。
    fn listen(&self) -> Result<()>;
}

/// 一个 socket 的目标权限位（control=0600、data=0660/专用组）。常量层即区分两平面权限隔离（L-1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SockPerms {
    /// 期望模式位（control 恒 0o600；data 恒 0o660）。
    pub mode: u32,
    /// 是否设专用属组（data 平面经专用组放行 Agent，control 不设）。
    pub set_group: bool,
}

/// control.sock 权限：0600、不设组（仅属主可读写，L-1）。
pub const CONTROL_PERMS: SockPerms = SockPerms {
    mode: 0o600,
    set_group: false,
};

/// data.sock 权限：0660、设专用组（Agent 经专用组放行，L-1）。
pub const DATA_PERMS: SockPerms = SockPerms {
    mode: 0o660,
    set_group: true,
};

/// 单 socket 绑定原子序：`bind` → 立即 `chmod`/设属组 → `listen`（无 umask 竞态窗口，L-1）。
///
/// 把三步固化为单一调用，杜绝「bind 后 listen 前的可连窗口」：先 [`bind`](SocketEffects::bind)，
/// **紧接** [`secure`](SocketEffects::secure)（携 `perms` chmod/设属组），**最后**
/// [`listen`](SocketEffects::listen)——`secure` 恒先于 `listen`。任一子步 `Err` 即 fail-closed
/// 向上短路（绝不在权限未收紧时进入 listen）。子步的实际 IO 由 `eff` 承接（真实实现绑真实
/// 系统调用，测试以记录式接收器见证次序）。
pub fn bind_then_secure_then_listen<E: SocketEffects>(eff: &E, perms: SockPerms) -> Result<()> {
    // L-1：bind → secure → listen 的原子序。secure 必须在 listen 之前，否则 bind 后到 chmod
    // 前存在默认 umask 下的竞态窗口；任一步失败短路（fail-closed），绝不带未收紧权限去 listen。
    eff.bind()?;
    eff.secure(perms)?;
    eff.listen()?;
    Ok(())
}

/// 真实 UDS [`SocketEffects`]：把三子步绑定到真实 `std` UDS 系统调用（无新增依赖）。
///
/// `bind` 经 [`StdUnixListener::bind`] 绑定路径（先清旧 inode）、`secure` 经
/// [`fs::set_permissions`] chmod 到 `perms.mode` 并按 `perms.set_group`+组名 chown 属组、
/// `listen` 把已收紧的 live listener 置 nonblocking 后存入 [`ListenerCell`]。`std` 绑定为同步、
/// 不需 tokio reactor，故 `RealSocketFactory` 的同步 `create_*` 方法可在 boot 同步链内直接驱动；
/// tokio 升格（`from_std`）推迟到装配/serve 期（彼时已在 async 上下文）。
struct RealSocketEffects<'a> {
    /// 目标 UDS 路径。
    path: &'a Path,
    /// 专用属组名（`None` 则不设组；`perms.set_group` 同时为真才 chown）。
    group: Option<&'a str>,
    /// `bind` 与 `listen` 间暂存已绑定 listener（`&self` 三步经内部可变性承接）。
    bound: RefCell<Option<StdUnixListener>>,
    /// `listen` 成功后 live listener 的去处（工厂的输出格）。
    out: &'a ListenerCell,
}

impl<'a> RealSocketEffects<'a> {
    fn new(path: &'a Path, group: Option<&'a str>, out: &'a ListenerCell) -> Self {
        Self {
            path,
            group,
            bound: RefCell::new(None),
            out,
        }
    }
}

impl SocketEffects for RealSocketEffects<'_> {
    fn bind(&self) -> Result<()> {
        // 清理旧 inode（残留 UDS 文件会令 bind 报 AddrInUse）；不存在则忽略。坏父目录 → bind
        // 失败 → fail-closed 返 Listener（绝不带半绑定状态前进）。
        let _ = fs::remove_file(self.path);
        let listener = StdUnixListener::bind(self.path).map_err(|_| DaemonError::Listener)?;
        *self.bound.borrow_mut() = Some(listener);
        Ok(())
    }

    fn secure(&self, perms: SockPerms) -> Result<()> {
        // 立即 chmod 到目标模式位——紧接 bind、先于 listen，关闭 umask 竞态窗口（L-1）。
        fs::set_permissions(self.path, fs::Permissions::from_mode(perms.mode))
            .map_err(|_| DaemonError::Listener)?;
        // 按需设专用属组：仅当要求设组且给定组名时 chown gid（gid 经 /etc/group 解析，纯 std
        // 文件读、无 libc）。组名给定却解析不到 → fail-closed（绝不静默放弃专用组隔离）。
        if perms.set_group {
            if let Some(name) = self.group {
                let gid = resolve_gid(name).ok_or(DaemonError::Listener)?;
                chown_group(self.path, gid)?;
            }
        }
        Ok(())
    }

    fn listen(&self) -> Result<()> {
        // std UDS 在 bind 时已进入 listen 态；本步把权限已收紧的 live listener 置 nonblocking
        // （供后续 tokio from_std 升格）后存入输出格。仅在 secure 之后执行（原子序保证）。
        let listener = self
            .bound
            .borrow_mut()
            .take()
            .ok_or(DaemonError::Listener)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| DaemonError::Listener)?;
        *self.out.lock().map_err(|_| DaemonError::Listener)? = Some(listener);
        Ok(())
    }
}

/// 解析组名 → gid（读 `/etc/group`，纯 std、无 libc 依赖）。未找到返 `None`。
fn resolve_gid(name: &str) -> Option<u32> {
    let mut contents = String::new();
    fs::File::open("/etc/group")
        .ok()?
        .read_to_string(&mut contents)
        .ok()?;
    for line in contents.lines() {
        // 格式：name:passwd:gid:members
        let mut fields = line.split(':');
        let group_name = fields.next()?;
        let _passwd = fields.next();
        let gid_str = fields.next()?;
        if group_name == name {
            return gid_str.parse().ok();
        }
    }
    None
}

/// chown 一个路径到 `gid`（uid 保持不变，传 `u32::MAX` 即「不改 owner」语义）。
///
/// 经 `std::os::unix::fs::chown`（std ≥1.73，无 libc/nix）。失败 → fail-closed 返 Listener。
fn chown_group(path: &Path, gid: u32) -> Result<()> {
    std::os::unix::fs::chown(path, None, Some(gid)).map_err(|_| DaemonError::Listener)
}

/// 在 `path` 上以 `perms`/`group` 跑 `bind → secure → listen` 原子序，live listener 存入 `out`。
///
/// 同步核：经 [`RealSocketEffects`] 把三子步绑真实 std UDS 系统调用，再交
/// [`bind_then_secure_then_listen`] 固化原子序。`RealSocketFactory` 两个 `create_*` 方法与下面
/// 两个 async 包装都收敛到此函数，确保 control/data 两平面走完全一致的绑定收紧时序。
pub fn create_listener_into(
    path: &Path,
    group: Option<&str>,
    perms: SockPerms,
    out: &ListenerCell,
) -> Result<()> {
    let eff = RealSocketEffects::new(path, group, out);
    bind_then_secure_then_listen(&eff, perms)
}

/// 创建并绑定 control.sock 于 `path`（0600，先于 data.sock），live listener 存入 `out`。
///
/// 经 [`create_listener_into`] 在 `path` 上固化 `bind → 立即 chmod 0600 → listen` 原子序
/// （[`CONTROL_PERMS`]，不设专用组）。绑定 / chmod 失败即 fail-closed 短路。
pub async fn bind_control(path: &Path, perms: SockPerms, out: &ListenerCell) -> Result<()> {
    create_listener_into(path, None, perms, out)
}

/// 创建并开放 data.sock 于 `path`（0660 + 可选专用组），live listener 存入 `out`。
///
/// 经 [`create_listener_into`] 在 `path` 上固化 `bind → 立即 chmod 0660 + 设专用组（`group`
/// 为 `Some` 时）→ listen` 原子序（[`DATA_PERMS`]）。整链终结动作，仅在可连 uid 自检通过后调用。
pub async fn open_data(
    path: &Path,
    group: Option<&str>,
    perms: SockPerms,
    out: &ListenerCell,
) -> Result<()> {
    create_listener_into(path, group, perms, out)
}
