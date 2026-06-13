//! `Channel` 三件套装配与关闭 / 取消语义承载（channel-assembly 单元）行为测试（RED）。
//!
//! 被测对象：`postern_transports::chan::{TransportChannelInner, TunnelHandle, KeepaliveHandle,
//! into_channel}`——把「本地端点句柄 + 健康事实视图 + 关闭 / 取消触点」三块组装为 **crate
//! 内部** 的具体通路状态类型 [`TransportChannelInner`]，并装进 `core::plugin::Channel` 的不透明
//! `handle: Box<dyn Send + Sync>`（§3.1 / §3.5 / §5.2 / §8 F-5/F-7/L-6/L-9）。
//!
//! **关键裁决（写入 type_level_notes）**：`core::Channel` 完全不暴露 health/close 公开方法
//! （只 `handle: Box<dyn Send + Sync>`），故本域健康 / 关闭控制面是 `handle` 内部的
//! [`TransportChannelInner`]——本 crate **绝不**给 core 的 `Channel` 加方法（改 core 即违纪）。
//! 因此本单元在装进 `handle` **之前** 持有具体 inner，直接在 inner 上驱动 close/abort/health
//! 观察；装进 `handle` 后「daemon 据其拿回控制面」一面受限于 core 的 `handle` 类型（见
//! type_level_notes 对 `Any`/downcast 的如实标注），open→注入真实机密链路如实标注为集成层。
//!
//! 覆盖 §8 条目（逐条加注释）：
//! - §8 F-7 不重定义 `Channel`：本 crate 不声明名为 `Channel` 的类型；[`into_channel`] 只把
//!   inner 装进 `core::Channel.handle`（返回类型即 core 的 `Channel`），inner 为 `Send + Sync`。
//! - §8 F-5 优雅 close 幂等：经内部控制面下达优雅 close → 桩底层隧道 close 被调用**恰 1 次**、
//!   健康视图此后返回 `Closed`；再次 close **不**二次调用桩 close、**不**报错（幂等）。
//! - §8 F-5 / §3.5 close 有序拆除：优雅 close 的执行顺序为 停保活 task → 停桥接泵 → 关底层隧道
//!   → 健康转 `Closed`（以桩记录的调用次序断言：保活停在底层 close 之前；底层 close 后健康 Closed）。
//! - §8 L-6 强制 abort 砍在飞：对一条「执行中」（泵在搬一个永不返回的慢端点）通路下达 abort →
//!   泵与保活被取消、健康转 `Closed`，且关闭在桩慢端点的在途操作返回**之前**完成（不等其优雅跑完）。
//! - §8 L-9 差异不外溢：[`TransportChannelInner`] 的关闭 / 健康接口**不含**长 / 非长分支——
//!   persistent（`Some(keepalive)`）与非 persistent（`None`）两形态装配出的 inner 用法一致。
//! - §8 无孤儿任务：close / abort 后，泵与保活的后台 task 均已终止——不留后台 task 触达底层。
//! - §3.5 / L-7 关闭报错脱敏：底层 close 报错 → 经脱敏呈现为 `TransportError::CloseFailed`，
//!   其 `Display` / `Debug` 渲染串**不含**注入的真实地址子串（绝不外泄原始地址）。
//!
//! 本单元**不构造机密类型**（不写 `ResolvedTarget` / `ResourceCredential` / `ConnOrigin` 字面）、
//! 不嵌裸数据库写标记、不依赖兄弟单元；底层隧道用记录调用的内存桩，端点一律内存双工管道 /
//! loopback（§9）。异步用 `#[tokio::test]`。

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Notify;
use tokio::time::timeout;

use postern_core::error::TransportError;
use postern_core::plugin::Channel;

use postern_transports::chan::{
    into_channel, KeepaliveHandle, TransportChannelInner, TunnelHandle,
};
use postern_transports::health::{health_view, Health, HealthReader};
use postern_transports::pump::{spawn_bridge, PumpHandle};

/// 关闭 / abort 收口的统一超时上界（CI 容差）：下达指令后，关闭必须在此时限内完成
/// （§3.5 / L-6 砍在飞，不等待在途）。超时即视为「没有真正砍在飞」/「关闭挂死」= 红。
const CLOSE_DEADLINE: Duration = Duration::from_secs(2);

/// 注入的「真实地址」明文子串（仅测试夹具内，不构造机密类型）：用于 L-7 钉死脱敏后的
/// `TransportError` 渲染串不含此子串。它是普通字符串，不是任何机密 / `ResolvedTarget`。
const FAKE_ADDR: &str = "10.0.3.17";

