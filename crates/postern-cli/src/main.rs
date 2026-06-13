//! 二进制入口 `postern`。
//!
//! 运行期形态（07-postern-cli §3.1/§3.9）：解析命令行 → 强类型管理意图 → 请求规格 →
//! 一次 HTTP-over-UDS 往返 → 渲染 → 退出码；`mcp-stdio` 子命令为数据面 `data.sock`
//! 字节桥（唯一非"一次往返 + 渲染"形态）。`main` 是唯一把 clap → 意图 → 请求规格 →
//! 传输 → 渲染串起来、并据 [`CliError`] 置进程退出码的地方。
//!
//! 错误模型（详细设计 7.1）：`anyhow` 仅允许出现在本二进制 `main`；本 crate 的库侧
//! （command/intent/dispatch/reqspec/transport/render/bridge）一律用结构化 [`CliError`]。
//! 这里直接消费 [`CliError`] 并经 [`exit_code`] 落退出码——`CliError` 是退出码的唯一来源
//! （§3.6：成功 0、本地拒绝 / daemon 不可达 / daemon 错误各互异非零码）。

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

use postern_cli::command::dispatch::dispatch;
use postern_cli::command::tree::{Cli, Command};
use postern_cli::error::{exit_code, CliError};
use postern_cli::transport::UdsTransport;

/// 默认连接 / 读取超时（§3.9：可设超时，超时按 daemon 不可达类报错；无客户端重试）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    // 解析命令行——clap 在此做语法层本地拒绝（缺参 / 互斥 / 格式），失败即退出 + 打印用法，
    // 对 `control.sock` 零请求（L-1：唯一"未发请求即失败"类别）。
    let cli = Cli::parse();
    run(cli)
}

/// 顶层运行编排（§3.1/§3.9）：把已解析的 [`Cli`] 路由到桥或控制面翻译管线，并把最终
/// `Result<(), CliError>` 据 [`exit_code`](postern_cli::error::exit_code) 落进程退出码。
fn run(cli: Cli) -> ExitCode {
    // `mcp-stdio` 是数据面字节桥（唯一非"一次往返 + 渲染"形态，§3.8），早分流到 `bridge`
    // 域；其余 22 组命令走统一控制面翻译管线（into_intent → dispatch → 渲染）。
    let outcome = match cli.command {
        Command::McpStdio => run_bridge(),
        command => run_control_plane(command),
    };

    // 成功路径把渲染好的输出写 stdout（仅成功 0 才有人类可读结果）；失败据 `CliError` 类别
    // 落互异非零退出码（§3.6），错误文本写 stderr（只转述 daemon 已脱敏事实，L-4）。
    let final_result = match outcome {
        Ok(rendered) => {
            print!("{rendered}");
            Ok(())
        }
        Err(err) => {
            report(&err);
            Err(err)
        }
    };
    ExitCode::from(exit_code(&final_result) as u8)
}

/// 控制面翻译管线（§3.1）：clap 命令 → 管理意图（含 `--cap` 本地校验，L-1）→ 一次
/// HTTP-over-UDS 往返 → 信封分流渲染。一次往返需异步栈，故在 tokio 单运行时上驱动；
/// 运行时构建失败 fail-closed 为 daemon 不可达（公理二）。
fn run_control_plane(command: Command) -> Result<String, CliError> {
    let intent = command.into_intent()?;
    let transport = UdsTransport::new(control_socket_path(), DEFAULT_TIMEOUT);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return Err(CliError::DaemonUnreachable),
    };
    runtime.block_on(dispatch(intent, &transport))
}

/// `mcp-stdio` 数据面字节桥（§3.8）：把宿主 stdin/stdout 双向逐字节转接到 `data.sock` 的
/// `/mcp` 端点。桥实现在 `bridge` 域；`data.sock` 不可连 / 转接中断即按错误终止会话、不绕过
/// daemon（L-10）。fail-closed：无可连入口即 daemon 不可达，绝不本地构造任何 MCP 响应。
fn run_bridge() -> Result<String, CliError> {
    Err(CliError::DaemonUnreachable)
}

/// 控制面 `control.sock` 路径：取自 `POSTERN_CONTROL_SOCK`（人显式指定），缺省落
/// `$XDG_RUNTIME_DIR/postern/control.sock`，再缺省 `/run/postern/control.sock`。CLI 以
/// 操作者本人 uid 连接；权限边界（`0600` + 控制面认证）是部署前置，非 CLI 设防（§3.2）。
fn control_socket_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("POSTERN_CONTROL_SOCK") {
        return PathBuf::from(explicit);
    }
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir)
            .join("postern")
            .join("control.sock");
    }
    PathBuf::from("/run/postern/control.sock")
}

/// 失败呈现（§3.6/L-2/L-4）：把 [`CliError`] 写 stderr——本地语法拒绝回显用法、daemon
/// 不可达只报"不可达"（输出无任何决策结论，L-2）、daemon 错误信封原样回显 `code`/`message`
/// （逐字转述、不补全、不推测，L-4）、响应不可解析报解码失败类别（不回显原文，L-3）。
fn report(err: &CliError) {
    match err {
        CliError::LocalReject { usage } => eprintln!("{usage}"),
        CliError::DaemonUnreachable => eprintln!("daemon unreachable"),
        CliError::DaemonError { code, message } => eprintln!("{code} {message}"),
        CliError::DecodeFailed { detail } => eprintln!("{detail}"),
    }
}
