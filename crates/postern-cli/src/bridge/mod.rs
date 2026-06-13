//! `mcp-stdio` 数据面字节桥面（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.8/§6.3，F-10，L-10）：`mcp-stdio` 是 §3 唯一**非"一次往返 +
//! 渲染"**的形态——把仅支持 stdio 的 MCP 宿主的 stdin/stdout 字节流，零逻辑双向转接到
//! daemon 数据面 `data.sock` 的 `/mcp` 端点（经 hyperlocal 连 `data.sock`，权限 `0660`/
//! 专用组，与控制面 `control.sock` 不同）。桥是字节搬运者，不是控制面客户端。
//!
//! 零逻辑搬运红线（§3.8，公理七、F-10）：桥**不**构造归一化请求、**不**解析意图、**不**归类、
//! **不**脱敏、**不**增删任何字节——归一化 [0] / 求值 [1]~[6] / 执行 [8] / 脱敏 [9] 全在
//! daemon 数据面内核完成（经 stdio 桥与直连 `data.sock` 走完全相同管线，得一致语义）。桥代码
//! 路径里**不出现**那些数据面求值族类型（构造签名可核——F-10 明列的三类引用一个都不出现）。
//!
//! 中断即终止、不绕过（§3.8，L-10）：`data.sock` 不可连或转接中断 → 桥按错误终止该会话并
//! 非零退出，**不**产生任何本地构造的 MCP 响应、**不**伪造、**不**绕过 daemon（宁可"断"
//! 也不"补"，否则即在数据面外造一条无策略 / 无审计 / 无脱敏的旁路）。
//!
//! 子模块：`stdio`（stdin↔sock 双向并发字节拷贝，任一方向 EOF/错误即收束整个会话）。

pub mod stdio;

use crate::error::CliError;

/// 桥会话的终止结果（§3.8，L-10）。
///
/// 桥**没有**"决策 / 拒绝 / 错误信封"概念——它不在 [0]~[10] 求值管线内（公理七：求值全在
/// daemon）。一个桥会话只有两种收束：双向流都正常到 EOF（`Completed`），或任一方向因连接 /
/// 转接失败而中断（`Interrupted`）。两者都不携带任何本地构造的 MCP 响应或决策——桥宁可"断"
/// 也不"补"，结构上无可携带响应的字段（L-10：不伪造、不绕过 daemon）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeOutcome {
    /// 双向流都正常到 EOF——会话自然收束（宿主关闭 stdin / daemon 关闭 sock 端）。映射成功
    /// 退出码（0）。桥不解释流过的字节，"成功"只意味着"双向搬运无 I/O 错误地走完"。
    Completed,
    /// 任一方向连接失败 / 转接中断——会话按错误终止（L-10）。映射**非零**退出码；桥**不**
    /// 产生任何本地 MCP 响应或决策。结构上无响应载荷字段——杜绝在数据面外造旁路。
    Interrupted,
}

impl BridgeOutcome {
    /// 桥会话收束的进程退出码（§3.6/§3.8，L-10）。正常双向 EOF → 0；中断（连接不可达 /
    /// 转接破裂）→ **非零**——与控制面失败语义一致地 fail-closed，但桥无信封 / 无决策可呈现。
    pub fn exit_code(self) -> i32 {
        match self {
            BridgeOutcome::Completed => crate::error::EXIT_OK,
            // 中断映射到 daemon-不可达类的同一非零码（3）：桥的中断本质就是"数据面入口不可达
            // 或转接破裂"，与控制面 `DaemonUnreachable` 同源（公理二 fail-closed）。桥**不**自
            // 造任何携响应的错误变体——若需结构化错误，复用无载荷的 [`CliError::DaemonUnreachable`]。
            BridgeOutcome::Interrupted => CliError::DaemonUnreachable.code(),
        }
    }
}