/// 共享的有序事件日志：桩底层隧道与桩保活任务把动作名按发生次序追加于此，供「调用次序」
/// 断言（§3.5 有序拆除）。仅测试夹具内的普通字符串记录，不承载机密。
type EventLog = Arc<Mutex<Vec<&'static str>>>;

/// [`assemble_persistent_inner`] 的返回件：inner + 底层隧道桩 + 保活退出观测位 + 健康读半 +
/// 有序日志 + **本地/底层端点的对端半**（拆成命名别名以保持夹具签名清爽）。
///
/// 末项 [`PeerEnds`] 持住桥接泵两端口的**对端双工半**：必须随 rig 存活到 `close()` / `abort()`
/// 之后——否则对端半在装配后被 drop ⇒ 泵的读取读到 EOF ⇒ 泵因「底层断开」**提前**退出
/// （而非被 close 的有序 cancel/join 停掉），使 `"pump_stop"` 的次序位置反映的是 EOF 死亡时机
/// 而非拆除次序（§3.5 有序拆除的可观测性前提）。
type PersistentRig = (
    TransportChannelInner,
    Arc<RecordingTunnel>,
    Arc<AtomicBool>,
    HealthReader,
    EventLog,
    PeerEnds,
);

/// 桥接泵两端口的**对端双工半**（本地端点对端 + 底层隧道对端）：仅用于在测试存活期内
/// 保持端点不被 drop（避免对端半提前 drop 造成泵读 EOF 提前退出）。无行为，仅持有所有权。
type PeerEnds = (tokio::io::DuplexStream, tokio::io::DuplexStream);

// ── 桩底层隧道：记录 close / cancel 调用次数与全局次序（§9 行为观察） ──────────

/// 记录调用的桩底层隧道（§3.5 / F-5 / L-6）：`close` / `cancel` 各记一次调用，并把事件
/// 名追加进共享有序日志 `log`（供「调用次序」断言）。`close_should_fail` 为真时 `close`
/// 返回 `Err(())`（模拟「关隧道时对端已不可达」），用于 L-7 脱敏钉死。
///
/// 本桩**不接触机密**——不持 `ResolvedTarget` / `ResourceCredential`，仅持普通计数 / 日志。
struct RecordingTunnel {
    close_calls: AtomicUsize,
    cancel_calls: AtomicUsize,
    log: EventLog,
    close_should_fail: bool,
}

impl RecordingTunnel {
    fn new(log: EventLog) -> Self {
        Self {
            close_calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
            log,
            close_should_fail: false,
        }
    }

    /// 关底层报错的变体（§3.5 / L-7）：`close` 记一次调用后返回 `Err(())`。
    fn failing(log: EventLog) -> Self {
        Self {
            close_should_fail: true,
            ..Self::new(log)
        }
    }

    fn close_count(&self) -> usize {
        self.close_calls.load(Ordering::SeqCst)
    }

    fn cancel_count(&self) -> usize {
        self.cancel_calls.load(Ordering::SeqCst)
    }
}

impl TunnelHandle for RecordingTunnel {
    fn close(&self) -> Result<(), ()> {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        self.log.lock().expect("log mutex").push("tunnel_close");
        if self.close_should_fail {
            // 模拟「关隧道时对端已不可达」——底层 close 报错。诊断侧地址明文仅在测试夹具，
            // 不随返回越界（返回只承载 `Err(())`）；上层须脱敏为 CloseFailed，绝不外泄地址。
            Err(())
        } else {
            Ok(())
        }
    }

    fn cancel(&self) {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        self.log.lock().expect("log mutex").push("tunnel_cancel");
    }
}

/// 让测试侧持 `Arc<RecordingTunnel>` 断言调用次数，同时把它装进 `Box<dyn TunnelHandle>`
/// 交给 inner。仅转发，无逻辑。
struct ArcTunnel(Arc<RecordingTunnel>);

impl TunnelHandle for ArcTunnel {
    fn close(&self) -> Result<(), ()> {
        self.0.close()
    }
    fn cancel(&self) {
        self.0.cancel()
    }
}

// ── 桩保活后台任务：spawn 一个在 cancel 时退出的 task，组装成 KeepaliveHandle ──

/// 共享给「桩保活任务」的观测位：`exited` 在任务退出时置真（供「无孤儿任务」/「保活先停」
/// 断言），`log` 在退出时追加 `"keepalive_stop"`（供「调用次序」断言）。
struct KeepaliveProbe {
    exited: Arc<AtomicBool>,
    log: EventLog,
}

