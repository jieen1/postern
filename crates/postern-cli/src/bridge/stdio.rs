//! stdin↔sock 双向字节拷贝（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.8/§3.9，F-10，L-10）：以 tokio 两个方向各一个异步拷贝任务承载
//! `host(stdin) → data.sock` 与 `data.sock → host(stdout)` 两个独立字节流，**并发**搬运（一端
//! 阻塞不得卡死另一端）；任一方向 EOF / 错误即取消另一方向、收束整个会话。桥随宿主进程生命
//! 周期持续转接（1 条长连接），不属控制面"一次往返"模型。
//!
//! 逐字节恒等纪律（§3.8，F-10）：读回的字节序列与写入逐字节相等（含任意二进制 / 分片）；
//! 桥内不解析、不归类、不脱敏、不增删字节。任一字节被改写 / 桥内出现解析即不过 F-10。
//! `data.sock` 不可连或转接中断 → 按错误终止会话、非零退出，无任何本地构造的 MCP 响应或
//! 决策（L-10）。

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use super::BridgeOutcome;

/// 入向（host_in）EOF 后给 sock→host_out 方向的有界排空窗口：宿主关闭 stdin 后，先半关
/// sock 写半流（向数据面端点示意"入向已尽"），再让出向把端点据此回送的剩余字节排空到宿主
/// 出向，随后收束（§3.8：入向 EOF 即收束会话；同时不截断已在途的出向字节）。窗口仅是优雅
/// 排空的上界——出向自然到 EOF 即立刻返回，远早于此（数据面端点不回送则到窗口即止，不挂死）。
const HOST_IN_EOF_DRAIN_GRACE: Duration = Duration::from_millis(250);

/// 在宿主 stdin/stdout 与数据面 sock 之间做**双向并发**逐字节转接（§3.8，F-10/L-10）。
///
/// 形态（§3.8/§3.9）：起**两个**独立的方向各一个异步拷贝任务——
/// - `host_in → sock`（宿主 stdin 进、写入 data.sock）；
/// - `sock → host_out`（data.sock 出、写入宿主 stdout）。
///
/// 两个方向**并发**运行（一端阻塞不得卡死另一端）；**任一方向** EOF 或 I/O 错误即取消另一
/// 方向、收束整个会话——单向 EOF/错误必须连带终止反向流，不得让反向流孤悬挂死。
///
/// 收束语义（返回 [`BridgeOutcome`]）：
/// - 两个方向都正常到 EOF（无 I/O 错误）→ [`BridgeOutcome::Completed`]；
/// - 任一方向 I/O 错误 / 转接中断 → [`BridgeOutcome::Interrupted`]（L-10：会话按错误终止）。
///
/// 逐字节恒等（F-10）：本函数只搬字节——读多少写多少、原样转发，**不**解析、**不**归类、
/// **不**脱敏、**不**增删任何字节（含任意二进制 / 任意分片边界）。它是 mover，不是 interpreter。
///
/// 泛型签名（可单测的核心）：宿主侧与 sock 侧均以 `AsyncRead`/`AsyncWrite` 抽象——测试可用
/// 内存双工流对接回声 Fake MCP 端点驱动，无需真实 stdin/stdout 或真实 daemon（§9 测试策略）。
pub async fn pump_bidirectional<HostIn, HostOut, SockRead, SockWrite>(
    mut host_in: HostIn,
    mut host_out: HostOut,
    mut sock_read: SockRead,
    mut sock_write: SockWrite,
) -> BridgeOutcome
where
    HostIn: AsyncRead + Unpin,
    HostOut: AsyncWrite + Unpin,
    SockRead: AsyncRead + Unpin,
    SockWrite: AsyncWrite + Unpin,
{
    // 收束语义先在一个借用块内判定，块结束即释放两个拷贝 future 对四个半流的借用，随后才半关
    // 宿主出向写半流（向宿主示意"会话已尽 / 出向无更多字节"）——否则出向消费侧读不到 EOF。
    let outcome = {
        // 宿主出向方向（`sock_read → host_out`）的拷贝 future 贯穿整个会话——它承载数据面端点
        // 回送的字节，且在入向 EOF 后仍需把在途回声排空到宿主出向。先固定它，入向方向再与之并发竞争。
        let downstream = tokio::io::copy(&mut sock_read, &mut host_out);
        tokio::pin!(downstream);

        // 入向方向（`host_in → sock_write`）与出向并发推进，取首个收束者（§3.8 双向并发：一端阻塞
        // 不卡死另一端；任一方向先收束即取消另一方向——未中选的 future 被 drop 即中止，不让反向流孤悬）。
        // `tokio::io::copy` 只搬字节：读多少写多少、原样转发、收尾 flush，不解析 / 不归类 / 不脱敏 /
        // 不增删任一字节（F-10）。它返回 `Ok` 当且仅当读到**干净 EOF**，返回 `Err` 当任一侧 I/O 失败。
        let upstream_result = {
            let upstream = tokio::io::copy(&mut host_in, &mut sock_write);
            tokio::pin!(upstream);
            tokio::select! {
                up = &mut upstream => Some(up),
                // 宿主出向方向先收束：数据面端点关闭 / 转接破裂——无论 sock 侧读到 EOF 还是 I/O
                // 错误，都意味着数据面入口在宿主仍活时断开，按中断终止会话（L-10：中断即终止、非零
                // 退出、不绕过 daemon、不自造任何本地 MCP 响应）。`None` 标记"出向先收束"，入向 future
                // 在此块结束时被 drop（取消入向，不让宿主入向孤悬挂死，§3.8）。
                _down = &mut downstream => None,
            }
            // 入向方向的拷贝 future 在此块末尾被 drop，其对 `sock_write` 的借用随之释放。
        };

        match upstream_result {
            // 宿主入向（host_in）方向先收束：
            Some(Ok(_)) => {
                // 干净 EOF（宿主关闭 stdin）→ 正常会话收束（§3.8）。先半关 sock 写半流向数据面端点
                // 示意"入向已尽"，再在**有界**排空窗口内把出向在途字节排空到宿主出向（不截断回声）——
                // 窗口内出向自然到 EOF 即立刻返回；窗口届满（端点不回送）即收束，绝不挂死等反向 EOF。
                let _ = sock_write.shutdown().await;
                let _ = tokio::time::timeout(HOST_IN_EOF_DRAIN_GRACE, &mut downstream).await;
                BridgeOutcome::Completed
            }
            // 入向 I/O 错误（写 sock 失败 = 转接破裂）→ 按错误终止会话（L-10）。
            Some(Err(_)) => BridgeOutcome::Interrupted,
            // 出向方向先收束（数据面端点关闭 / 转接破裂）→ 按中断终止会话（L-10）。
            None => BridgeOutcome::Interrupted,
        }
    };

    // 会话收束后半关宿主出向写半流——把已写入的全部字节落定并向出向消费侧（宿主 stdout）示意
    // EOF，使其读尽收尾，不孤悬挂死（§3.8）。桥不在此增删任何字节，仅做流的正常收尾。
    let _ = host_out.shutdown().await;

    outcome
}
