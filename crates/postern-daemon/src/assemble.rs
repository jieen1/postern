//! 进程级装配缝（模块文档 06 §1 唯一组装点 / §3.8 并发与线程模型 / §5.1 进程对外形态 / §8 DoD）。
//!
//! 这是 daemon 的**最终组装点**在库面的可测投影：`main.rs`（唯一允许 `anyhow` 的文件）建好
//! tokio multi-thread 运行时后，把控制权交给 boot 启动链产出 [`boot::BootReport`]，再交给本缝
//! 把已装配状态**接线**为进程对外形态——
//!
//! 1. **三个相互独立的 spawn**（§3.8 多线程运行时）：数据面 router（挂 data.sock 外壳）、
//!    控制面 router（挂 control.sock 控制面）、sweeper 周期任务，各自 spawn 到 multi-thread
//!    运行时上、互不阻塞；三处 spawn 经可注入的 [`PlaneSpawner`] 暴露为接线点，集成测试以
//!    Fake 见证「恰好三处独立 spawn、各拿到对应平面句柄集」。
//! 2. **boot Err → 非零进程退出码**（§8 / 公理二 fail-closed）：一次 boot `Err` 经 [`boot_exit_code`]
//!    映射为非零退出码（`main` 据此 `process::exit` 非零 / 返回 `Err`），且 data.sock **不 serving**
//!    （boot 在 socket 创建前短路，[`boot::BootReport::data_plane_open`] 恒 false）。
//!
//! 纪律（雷区）：本文件在 `src/`（**非** `src/kernel/`、**非** `src/shells/`）——零 SQL 标记、
//! 绝不出现字面 `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`（需要来源类型时经
//! `use postern_core::request::ConnOrigin as Origin` 别名）；`anyhow` 禁用（仅 main.rs），
//! 只用 [`DaemonError`](crate::error::DaemonError)/`thiserror`。本缝只搬运装配产物 + 接线，
//! 无任何业务/安全判定（Simplicity-First：main 极薄、库面只负责 spawn 编排与退出码映射）。
//!
//! 本波次为 RED 桩：类型/签名对齐设计承诺，函数体 `todo!()`。

use crate::boot::{BootError, BootReport, HandleKind};
use crate::error::{DaemonError, Result};

/// 进程成功退出码（boot 全装配就绪、三平面已 spawn、data.sock serving）。
pub const EXIT_OK: u8 = 0;

/// 进程 fail-closed 退出码（boot 任一步 Err → 非零退出、data.sock 不 serving，公理二）。
///
/// 恒**非零**：任何对 [`boot_exit_code`] 的失败映射都落到这个非零码，使「boot 失败」在进程
/// 边界以非零退出可观察（systemd/容器据此判定启动失败、不误判已就绪）。
pub const EXIT_BOOT_FAILED: u8 = 1;

/// 进程三平面（数据面 router / 控制面 router / sweeper 周期任务）的可注入 spawn 缝（§3.8）。
///
/// boot 成功后，[`serve_assembled`] 把每个平面**各自独立** spawn 到 multi-thread 运行时上：
/// 数据面 router 与控制面 router 跑在各自 socket 上、互不阻塞；sweeper 作为周期任务并行 spawn。
/// 三处 spawn 经本缝暴露为接线点——真实实现把每处接到 `tokio::spawn`（router serve / sweeper
/// run），测试以记录式 Fake 见证「恰好三处独立 spawn、各平面拿到 boot 装配出的对应句柄集」。
///
/// 句柄集分流是红线 7.2-2 的进程级见证：数据面 spawn **绝不**收到 [`HandleKind::PolicyRepo`]，
/// 控制面/ sweeper spawn 才持 `PolicyRepo` 写句柄。每处 spawn 失败即 fail-closed 上抛（不放行
/// 半装配的进程形态）。
pub trait PlaneSpawner {
    /// spawn 数据面 router（挂 data.sock 外壳：HTTP/MCP 共挂）。`handles` 恰为数据面注入集
    /// （绝不含 [`HandleKind::PolicyRepo`]）。失败 → fail-closed `Err`。
    fn spawn_data_plane(&self, handles: &[HandleKind]) -> Result<()>;
    /// spawn 控制面 router（挂 control.sock，0600+认证）。`handles` 恰为控制面注入集
    /// （`PolicyRepo` 写句柄只在此）。失败 → fail-closed `Err`。
    fn spawn_control_plane(&self, handles: &[HandleKind]) -> Result<()>;
    /// spawn sweeper 周期任务（actor=system，与控制面共用 `PolicyRepo` 写锁）。失败 →
    /// fail-closed `Err`。
    fn spawn_sweeper(&self, handles: &[HandleKind]) -> Result<()>;
}