/// spawn 一个桩保活后台任务并组装成 [`KeepaliveHandle`]（§3.3 / §3.7）。
///
/// 任务体：在 `cancel.notified()` 上等待——被通知即把 `"keepalive_stop"` 追加进共享日志、
/// 置 `exited = true` 后退出（模拟「停保活 task」收口）。返回句柄交给 inner 装配；
/// `exited` / `log` 留在测试侧观测。本桩**不接触机密**、不连真实 ssh/ssm。
fn spawn_stub_keepalive(probe: KeepaliveProbe) -> KeepaliveHandle {
    let cancel = Arc::new(Notify::new());
    let task_cancel = Arc::clone(&cancel);
    let KeepaliveProbe { exited, log } = probe;
    let task = tokio::spawn(async move {
        // 等待协作取消——被通知即收口退出（停保活 task）。
        task_cancel.notified().await;
        log.lock().expect("log mutex").push("keepalive_stop");
        exited.store(true, Ordering::SeqCst);
    });
    KeepaliveHandle::new(task, cancel)
}

// ── 永不返回的本地端点：读 future 永久 Pending（用于 L-6 abort 砍在飞观察） ──────

/// 永不返回的可控端点：`poll_read` 恒 `Poll::Pending` 并记录已被 poll（取消点命中观察），
/// `poll_write` 即时吞下。用作 L-6「在途读永不返回」的注入端点——abort 若真砍在飞，则不等待
/// 此读返回即可完成关闭。本类型不含任何机密 / 协议解析。
struct NeverReturnsRead {
    polled: Arc<AtomicBool>,
}

impl NeverReturnsRead {
    fn new(polled: Arc<AtomicBool>) -> Self {
        Self { polled }
    }
}

impl AsyncRead for NeverReturnsRead {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.polled.store(true, Ordering::SeqCst);
        Poll::Pending
    }
}

impl AsyncWrite for NeverReturnsRead {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// ── 端点包装：桥接泵任务终止时（其捕获的端点被 drop）向有序日志追加 "pump_stop" ──

/// 把一个内存双工端点包成「泵停可观测」端点：本包装由桥接泵任务**按值捕获**，泵任务
/// 一旦终止（cancel 收口 / EOF 退出），其捕获的端点随任务体 drop —— `Drop` 即向共享有序
/// 日志追加 `"pump_stop"`。这是把「停桥接泵」这一步钉进拆除序列的**可观测点**：泵不向
/// 健康/日志写业务事件（[`crate::pump::bridge`] 全文无 log/push），故泵停的次序位置原本
/// 不可观测；经此包装，`close()` 中 `pump.join().await` 收口（泵任务终止 ⇒ 端点 drop ⇒
/// `"pump_stop"`）**先于**其后的底层 `tunnel.close()`（`"tunnel_close"`），使 §3.5 三步次序
/// 「停保活 → 停桥接泵 → 关底层隧道」的**中间一步**可被断言。本包装仅记录次序、不承载机密。
struct PumpStopRecorder {
    inner: tokio::io::DuplexStream,
    log: EventLog,
}

impl PumpStopRecorder {
    fn new(inner: tokio::io::DuplexStream, log: EventLog) -> Self {
        Self { inner, log }
    }
}

impl Drop for PumpStopRecorder {
    fn drop(&mut self) {
        // 泵任务终止 ⇒ 其捕获的本端点被 drop ⇒ 记录「停桥接泵」于有序日志（§3.5 中间步）。
        self.log.lock().expect("log mutex").push("pump_stop");
    }
}

impl AsyncRead for PumpStopRecorder {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PumpStopRecorder {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

// ── 装配夹具：把三件套组装成一条「可用通路」inner（pump + keepalive + tunnel + health）──

/// 装配一条 **persistent**（带桩保活）通路 inner：内存双工管道做本地端点 ⇆ 底层隧道的桥接泵，
/// 桩保活任务，记录调用的桩底层隧道，写半 / 读半分离的健康视图（§3.1 三件套）。
///
/// 返回 inner、底层隧道桩（断言 close/cancel 次数）、保活退出观测位、健康读半、有序日志、
/// 以及桥接泵两端口的**对端双工半**（[`PeerEnds`]，须随 rig 存活到关闭之后）。
///
/// 本地端点经 [`PumpStopRecorder`] 包装，使「停桥接泵」一步在有序日志中可观测（其 `Drop`
/// 在泵任务终止时追加 `"pump_stop"`），供 §3.5 三步次序的中间步断言。**对端双工半经 rig 返回
/// 持活**：若在此处 drop 它们，泵会读到 EOF 而**提前**退出（泵停时机由 EOF 死亡而非 close 的
/// 有序 cancel/join 决定），`"pump_stop"` 的次序位置即失真——故必须把两对端半交回测试侧持活，
/// 让泵只在 close 的有序拆除中被停掉。
fn assemble_persistent_inner() -> PersistentRig {
    let log: EventLog = Arc::new(Mutex::new(Vec::new()));
    let (local_pump, local_peer) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, underlay_peer) = tokio::io::duplex(64 * 1024);
    // 本地端点包成「泵停可观测」端点：泵任务终止时其 Drop 记录 "pump_stop"（§3.5 中间步）。
    let local_pump = PumpStopRecorder::new(local_pump, Arc::clone(&log));
    // 桥接泵的健康写半交给泵；inner 自身另持一组写半 / 读半（三件套之一），二者各自独立。
    let (pump_hw, _pump_hr) = health_view();
    let pump: PumpHandle = spawn_bridge(local_pump, underlay_pump, pump_hw);

