//! 双向字节桥接回路（§3.2 桥接泵数据流 / §3.6 并发模型）。
//!
//! 把「本地端点读到的字节写入底层隧道」与「底层隧道读到的字节写回本地端点」两条
//! copy 回路绑成一个后台任务；**有界缓冲**承接、写慢回压、任一方向 EOF/错误推进
//! 收尾。任务边界即 `Channel` 生命周期边界：关闭即随之 abort，不留孤儿任务（§3.7）。
//! 机制层用内存管道 / loopback 充分测试（§9），泵**只搬字节、不解析协议**——不认识
//! 字节里是 SQL 还是 HTTP（那是适配器的事），是「对底层形态零感知」在搬运层的体现。
//!
//! 两方向 EOF / IO 错误即通路死活的一手信号（§3.2）：任一方向读到 EOF（对端正常半
//! 关）或遇到 RST / IO 错误，泵据此推进该方向收尾；两方向都不可搬运时泵退出，并把
//! 「死亡」事实翻进健康视图（写 [`HealthWriter::mark_dead`]）。泵退出与 `Channel`
//! 关闭互为收口：`Channel` 关闭 ⇒ 取消泵任务（cancel 触点 / `abort`）；泵因底层断开
//! 退出 ⇒ 通路转「死亡」。取消必须真正**砍在飞**——被取消时立即中止，不等待在途
//! 读写自然返回（§3.5、L-6）。

use std::sync::Arc;

use tokio::io::{copy_buf, split, AsyncRead, AsyncWrite, BufReader};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::health::HealthWriter;

/// 桥接泵单方向有界承接缓冲的固定字节数（§3.2 / §3.6 资源边界）。
///
/// 泵以**固定大小**缓冲做 `read → write` 循环承接，写慢即对读侧形成天然背压，
/// **绝不**无界堆积（无 `read_to_end` 式累积）。缓冲尺寸是编译期常量、不随运行时
/// 流量增长（有界缓冲的结构保证，非运行期回收）。
pub const BRIDGE_BUFFER_BYTES: usize = 16 * 1024;

/// 桥接泵的取消 / 收口句柄（§3.5 / §3.7）。
///
/// `Channel` 关闭/取消触点背后绑定本句柄：`cancel()` 经 [`Notify`] 让泵任务在其
/// `read`/`write` 的 `select` 分支上立即让出并退出（**砍在飞**，不等待在途读写
/// 自然返回，L-6）；`abort()` 走 [`JoinHandle::abort`] 硬中止后台任务。两条路径
/// 都保证关闭后泵任务在有限时间内终止、不留孤儿任务。
pub struct PumpHandle {
    /// 后台泵任务句柄——`abort()` 走它硬中止（§3.5 强制 abort 路径）。
    task: JoinHandle<()>,
    /// 协作取消信号——`cancel()` 触发它，泵任务在 `select` 分支上立即退出
    /// （§3.5 / L-6 砍在飞，不等待在途）。
    cancel: Arc<Notify>,
}

impl PumpHandle {
    /// 协作取消：通知泵任务在其 `read`/`write` 的 `select` 分支上立即让出并退出，
    /// **不等待**在途读写自然返回（§3.5 强制 abort / L-6 砍在飞）。
    ///
    /// 与 [`Self::abort`] 的区别：`cancel()` 是泵自身在取消点收尾（仍写「关闭」
    /// 事实），`abort()` 是从外部硬砍任务。两者都不等待在途 I/O。
    pub fn cancel(&self) {
        // 唤醒泵任务在 `select` 上等待的 `cancel.notified()` 分支：泵立即丢弃在途
        // copy future 并退出（砍在飞，不 await 在途读写自然返回，L-6）。
        self.cancel.notify_one();
    }

    /// 硬中止后台泵任务（[`JoinHandle::abort`]）：立即丢弃在途读写，任务不再触达
    /// 底层（§3.5 / §3.7 不留孤儿任务）。
    pub fn abort(&self) {
        self.task.abort();
    }