/// 字节桥的目标数据面端点（§3.8/§6.3）。
///
/// 桥连的是 daemon **数据面** `data.sock`（权限 `0660`/专用组），**不是**控制面 `control.sock`
/// （`0600`）——两者是不同 socket、不同权限边界、不同平面（§6.3 雷区）。本类型只持目标
/// `data.sock` 路径这一纯配置；桥**不**经控制面请求机制路由 MCP 字节（数据面字节不走控制面）。
#[derive(Debug, Clone)]
pub struct DataPlaneEndpoint {
    /// 目标 `data.sock` 的文件系统路径。区别于控制面 `control.sock`——桥连数据面入口。
    data_socket_path: std::path::PathBuf,
}

impl DataPlaneEndpoint {
    /// 以目标 `data.sock` 路径构造数据面端点（§6.3）。仅持纯路径配置，不建任何连接。
    pub fn new(data_socket_path: impl Into<std::path::PathBuf>) -> Self {
        DataPlaneEndpoint {
            data_socket_path: data_socket_path.into(),
        }
    }

    /// 目标 `data.sock` 路径（桥连接的数据面入口，非控制面 `control.sock`）。
    pub fn data_socket_path(&self) -> &std::path::Path {
        &self.data_socket_path
    }

    /// 连到数据面 `data.sock`，返回一条可双向读写的 UDS 流（§3.8/§6.3）。
    ///
    /// 复用 UDS-connect（`tokio::net::UnixStream::connect`），但目标是**数据面** socket 路径
    /// （非控制面）。连接失败（`data.sock` 不可连 / 移除 / 无权连 / daemon 数据面未监听）→
    /// fail-closed 为 [`CliError::DaemonUnreachable`]（L-10：中断即终止、非零退出、不绕过），
    /// **绝不**自造任何本地 MCP 响应或回退（结构上无 store/secrets 依赖即无可绕过路径）。
    pub async fn connect(&self) -> Result<tokio::net::UnixStream, CliError> {
        // 复用 transport 同款 UDS-connect（`tokio::net::UnixStream::connect`），但目标是**数据面**
        // socket 路径（非控制面）。任何连接失败（`data.sock` 缺失 / 移除 / 无权连 / daemon 数据面
        // 未监听）→ fail-closed 为 [`CliError::DaemonUnreachable`]（公理二，L-10：中断即终止、非零
        // 退出、不绕过）。**绝不**自造任何本地 MCP 响应或回退——结构上无 store/secrets 依赖即无可
        // 绕过路径，亦不经控制面请求机制路由数据面字节。
        match tokio::net::UnixStream::connect(&self.data_socket_path).await {
            Ok(stream) => Ok(stream),
            Err(_) => Err(CliError::DaemonUnreachable),
        }
    }
}

/// 运行一次完整的 `mcp-stdio` 桥会话（§3.8，F-10/L-10）。
///
/// 连数据面 `data.sock`，随后在宿主 stdin/stdout 与该 sock 之间以两个独立并发拷贝任务双向
/// 逐字节转接（见 [`stdio::pump_bidirectional`]），任一方向 EOF/错误即收束整个会话，返回
/// [`BridgeOutcome`]。连接失败 → [`BridgeOutcome::Interrupted`]（L-10）。
///
/// 本入口消费真实进程 stdin/stdout（数据面字节桥随宿主生命周期持续转接），其端到端行为以
/// 集成层覆盖；可单测的核心是 [`stdio::pump_bidirectional`] 的双向并发拷贝与收束语义。
pub async fn run_session(endpoint: &DataPlaneEndpoint) -> BridgeOutcome {
    // 连数据面 `data.sock`——不可连即中断（L-10：不回退、不自造本地响应）。
    let stream = match endpoint.connect().await {
        Ok(stream) => stream,
        Err(_) => return BridgeOutcome::Interrupted,
    };

    // 拆 sock 为读 / 写两半，分别承载 sock→host_out 与 host_in→sock 两个独立方向。
    let (sock_read, sock_write) = tokio::io::split(stream);

    // 绑真实进程 stdin/stdout——数据面字节桥随宿主生命周期持续转接。
    let host_in = tokio::io::stdin();
    let host_out = tokio::io::stdout();

    // 双向并发逐字节转接，任一方向 EOF/错误即收束会话，映射收束结果。
    stdio::pump_bidirectional(host_in, host_out, sock_read, sock_write).await
}