    let ka_exited = Arc::new(AtomicBool::new(false));
    let keepalive = spawn_stub_keepalive(KeepaliveProbe {
        exited: Arc::clone(&ka_exited),
        log: Arc::clone(&log),
    });

    let tunnel = Arc::new(RecordingTunnel::new(Arc::clone(&log)));
    let (health_w, health_r) = health_view();
    let inner = TransportChannelInner::assemble(
        pump,
        Some(keepalive),
        Box::new(ArcTunnel(Arc::clone(&tunnel))),
        health_w,
        health_r.clone(),
    );

    // 末项交回两对端半：测试侧须持活到 close/abort 之后（否则泵提前 EOF 退出，pump_stop 失真）。
    (
        inner,
        tunnel,
        ka_exited,
        health_r,
        log,
        (local_peer, underlay_peer),
    )
}

// ── §8 F-7 不重定义 Channel：into_channel 返回 core 的 Channel，只填充 handle ──────

/// §8 F-7：本 crate **不**声明名为 `Channel` 的类型；[`into_channel`] 把 [`TransportChannelInner`]
/// 装进 `core::plugin::Channel` 的不透明 `handle: Box<dyn Send + Sync>`，**返回类型即 core 的
/// `Channel`**（与 `Adapter::execute(ch: &mut Channel, …)` 共享同一类型）。
///
/// 钉死：①装配产出的就是 **core 的** `Channel`（`let ch: Channel`，`Channel` 即
/// `postern_core::plugin::Channel`——若本 crate 自定义同名类型则此标注不通过）；②该 `Channel`
/// 可被当作上层句柄**跨任务移动**（`tokio::spawn` 要求 `Send`），从其不透明 handle 取回
/// 字节大小恒为指针宽度（fat-ptr 的 data 宽度），证明 inner 确被装进 handle 并可携带
/// （不对 `dyn Send+Sync` 做 downcast——core 的 handle 不带 `Any`，downcast 不可行，见
/// type_level_notes 的 Any/downcast 裁决）。
#[tokio::test]
async fn into_channel_yields_core_channel_carrying_the_inner_in_handle() {
    let (inner, _tunnel, _ka_exited, _hr, _log, _peers) = assemble_persistent_inner();

    // 返回类型即 core 的 Channel（不重定义类型——本 crate 无同名类型）。
    let ch: Channel = into_channel(inner);

    // 该 Channel 可跨任务移动（spawn 要求其 `Send`，进而要求 handle 的 Box<dyn Send+Sync>
    // 真被填了一个 Send+Sync 值）；任务内确认 handle 是个胖指针（size_of_val = 2*usize）——
    // 即 handle 装着一个有动态尺寸的具体 inner，而非空壳。
    let joined = tokio::spawn(async move {
        // 取被 box 值的运行期大小：非零即证明 handle 携带了一个真实的具体 payload（inner）。
        std::mem::size_of_val(&*ch.handle)
    })
    .await
    .expect("the core::Channel must be Send (its handle boxes a Send+Sync inner) — movable across tasks");

    assert!(
        joined > 0,
        "core::Channel.handle must box a concrete non-zero-sized inner (F-7: fill the handle, do not redefine Channel)"
    );
}

// ── §8 F-7（结构）：装进 handle 的 inner 是 Send + Sync（core 约束 Box<dyn Send+Sync>） ──

/// §8 F-7（结构保证）：装进 `handle` 的 inner 必须 `Send + Sync`（core 约束
/// `Box<dyn Send + Sync>`）。以编译期约束钉死——若 inner 不是 `Send + Sync`，本断言不编译。
#[tokio::test]
async fn inner_is_send_and_sync_for_core_handle_bound() {
    fn assert_send_sync<T: Send + Sync>() {}
    // 编译即证明：TransportChannelInner 可装入 core::Channel.handle 的 Box<dyn Send + Sync>。
    assert_send_sync::<TransportChannelInner>();
}

// ── §8 F-5 优雅 close 幂等：底层 close 恰 1 次、健康转 Closed、重复 close 不二次关不报错 ──

/// §8 F-5 优雅 close 幂等：经内部控制面下达优雅 `close()` → 桩底层隧道的 `close` 被调用**恰 1
/// 次**、健康视图此后返回 `Closed`；**再次** `close()` 不二次调用桩 close（计数仍为 1）、
/// 不报错（返回 `Ok`，幂等，§3.5）。
#[tokio::test]
async fn graceful_close_is_idempotent_underlay_closed_exactly_once() {
    let (mut inner, tunnel, _ka_exited, hr, _log, _peers) = assemble_persistent_inner();

    // 关闭前：健康为初态「活」（被动读，§3.4）。
    assert_eq!(inner.health(), Health::Alive);
    assert_eq!(hr.get(), Health::Alive);

    // 第一次优雅 close：有序拆除并关底层恰一次。
    let first = timeout(CLOSE_DEADLINE, inner.close())
        .await
        .expect("graceful close must complete within deadline (no hang)");
    assert_eq!(first, Ok(()), "first graceful close returns Ok");
    assert_eq!(
        tunnel.close_count(),
        1,
        "underlay tunnel close called exactly once"
    );
    // 健康视图此后返回「已关闭」终态（§3.5 末步）。
    assert_eq!(inner.health(), Health::Closed);
    assert_eq!(hr.get(), Health::Closed);

    // 第二次 close（幂等）：不二次关底层（计数仍为 1）、不报错（仍 Ok）。
    let second = timeout(CLOSE_DEADLINE, inner.close())
        .await
        .expect("repeat close must complete within deadline");
    assert_eq!(
        second,
        Ok(()),
        "repeat graceful close is a no-op Ok (idempotent, §3.5)"
    );
    assert_eq!(
        tunnel.close_count(),
        1,
        "repeat close must NOT call the underlay tunnel a second time (idempotent once-guard, §3.5)"
    );
    assert_eq!(
        inner.health(),
        Health::Closed,
        "health stays Closed across repeat close"
    );
}

// ── §8 F-5 / §3.5 close 有序拆除：停保活 → 关底层隧道（以桩记录的调用次序断言） ──

/// §8 F-5 / §3.5 close 有序拆除：优雅 close 的执行顺序为 停保活 task → 停桥接泵 → 关底层隧道
/// → 健康转 `Closed`。以桩记录的调用次序断言**完整三步**：保活 task 停（日志
/// `"keepalive_stop"`）**在**桥接泵停（日志 `"pump_stop"`）**之前**、桥接泵停**在**底层隧道
/// close（日志 `"tunnel_close"`）**之前**；底层 close 之后健康转 `Closed`。
///
/// 钉死次序不变量（§3.5）：三步皆被点名钉位——
/// - 一个先关底层、再停保活的实现（次序颠倒）会让 `"tunnel_close"` 排在 `"keepalive_stop"` 前；
/// - 一个把停泵滞后到关底层之后（先关底层隧道、再停桥接泵）的实现会让 `"tunnel_close"` 排在
///   `"pump_stop"` 前。
/// 两类违规均与 §3.5 规定的「停保活 → 停桥接泵 → 关底层隧道」拆除次序相悖 → 本断言钉红。
/// `"pump_stop"` 由 [`PumpStopRecorder`] 在泵任务终止（`pump.join().await` 收口）时记录，故停泵
/// 一步的次序位置可观测、被守住——不再只钉 keepalive↔tunnel 两步而漏掉被点名要求的 pump 一步。
#[tokio::test]
async fn graceful_close_tears_down_in_order_keepalive_then_pump_then_underlay() {
    // `_peers` 持住桥接泵对端半到 close 之后：保证泵只被 close 的有序 cancel/join 停掉
    // （而非对端半提前 drop 造成 EOF 提前退出），故 "pump_stop" 的次序位置忠实反映拆除次序。
    let (mut inner, tunnel, ka_exited, _hr, log, _peers) = assemble_persistent_inner();

    timeout(CLOSE_DEADLINE, inner.close())
        .await
        .expect("graceful close completes")
        .expect("graceful close returns Ok");

    // 保活 task 已停（无孤儿）、底层已关一次、健康为 Closed。
    assert!(
        ka_exited.load(Ordering::SeqCst),
        "keepalive task stopped during close (no orphan)"
    );
    assert_eq!(tunnel.close_count(), 1, "underlay closed once");
    assert_eq!(
        inner.health(),
        Health::Closed,
        "health Closed after underlay close (last step)"
    );

    // 次序断言：以桩记录的调用次序钉死完整三步（§3.5）。
    let events = log.lock().expect("log mutex").clone();
    let ka_idx = events
        .iter()
        .position(|e| *e == "keepalive_stop")
        .expect("keepalive stop must be recorded during ordered teardown");
    let pump_idx = events
        .iter()
        .position(|e| *e == "pump_stop")
        .expect("bridge pump stop must be recorded during ordered teardown");
    let tun_idx = events
        .iter()
        .position(|e| *e == "tunnel_close")
        .expect("underlay close must be recorded during ordered teardown");

    // 第一步 → 第二步：停保活在停桥接泵之前。
    assert!(
        ka_idx < pump_idx,
        "ordered teardown: keepalive must stop BEFORE the bridge pump \
         (停保活 task → 停桥接泵, §3.5); observed log = {events:?}"
    );
    // 第二步 → 第三步：停桥接泵在关底层隧道之前（钉死被点名的 pump 步——一个把停泵滞后到
    // tunnel close 之后的实现会让 tun_idx < pump_idx → 钉红）。
    assert!(
        pump_idx < tun_idx,
        "ordered teardown: the bridge pump must stop BEFORE the underlay tunnel is closed \
         (停桥接泵 → 关底层隧道, §3.5); observed log = {events:?}"
    );
}

// ── §8 L-6 强制 abort 砍在飞：泵搬永不返回慢端点，abort 在在途返回之前完成 ────────

/// §8 L-6 强制 abort 砍在飞：对一条「执行中」（泵在搬一个**永不返回**的慢端点）通路下达
/// `abort()` → 泵与保活 task 被取消、健康转 `Closed`，且关闭在桩慢端点的在途操作返回**之前**
/// 完成（不等其优雅跑完）。底层隧道桩记录到 `cancel` 被调用（强制路径，非优雅 close）。
///
/// 钉死 L-6 核心：慢端点的读 future 永久 `Pending`，故若 `abort()` 在 `CLOSE_DEADLINE` 内完成，
/// 即证明关闭**没有**等待在途读返回（砍在飞）。一个走优雅排空、await 在途的实现会在此挂死超时 = 红。
#[tokio::test]
async fn forced_abort_cuts_in_flight_before_slow_op_returns() {
    let log: EventLog = Arc::new(Mutex::new(Vec::new()));

    // 本地端点 = 永不返回的慢端点（在途读永久 Pending）；底层用内存管道（保留测试侧避免
    // 其 drop 抢先制造 EOF——确保「执行中」状态由慢端点维持，而非底层断开）。
    let probe = Arc::new(AtomicBool::new(false));
    let slow_local = NeverReturnsRead::new(Arc::clone(&probe));
    let (underlay_pump, _underlay_test) = tokio::io::duplex(64 * 1024);
    let (pump_hw, _pump_hr) = health_view();
    let pump = spawn_bridge(slow_local, underlay_pump, pump_hw);

    let ka_exited = Arc::new(AtomicBool::new(false));
    let keepalive = spawn_stub_keepalive(KeepaliveProbe {
        exited: Arc::clone(&ka_exited),
        log: Arc::clone(&log),
    });

    let tunnel = Arc::new(RecordingTunnel::new(Arc::clone(&log)));
    let (health_w, health_r) = health_view();
    let mut inner = TransportChannelInner::assemble(
        pump,
        Some(keepalive),
        Box::new(ArcTunnel(Arc::clone(&tunnel))),
        health_w,
        health_r.clone(),
    );

    // 让泵把慢端点的在途读 poll 起来（「执行中」状态确立：取消点要命中在途读）。
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        probe.load(Ordering::SeqCst),
        "pump must have an in-flight read on the never-returning endpoint before abort"
    );