    /// 等待泵任务终止（供取消/退出收口处观测「任务确已结束」）。返回任务是否
    /// 因 [`Self::abort`] 被取消（`true` = 被 abort 取消，`false` = 自然跑完退出）。
    pub async fn join(self) -> bool {
        // `JoinHandle::await`：`Ok(())` = 任务自然跑完（含因 EOF/错误推进死亡后退出
        // 或经 cancel 协作退出）；`Err(e)` 且 `e.is_cancelled()` = 经 `abort()` 硬取消。
        // 其余 `Err`（任务体 panic）按「非 abort 取消」收口（泵任务体禁 panic，不应到达）。
        match self.task.await {
            Ok(()) => false,
            Err(e) => e.is_cancelled(),
        }
    }
}

/// 在 `local`（本地端点）与 `underlay`（底层隧道）两个字节双工端点之间架起双向
/// 桥接泵，作为后台 tokio 任务运行（§3.2 桥接泵数据流 / §3.6 并发模型）。
///
/// 语义等价 `tokio::io::copy_bidirectional`：两个方向各一条 `read → write` copy
/// 回路，以 [`BRIDGE_BUFFER_BYTES`] 有界缓冲承接，写慢即对读侧形成天然背压、**不
/// 无界堆积**。泵**只搬字节、不解析协议**。任一方向 EOF / IO 错误推进该方向收尾；
/// 两方向都不可搬运时泵退出，并向 `health` 写入「死亡」（[`HealthWriter::mark_dead`]，
/// §3.2 泵退出与死亡互为收口、F-4）。返回 [`PumpHandle`] 供 `Channel` 关闭/取消触点
/// 绑定（cancel 砍在飞 / abort 硬中止，§3.5 / L-6）。
///
/// 端点为泛型 `AsyncRead + AsyncWrite`：机制层测试以内存双工管道（`tokio::io::duplex`）
/// 或 loopback `TcpStream` 充当两端，无需真实远端、无需机密类型（§9）。
pub fn spawn_bridge<L, U>(mut local: L, mut underlay: U, health: HealthWriter) -> PumpHandle
where
    L: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    U: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let cancel = Arc::new(Notify::new());
    let task_cancel = Arc::clone(&cancel);

    let task = tokio::spawn(async move {
        // 拆出两端的读/写半，组两条独立的单向 copy 回路（§3.2 桥接泵两方向各一条
        // `read → write`）。每条回路把读半包进容量恰为 [`BRIDGE_BUFFER_BYTES`] 的
        // [`BufReader`]，再以 [`copy_buf`] 经该缓冲承接：底层读单次最多被请求
        // `BRIDGE_BUFFER_BYTES` 字节（缓冲尺寸即搬运承接上界，常量真正接入搬运回路，
        // 非装饰），写慢即对读侧形成天然背压、不无界堆积。泵只搬字节、不窥探内容
        // （不解析协议）。
        let (local_rd, mut local_wr) = split(&mut local);
        let (underlay_rd, mut underlay_wr) = split(&mut underlay);
        let mut local_rd = BufReader::with_capacity(BRIDGE_BUFFER_BYTES, local_rd);
        let mut underlay_rd = BufReader::with_capacity(BRIDGE_BUFFER_BYTES, underlay_rd);

        // 前向：本地端点 → 底层隧道。
        let forward = copy_buf(&mut local_rd, &mut underlay_wr);
        // 反向：底层隧道 → 本地端点。
        let reverse = copy_buf(&mut underlay_rd, &mut local_wr);

        tokio::select! {
            // 协作取消：被通知即丢弃在途 copy future 立刻退出（砍在飞，不 await
            // 在途读写自然返回，L-6）；写「关闭」事实——这是按指令收口非通路死亡。
            _ = task_cancel.notified() => {
                health.mark_closed();
            }
            // 任一方向收尾（EOF / RST / IO 错误使该方向的 copy 回路完结）即通路死亡的
            // 一手信号：一个端点（本地或底层隧道）一旦断开，整条桥接通路即不可再搬运，
            // 泵随之退出并把死亡翻进健康视图（§3.2 泵退出与死亡互为收口、F-4）。无论
            // `Ok`（EOF）还是 `Err`（RST/IO 错误）都收口为死亡——IO 错误即作死活信号，
            // 绝不被静默吞掉、不留伪 `Alive` 通路。
            _ = forward => {
                health.mark_dead();
            }
            _ = reverse => {
                health.mark_dead();
            }
        }
    });

    PumpHandle { task, cancel }
}