/// 把一次 boot 结果映射为进程退出码（§8 / 公理二）。
///
/// `Ok(_)` → [`EXIT_OK`]（0）；`Err(_)` → [`EXIT_BOOT_FAILED`]（**非零**）。这是「boot Err →
/// 非零进程退出」契约的纯函数核心：`main` 据此 `process::exit(code)` / 返回 `Err`，使启动失败
/// 在进程边界可观察。无副作用、无 IO；失败码恒非零，绝不把失败误映射为 0。
pub fn boot_exit_code(result: &std::result::Result<BootReport, BootError>) -> u8 {
    match result {
        Ok(_) => EXIT_OK,
        Err(_) => EXIT_BOOT_FAILED,
    }
}

/// boot 成功后把已装配状态接线为进程对外形态：三平面各自独立 spawn（§1 唯一组装点 / §3.8）。
///
/// 数据面 router、控制面 router、sweeper 三者**各自独立** spawn 到 multi-thread 运行时（互不
/// 阻塞）；句柄集分流取自 boot 装配产物（[`BootReport::data_plane_handles`] /
/// [`BootReport::control_plane_handles`]，红线 7.2-2 在此见证：数据面 spawn 绝不收到
/// `PolicyRepo`）。任一处 spawn 失败 → fail-closed `Err`（不放行半装配进程形态）。boot 失败
/// （`data_plane_open == false`）时本函数**绝不** spawn 任何平面——data.sock 不 serving。
///
/// [`BootReport::data_plane_handles`]: crate::boot::BootReport::data_plane_handles
/// [`BootReport::control_plane_handles`]: crate::boot::BootReport::control_plane_handles
pub async fn serve_assembled<S: PlaneSpawner>(report: &BootReport, spawner: &S) -> Result<()> {
    // fail-closed 守门（公理二）：data.sock 未开放（boot 失败 / 半装配 report）时**绝不** spawn
    // 任何平面。serve_assembled 只接受「整链终结动作已完成」的报告——data_plane_open==false 意味
    // boot 在 data.sock 创建前短路，此时若仍 spawn 数据面 router 即「先开门再装锁」（fail-open）。
    // 故在任一 spawn 之前显式短路上抛 Err（不放行半装配进程形态），且一处 spawn 也不发生。
    if !report.data_plane_open {
        return Err(DaemonError::Boot);
    }
    // 三平面各自独立 spawn（§3.8 multi-thread）。句柄集分流直接取自 boot 装配产物：数据面 spawn
    // 收 data_plane_handles（红线 7.2-2：绝不含 PolicyRepo），控制面与 sweeper 共用 PolicyRepo
    // 写锁、均收 control_plane_handles。任一处 spawn 失败 `?` 即 fail-closed 短路（不放行半装配
    // 进程形态，公理二）。
    spawner.spawn_data_plane(&report.data_plane_handles)?;
    spawner.spawn_control_plane(&report.control_plane_handles)?;
    spawner.spawn_sweeper(&report.control_plane_handles)?;
    Ok(())
}

/// 进程级 boot→serve 装配主缝：把一次 boot 结果接线为**进程退出码**（§8 item 4 / 公理二）。
///
/// 这是 `main` 实际调用的端到端缝（§1 唯一组装点的进程级收尾），把「boot 失败 → 非零退出 +
/// 零 spawn」与「boot 成功 → 三平面 spawn」两条路径统一：
///
/// - boot `Err`：经 [`boot_exit_code`] 映射为 [`EXIT_BOOT_FAILED`]（**非零**），且**绝不** spawn
///   任何平面——data.sock 不 serving（boot 在 socket 创建前已短路）。
/// - boot `Ok` 但 [`serve_assembled`] 任一平面 spawn 失败：同样映射为非零退出码（不放行半装配
///   进程形态，公理二）。
/// - boot `Ok` 且三平面 spawn 全成功：返回 [`EXIT_OK`]（0）。
///
/// `main` 据返回码 `process::exit(code)`，使「boot Err → 非零进程退出 + data.sock 不 serving」
/// 在进程边界以退出码可观察（systemd/容器据此判定启动失败）。本函数无 IO、无机密，只编排已注入
/// 的 boot 结果与 [`PlaneSpawner`]——故可在集成测试里以 Fake 端到端见证「真实 main 路由的退出码
/// 语义」，无需起真实二进制。
pub async fn run_assembled<S: PlaneSpawner>(
    result: std::result::Result<BootReport, BootError>,
    spawner: &S,
) -> u8 {
    // boot 失败：先据 boot_exit_code 取非零码，且因 report 不存在，serve_assembled 根本不被调用
    // ——一处平面也不 spawn（fail-closed：boot 在 socket 创建前短路，进程非零退出、不 serving）。
    let report = match result {
        Ok(report) => report,
        Err(err) => return boot_exit_code(&Err(err)),
    };
    // boot 成功：把已装配状态 spawn 为进程对外形态；任一平面 spawn 失败 → 非零退出码（公理二）。
    match serve_assembled(&report, spawner).await {
        Ok(()) => EXIT_OK,
        Err(_) => EXIT_BOOT_FAILED,
    }
}