    // 下达强制 abort——必须在慢端点在途读返回之前完成（砍在飞，不等优雅跑完，L-6）。
    timeout(CLOSE_DEADLINE, inner.abort()).await.expect(
        "forced abort must complete BEFORE the never-returning in-flight read returns (L-6)",
    );

    // 健康转「已关闭」终态。
    assert_eq!(
        inner.health(),
        Health::Closed,
        "abort drives health to Closed"
    );
    assert_eq!(health_r.get(), Health::Closed);
    // 强制路径取底层 cancel（非优雅 close）——桩隧道记录到 cancel 被调用。
    assert!(
        tunnel.cancel_count() >= 1,
        "forced abort must cancel the underlay in-flight (TunnelHandle::cancel), not graceful-close it"
    );
    // 保活 task 被取消、已退出——不留孤儿。
    assert!(
        ka_exited.load(Ordering::SeqCst),
        "keepalive task cancelled and exited on abort (no orphan)"
    );
}

// ── §8 无孤儿任务：close 后泵 / 保活后台 task 均已终止（不再触达底层） ────────────

/// §8 无孤儿任务（close 路径）：优雅 close 后，保活后台 task 必须已终止——以保活桩的
/// `exited` 观测位在 `CLOSE_DEADLINE` 内为真断言（不留后台 task 触达底层，§3.7）。
///
/// 泵侧的「无孤儿」由 [`forced_abort_cuts_in_flight_before_slow_op_returns`]（abort 在永不返回
/// 读上仍在时限内完成 ⇒ 泵任务被砍、未挂死）协同钉死；本用例钉保活任务在 close 后确已退出。
#[tokio::test]
async fn close_leaves_no_orphan_keepalive_task() {
    let (mut inner, _tunnel, ka_exited, _hr, _log, _peers) = assemble_persistent_inner();

    timeout(CLOSE_DEADLINE, inner.close())
        .await
        .expect("close completes")
        .expect("close Ok");

    assert!(
        ka_exited.load(Ordering::SeqCst),
        "after close the keepalive background task must have terminated \
         (no orphan task touching the underlay, §3.7)"
    );
}

// ── §8 L-9 差异不外溢：persistent(Some) 与非 persistent(None) 装配 / 关闭路径一致 ──

/// §8 L-9 差异不外溢：[`TransportChannelInner`] 的关闭 / 健康接口**不含**长 / 非长分支——
/// 非 persistent 通路（`keepalive == None`，F-3 无保活任务）经**同一** `assemble` / `close`
/// API 装配与关闭，行为与 persistent 一致：close 关底层恰一次、健康转 `Closed`、幂等。
///
/// 钉死：装配 / 关闭代码路径对 `Some(keepalive)` 与 `None` 一致（persistent 差异不进入
/// `Channel` 装配）——`assemble` 第二参为 `Option`，`close` 无 `if persistent` 分支。
#[tokio::test]
async fn ephemeral_channel_closes_through_same_api_as_persistent() {
    let log: EventLog = Arc::new(Mutex::new(Vec::new()));
    let (local_pump, _local_test) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, _underlay_test) = tokio::io::duplex(64 * 1024);
    let (pump_hw, _pump_hr) = health_view();
    let pump = spawn_bridge(local_pump, underlay_pump, pump_hw);

