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

use crate::error::Result;

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

/// 创建并绑定 control.sock（0600，先于 data.sock）（占位）。
pub async fn bind_control() -> Result<()> {
    todo!()
}

/// 创建并开放 data.sock（0660/组，启动序列最后一步、整链唯一收尾动作）（占位）。
pub async fn open_data() -> Result<()> {
    todo!()
}