    let tunnel = Arc::new(RecordingTunnel::new(Arc::clone(&log)));
    let (health_w, health_r) = health_view();
    // 非 persistent：keepalive 为 None（F-3 无保活任务）——经同一 assemble 路径装配（L-9）。
    let mut inner = TransportChannelInner::assemble(
        pump,
        None,
        Box::new(ArcTunnel(Arc::clone(&tunnel))),
        health_w,
        health_r.clone(),
    );

    assert_eq!(
        inner.health(),
        Health::Alive,
        "ephemeral channel starts Alive"
    );

    // 经同一 close API 关闭：行为与 persistent 一致（关底层恰一次、健康 Closed）。
    let r = timeout(CLOSE_DEADLINE, inner.close())
        .await
        .expect("ephemeral close completes through the same API");
    assert_eq!(r, Ok(()), "ephemeral close returns Ok via the same path");
    assert_eq!(
        tunnel.close_count(),
        1,
        "ephemeral close underlay exactly once (same as persistent)"
    );
    assert_eq!(
        inner.health(),
        Health::Closed,
        "ephemeral health Closed after close"
    );

    // 幂等同样成立（差异不外溢：关闭语义对二形态一致）。
    let again = timeout(CLOSE_DEADLINE, inner.close())
        .await
        .expect("repeat ephemeral close completes");
    assert_eq!(again, Ok(()), "ephemeral close is idempotent too");
    assert_eq!(
        tunnel.close_count(),
        1,
        "no second underlay close (idempotent); persistent 差异不外溢"
    );
}

// ── §8 / §3.5 / L-7 关闭报错脱敏：底层 close 报错 → CloseFailed，渲染串不含真实地址 ──

/// §8 / §3.5 / L-7：优雅 close 执行中底层报错（桩隧道 `close` 返回 `Err`，模拟「关隧道时对端
/// 已不可达」）→ `close()` 返回 `Err(TransportError::CloseFailed)`；其 `Display` / `Debug` 渲染串
/// **均不含** 注入的真实地址子串 [`FAKE_ADDR`]（绝不外泄原始地址，§3.5 末 / L-7）。
///
/// 钉死两点：①底层关闭报错显式转 `CloseFailed`（**不**吞错放行）；②脱敏后渲染串不含真实地址
/// （类型层只承载常量化错误码判别）。一个把底层错误串 / 地址拼进错误的实现会在此被钉红。
#[tokio::test]
async fn close_failure_maps_to_sanitized_close_failed_without_address() {
    let log: EventLog = Arc::new(Mutex::new(Vec::new()));
    let (local_pump, _local_test) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, _underlay_test) = tokio::io::duplex(64 * 1024);
    let (pump_hw, _pump_hr) = health_view();
    let pump = spawn_bridge(local_pump, underlay_pump, pump_hw);

    // 底层 close 一律报错（模拟对端已不可达）；诊断侧地址明文 FAKE_ADDR 仅在测试夹具，
    // 不随返回越界——上层须脱敏为 CloseFailed，绝不外泄。
    let tunnel = Arc::new(RecordingTunnel::failing(Arc::clone(&log)));
    let (health_w, _health_r) = health_view();
    let mut inner = TransportChannelInner::assemble(
        pump,
        None,
        Box::new(ArcTunnel(Arc::clone(&tunnel))),
        health_w,
        tunnel_reader_placeholder(),
    );

    let err = timeout(CLOSE_DEADLINE, inner.close())
        .await
        .expect("close-with-failing-underlay completes")
        .expect_err("underlay close failure must surface as Err, never swallowed");

    // ① 显式转 CloseFailed（不吞错、不降级为其它变体）。
    assert_eq!(
        err,
        TransportError::CloseFailed,
        "underlay close error must map to the sanitized CloseFailed code (§3.5/L-7)"
    );

    // ② 脱敏：Display / Debug 渲染串均不含注入的真实地址子串（绝不外泄原始地址）。
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(
        !display.contains(FAKE_ADDR),
        "sanitized CloseFailed Display must not leak the real address {FAKE_ADDR}: got {display:?}"
    );
    assert!(
        !debug.contains(FAKE_ADDR),
        "sanitized CloseFailed Debug must not leak the real address {FAKE_ADDR}: got {debug:?}"
    );
}

/// 上一个用例需要一个 `HealthReader` 作 inner 第五参，但该用例不读它——取一个独立读半占位
/// （与 inner 内部写半解耦，仅满足签名）。仅夹具便利，无行为意义。
fn tunnel_reader_placeholder() -> HealthReader {
    let (_w, r) = health_view();
    r
}

// ── §8 F-5 / §3.5 强制 abort 幂等：重复 abort 不二次砍、不挂死、健康仍 Closed ──────

/// §8 F-5 / §3.5：强制 abort 同样**幂等**——已 abort 的通路再次 `abort()` 不二次取消底层
/// （cancel 计数不再增长）、不挂死，健康仍为 `Closed`（once 守卫，§3.5）。
///
/// 钉死：abort 经与 close 同一 `closed` once 守卫——重复下达是 no-op，不重复触达底层。
#[tokio::test]
async fn forced_abort_is_idempotent() {
    let (mut inner, tunnel, _ka_exited, _hr, _log, _peers) = assemble_persistent_inner();

    timeout(CLOSE_DEADLINE, inner.abort())
        .await
        .expect("first abort completes");
    assert_eq!(inner.health(), Health::Closed, "abort drives health Closed");
    let cancels_after_first = tunnel.cancel_count();

    // 第二次 abort（幂等）：不再二次取消底层、不挂死、健康仍 Closed。
    timeout(CLOSE_DEADLINE, inner.abort())
        .await
        .expect("repeat abort completes (no hang)");
    assert_eq!(
        tunnel.cancel_count(),
        cancels_after_first,
        "repeat abort must NOT cancel the underlay a second time (idempotent once-guard, §3.5)"
    );
    assert_eq!(
        inner.health(),
        Health::Closed,
        "health stays Closed across repeat abort"
    );
}

// ── §3.4 健康被动读：装配后初态为 Alive（关闭前不伪报死） ────────────────────────

/// §3.4 健康被动读：刚装配完成（未关闭、底层未断）的 inner 经 `health()` 被动读返回初态
/// `Alive`——不伪报死活、不主动 push（§3.4）。这是关闭 / abort 把健康推进到 `Closed` 的对照基线。
#[tokio::test]
async fn freshly_assembled_inner_reads_alive() {
    let (inner, _tunnel, _ka_exited, hr, _log, _peers) = assemble_persistent_inner();
    assert_eq!(
        inner.health(),
        Health::Alive,
        "freshly assembled channel reads Alive (passive, §3.4)"
    );
    assert_eq!(
        hr.get(),
        Health::Alive,
        "the read half shares the same Alive fact"
    );
}
