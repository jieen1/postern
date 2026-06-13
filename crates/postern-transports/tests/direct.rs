//! `direct` 形态 `Transport` 实现（direct-transport 单元）行为测试（RED）。
//!
//! 被测对象：`postern_transports::direct::{DirectTransport, KIND, PERSISTENT}` 与机制层
//! `postern_transports::direct::open::{connect_and_assemble, Dialer, Dialed}`——最薄的一层
//! `Transport`（§3.2 direct / §5.1）：按 `ResolvedTarget` 解出的真实地址**直发 TCP**，本地
//! socket 端点即该连接的本地一侧，把端点 + 健康 + 关闭组装成 `Channel`（F-1）。
//!
//! **机制层 / 机密薄入口分离（SEC_CONSTRUCTION_SITES 死线 / 写入 type_level_notes）**：
//! `Transport::open(target: ResolvedTarget, cred: ResourceCredential)` 按值消费机密类型，但
//! transports **不能构造** `ResolvedTarget`/`ResourceCredential`（契约 `SEC_CONSTRUCTION_SITES`
//! 仅 secrets）——本单元**绝不**写 `ResolvedTarget`/`ResourceCredential`/`ConnOrigin` 字面、
//! **绝不** `Name { .. }` / `Name::new`。故 `open` 入口「消费真实机密」的完整路径**不在本单元
//! 驱动**，如实由集成层（daemon 注入真实机密）覆盖；本单元只测可单测的部分：
//! - `kind()` 取值（恒 `"direct"`）、`persistent()` 取值（编译期固定常量 `false`、多次调用恒等）；
//! - 机制层 [`connect_and_assemble`]：经 loopback `TcpListener` 作**可达远端**、经**内部入口**
//!   （而非真实机密）驱动 dial→组装，产出 `Ok(Channel)` 且其 handle 装着可读写的本地端点
//!   （对齐 04 §4.1 Trace ① [7b] 本地 socket 通路，F-1）；
//! - 失败语义：不可达远端（loopback 上无监听者）→ `Err(TransportError)`，**无任何** `Ok` 路径
//!   （L-1，对齐 04 §4.2 D）；首次连接即失败桩 → dial **恰 1 次**、无 sleep / 退避（L-2）；
//!   半建底层后组装失败 → **先关半建底层、再返 `Err`**，不泄漏半成品、不返回伪健康 `Channel`
//!   （§3.2 关键取舍 / L-1）。
//!
//! 覆盖 §8 条目（逐条加注释）：F-1（open 建立 / 机制层 dial→组装 Channel）、F-6（persistent
//! 常量 / kind 恒值）、L-1（open 失败必 Err / 半建先拆）、L-2（不重试不退避 / dial 恰一次）、
//! L-5（open 签名为值 move 传入 target/cred——type_level，机密路径集成层覆盖）。
//!
//! 本单元不构造机密类型、不嵌裸数据库写标记、不依赖兄弟单元；端点 / 远端一律用 loopback
//! `TcpListener`/`TcpStream`（§9）。异步用 `#[tokio::test]`。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use postern_core::error::TransportError;
use postern_core::plugin::{Channel, Transport};

use postern_transports::chan::TunnelHandle;
use postern_transports::direct::open::{connect_and_assemble, Dialed, Dialer};
use postern_transports::direct::{DirectTransport, KIND, PERSISTENT};
use postern_transports::error::InnerFault;

/// 机制层 dial / 组装收口的统一超时上界（CI 容差）：建立 / 失败必须在此时限内有结果
/// （L-2 无 sleep / 退避——一个内置退避等待的实现会在此挂死超时 = 红）。
const OPEN_DEADLINE: Duration = Duration::from_secs(2);

/// 注入的「真实地址」明文子串（仅测试夹具内，不构造机密类型）：用于 L-1/L-7 钉死脱敏后的
/// `TransportError` 渲染串不含此子串。它是普通字符串，不是任何机密 / `ResolvedTarget`。
const FAKE_ADDR: &str = "10.0.3.17";

// ── 桩底层隧道：记录 close / cancel 调用次数（§3.5 / L-1 半建先拆观察） ──────────────

/// 记录调用的桩底层隧道（§3.5）：`close`/`cancel` 各记一次调用。用于钉死「半建底层后组装
/// 失败 → 先关半建底层（`close` 被调用）再返 `Err`」（L-1）与「成功组装出的 `Channel` 关闭时
/// 关底层」。本桩**不接触机密**——不持 `ResolvedTarget`/`ResourceCredential`。
struct RecordingTunnel {
    close_calls: Arc<AtomicUsize>,
    cancel_calls: Arc<AtomicUsize>,
}

impl RecordingTunnel {
    fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let close_calls = Arc::new(AtomicUsize::new(0));
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let me = Self {
            close_calls: Arc::clone(&close_calls),
            cancel_calls: Arc::clone(&cancel_calls),
        };
        (me, close_calls, cancel_calls)
    }
}

impl TunnelHandle for RecordingTunnel {
    fn close(&self) -> Result<(), ()> {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn cancel(&self) {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
    }
}

// ── 桩拨号器：经 loopback / 注入失败驱动机制层，记录 dial 尝试次数（§9 / L-2） ──────

/// 连到一个真实 loopback `TcpListener` 的**可达**桩拨号器（F-1）：`dial` 用 `TcpStream::connect`
/// 真发起一次 TCP 连接到 `addr`，把连接的本地半作底层隧道、另起一对内存管道作本地端点（桥接
/// 泵的两端皆为可读写字节双工）。记录 dial 尝试次数（L-2 钉「恰一次」）。
///
/// **本地端点对端半经 `local_peer` 槽交回测试侧持活**（F-1 核心）：本地端点（适配器侧 socket）
/// 即该通路的「本地 socket 一侧」——测试经此对端半在**组装出的 `Channel`** 上读写应用字节，验证
/// 字节确经桥接泵在本地端点 ⇆ 底层 TCP 之间双向流转（对齐 04 §4.1 Trace ① [7b]『该 Channel 可被
/// 上层当作本地 socket 使用』）。一个丢弃端点、不把端点接进桥接泵、返回伪健康 `Channel` 的实现，
/// 此对端半上的读写将永远无字节流转 → 钉红。
///
/// **不接触机密**：`dial` 只收 [`SocketAddr`]（普通地址值），不收机密类型。
struct LoopbackDialer {
    attempts: Arc<AtomicUsize>,
    tunnel_close: Arc<AtomicUsize>,
    tunnel_cancel: Arc<AtomicUsize>,
    /// 本地端点的**对端半**回交槽：`dial` 成功后把对端双工半放入，测试侧取出后即可在组装出的
    /// `Channel` 上经本地 socket 读写应用字节（F-1 本地 socket 通路）。
    local_peer: Arc<Mutex<Option<tokio::io::DuplexStream>>>,
}

impl LoopbackDialer {
    fn new() -> (
        Self,
        Arc<AtomicUsize>,
        Arc<Mutex<Option<tokio::io::DuplexStream>>>,
    ) {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (_t, close_calls, cancel_calls) = RecordingTunnel::new();
        let local_peer = Arc::new(Mutex::new(None));
        let me = Self {
            attempts: Arc::clone(&attempts),
            tunnel_close: close_calls,
            tunnel_cancel: cancel_calls,
            local_peer: Arc::clone(&local_peer),
        };
        (me, attempts, local_peer)
    }
}

#[async_trait::async_trait]
impl Dialer for LoopbackDialer {
    // 本地端点：内存双工管道的泵侧（适配器经 Channel 读写应用字节）。
    type Endpoint = tokio::io::DuplexStream;
    // 底层隧道：到 loopback 远端的真实 TCP 连接（direct = 连接本身）。
    type Underlay = TcpStream;

    async fn dial(
        &self,
        addr: SocketAddr,
    ) -> Result<Dialed<Self::Endpoint, Self::Underlay>, InnerFault> {
        // 恰一次连接尝试（L-2 钉死）——无重试、无退避循环。
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let underlay = TcpStream::connect(addr)
            .await
            .map_err(|_| InnerFault::connect(format!("direct dial failed to {addr}")))?;
        // 本地端点：内存双工管道。泵侧（`endpoint`）交给机制层接进桥接泵；对端半（本地 socket
        // 应用侧）经 `local_peer` 槽回交测试侧持活——测试经它在组装出的 Channel 上读写应用字节，
        // 验证字节确经桥接泵在本地端点 ⇆ 底层 TCP 之间双向流转（F-1 本地 socket 通路）。
        let (endpoint, local_peer) = tokio::io::duplex(64 * 1024);
        *self.local_peer.lock().expect("local_peer slot mutex") = Some(local_peer);
        let tunnel = RecordingTunnel {
            close_calls: Arc::clone(&self.tunnel_close),
            cancel_calls: Arc::clone(&self.tunnel_cancel),
        };
        Ok(Dialed {
            endpoint,
            underlay,
            tunnel: Box::new(tunnel),
            assemble_fault: None,
        })
    }
}

/// **首次连接即失败**的桩拨号器（L-1 / L-2）：`dial` 不真连，直接记一次尝试并返回连接类
/// [`InnerFault`]——模拟「不可达远端 / loopback 上无监听者 / 连 closed 端口」。记录尝试次数以
/// 钉「恰一次、无重试」（L-2）。**不接触机密**。
struct FailingDialer {
    attempts: Arc<AtomicUsize>,
}

impl FailingDialer {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let attempts = Arc::new(AtomicUsize::new(0));
        (
            Self {
                attempts: Arc::clone(&attempts),
            },
            attempts,
        )
    }
}

#[async_trait::async_trait]
impl Dialer for FailingDialer {
    type Endpoint = tokio::io::DuplexStream;
    type Underlay = tokio::io::DuplexStream;

    async fn dial(
        &self,
        addr: SocketAddr,
    ) -> Result<Dialed<Self::Endpoint, Self::Underlay>, InnerFault> {
        // 恰一次尝试即失败（无重试 / 无退避 / 无 sleep，L-2）。诊断载体携带 FAKE_ADDR 明文仅在
        // crate 内部，经 sanitize 脱敏后绝不越界（L-1/L-7）。
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let _ = addr;
        Err(InnerFault::connect(format!(
            "connection refused to {FAKE_ADDR}"
        )))
    }
}

/// **半建底层后组装失败**的桩拨号器（§3.2 关键取舍 / L-1）：`dial` 成功半建底层（产出端点 +
/// 底层隧道 + 记录调用的关闭句柄），但在 [`Dialed::assemble_fault`] 注入一个组装失败——驱动
/// 机制层走「先关半建底层、再返 `Err`」路径。底层隧道的 `close` 计数留在测试侧断言「半建确被
/// 关」。**不接触机密**。
struct HalfBuiltThenFailDialer {
    tunnel_close: Arc<AtomicUsize>,
    tunnel_cancel: Arc<AtomicUsize>,
}

impl HalfBuiltThenFailDialer {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let (_t, close_calls, cancel_calls) = RecordingTunnel::new();
        let me = Self {
            tunnel_close: Arc::clone(&close_calls),
            tunnel_cancel: cancel_calls,
        };
        (me, close_calls)
    }
}

#[async_trait::async_trait]
impl Dialer for HalfBuiltThenFailDialer {
    type Endpoint = tokio::io::DuplexStream;
    type Underlay = tokio::io::DuplexStream;

    async fn dial(
        &self,
        addr: SocketAddr,
    ) -> Result<Dialed<Self::Endpoint, Self::Underlay>, InnerFault> {
        let _ = addr;
        let (endpoint, _ep_peer) = tokio::io::duplex(64 * 1024);
        let (underlay, _ul_peer) = tokio::io::duplex(64 * 1024);
        let tunnel = RecordingTunnel {
            close_calls: Arc::clone(&self.tunnel_close),
            cancel_calls: Arc::clone(&self.tunnel_cancel),
        };
        // 半建成功，但注入「后续组装失败」——机制层须先关此半建底层再返 Err（§3.2 / L-1）。
        Ok(Dialed {
            endpoint,
            underlay,
            tunnel: Box::new(tunnel),
            assemble_fault: Some(InnerFault::handshake(format!(
                "post-dial assembly failed at {FAKE_ADDR}"
            ))),
        })
    }
}

/// 起一个 loopback `TcpListener` 作可达远端，返回其本地地址 + 持住 listener（accept 在后台
/// 完成握手，使 dial 的 `TcpStream::connect` 成功）。listener 须随测试存活到 dial 之后。
async fn spawn_loopback_remote() -> (SocketAddr, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback remote");
    let addr = listener.local_addr().expect("loopback local_addr");
    (addr, listener)
}

// ── §8 F-6 / F-8 kind() 恒返回 "direct"（编译期常量，不读配置 / 通路状态） ──────────────

/// §8 F-6 / F-8：`DirectTransport::kind()` 恒返回 `"direct"`（传输注册表选型键，§5.1）——对同
/// 一实例多次调用返回值恒等，且等于导出的编译期常量 [`KIND`]。
///
/// 钉死：①取值精确为 `"direct"`（非任何其它形态键）；②多次调用恒等（不依赖运行时状态）；
/// ③与导出常量一致（F-6 构造签名审查：取自固定常量、不读配置）。
#[test]
fn kind_is_constant_direct() {
    let t = DirectTransport::new();
    assert_eq!(
        t.kind(),
        "direct",
        "direct transport kind() must be exactly \"direct\" (§5.1)"
    );
    assert_eq!(
        t.kind(),
        KIND,
        "kind() must return the exported compile-time constant KIND (F-6)"
    );
    // 多次调用返回值恒等（不依赖运行时状态，F-6）。
    assert_eq!(
        t.kind(),
        t.kind(),
        "kind() must be identical across repeated calls on the same instance"
    );
}

// ── §8 F-6 persistent() 常量：恒 false、多次调用恒等、等于编译期常量 ─────────────────

/// §8 F-6：`DirectTransport::persistent()` 返回**编译期固定常量布尔** `false`（direct 非隧道
/// 直连、用毕即释放，本刀定为非长连接型，§3.2）——对同一实例多次调用返回值**恒等**、且等于
/// 导出常量 [`PERSISTENT`]（不读配置 / 通路状态，F-6 构造签名审查点）。
///
/// 钉死：①取值精确为 `false`（连接管理层据此**不**池化、用毕即销，F-3）；②同一实例多次调用
/// 恒等（不依赖运行时状态）；③与导出的编译期常量 `PERSISTENT` 一致。一个读配置 / 读通路状态
/// 决定 persistent 的实现会破坏「多次调用恒等 / 等于常量」→ 钉红。
#[test]
fn persistent_is_constant_false() {
    let t = DirectTransport::new();
    assert!(
        !t.persistent(),
        "direct is non-persistent (direct 非隧道直连用毕即释放, §3.2/F-6)"
    );
    assert_eq!(
        t.persistent(),
        PERSISTENT,
        "persistent() must return the compile-time constant PERSISTENT (F-6)"
    );
    const {
        assert!(
            !PERSISTENT,
            "exported PERSISTENT constant for direct must be false"
        );
    }
    // 同一实例多次调用返回值恒等（编译期常量、不读运行时状态，F-6）。
    assert_eq!(
        t.persistent(),
        t.persistent(),
        "persistent() must be identical across repeated calls (F-6)"
    );

    // 另起一个实例同样恒等——取值是该实现固定的常量，不随实例变化（F-6 构造签名审查）。
    let t2 = DirectTransport;
    assert_eq!(
        t2.persistent(),
        t.persistent(),
        "persistent() must be identical across instances (fixed constant)"
    );
}

// ── §8 F-1 open 建立（机制层）：可达远端 → Ok(Channel)，handle 装着可用本地端点 ──────────

/// §8 F-1：经机制层 [`connect_and_assemble`] 用 loopback `TcpListener` 作**可达远端**驱动
/// dial→组装 → 返回 `Ok(Channel)`，且该 `Channel` **可被上层当作本地 socket 使用**——经本地端点
/// 写入的应用字节确经桥接泵流向底层远端、远端写回的字节确经桥接泵流回本地端点（双向真字节流转，
/// 对齐 04 §4.1 Trace ① [7b]『该 Channel 可被上层当作本地 socket 使用』）。
///
/// 经**内部入口**（机制层 `connect_and_assemble`，按真实 `SocketAddr` 驱动）而非真实机密驱动
/// ——本单元不构造机密。`persistent` 取 [`PERSISTENT`]（direct 常量）传入。
///
/// 钉死本地 socket 通路（F-1 承重）：①可达远端 → `Ok(Channel)`（非 `Err`）；②返回类型即 **core
/// 的** `Channel`（与 `Adapter::execute(ch:&mut Channel,…)` 共享，不重定义类型，F-7）；③**前向**：
/// 经本地端点（本地 socket 应用侧）写入的字节，在 `OPEN_DEADLINE` 内被底层远端原样收到——证明
/// 本地端点确被组装进通路并接进桥接泵搬向底层；④**反向**：底层远端写回的字节，在 `OPEN_DEADLINE`
/// 内经本地端点原样读回——证明反向桥接同样落地。一个丢弃本地端点、不把端点接进桥接泵、返回伪
/// 健康 `Channel` 的破坏 F-1 的实现，③/④ 的读写将无字节流转 / 挂死超时 → 钉红（弱断言
/// `size_of_val>0` 对任意非空 inner 恒真、放不过此类缺陷，已弃用）。
#[tokio::test]
async fn open_mechanism_reachable_remote_yields_ok_channel_usable_as_local_socket() {
    // 可达远端 + 持住 listener 以便 accept 出底层连接的远端一侧（用于双向字节流转观察）。
    let (addr, listener) = spawn_loopback_remote().await;
    let (dialer, attempts, local_peer_slot) = LoopbackDialer::new();

    // 后台 accept：dial 真发起 TCP 后远端 accept 到底层连接的远端一侧（即桥接泵搬向 / 搬自的底层）。
    let accept_task = tokio::spawn(async move { listener.accept().await.map(|(s, _)| s) });

    let ch: Channel = timeout(
        OPEN_DEADLINE,
        connect_and_assemble(&dialer, addr, PERSISTENT),
    )
    .await
    .expect("connect_and_assemble must finish within deadline (no backoff hang, L-2)")
    .expect("reachable loopback remote must yield Ok(Channel) (F-1)");

    // dial 恰一次（成功路径也不重复尝试，L-2）。
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "successful open must dial exactly once (no retry, L-2)"
    );

    // 返回的就是 core 的 Channel（与 Adapter::execute 共享同一类型，不重定义，F-7）。
    // 持住 Channel 到字节流转断言之后——drop 即停桥接泵、断底层，故必须存活到读写验证完成。
    let _ch_alive: Channel = ch;

    // 取回底层远端一侧（桥接泵搬向 / 搬自的底层）。
    let mut remote = timeout(OPEN_DEADLINE, accept_task)
        .await
        .expect("loopback remote must accept the dialed TCP connection within deadline (F-1: dials real TCP)")
        .expect("accept task join")
        .expect("loopback remote accept Ok");

    // 取回本地端点的对端半（本地 socket 应用侧）——上层即经此在 Channel 上读写应用字节（F-1）。
    let mut local = local_peer_slot
        .lock()
        .expect("local_peer slot mutex")
        .take()
        .expect(
            "dial must hand back the local-endpoint peer half (the local socket app side, F-1)",
        );

    // ③ 前向（本地 socket → 底层远端）：经本地端点写入的字节确经桥接泵原样到达底层远端。
    //    一个丢弃本地端点 / 不把端点接进桥接泵的伪健康 Channel 会让此读挂死超时 → 钉红。
    let forward_msg = b"direct-fwd-7b";
    local
        .write_all(forward_msg)
        .await
        .expect("write app bytes into the local socket endpoint");
    local.flush().await.expect("flush local endpoint");
    let mut got_fwd = vec![0u8; forward_msg.len()];
    timeout(OPEN_DEADLINE, remote.read_exact(&mut got_fwd))
        .await
        .expect("forward bytes must reach the underlay remote within deadline (Channel usable as local socket, F-1)")
        .expect("read forwarded bytes at the underlay remote");
    assert_eq!(
        &got_fwd, forward_msg,
        "bytes written at the local socket endpoint must transit the bridge pump to the underlay remote unchanged (F-1)"
    );

    // ④ 反向（底层远端 → 本地 socket）：底层远端写回的字节确经桥接泵原样流回本地端点。
    let reverse_msg = b"direct-rev-7b";
    remote
        .write_all(reverse_msg)
        .await
        .expect("write bytes from the underlay remote");
    remote.flush().await.expect("flush underlay remote");
    let mut got_rev = vec![0u8; reverse_msg.len()];
    timeout(OPEN_DEADLINE, local.read_exact(&mut got_rev))
        .await
        .expect("reverse bytes must flow back to the local socket endpoint within deadline (Channel usable as local socket, F-1)")
        .expect("read reverse bytes at the local endpoint");
    assert_eq!(
        &got_rev, reverse_msg,
        "bytes written by the underlay remote must transit the bridge pump back to the local socket endpoint unchanged (F-1)"
    );
}

// ── §8 F-1 / F-8 DirectTransport 实现 core 的 Transport trait（共享类型，dyn 可用） ───────

/// §8 F-1 / F-8：`DirectTransport` 确实 **impl `core::plugin::Transport`**——可作
/// `&dyn Transport` 经 trait 对象调 `kind()`/`persistent()`（传输注册表按 trait 对象选型，
/// §5.1）。钉死本 crate **实现** core 定义的 trait（而非自定义同名 trait）。
///
/// `open` 经 trait 对象的调用因消费机密类型**不在此驱动**（机密薄入口集成层覆盖，见
/// type_level_notes）——本用例只钉 `kind`/`persistent` 经 trait 对象可达且取值正确。
#[test]
fn direct_transport_is_a_core_transport_trait_object() {
    let t = DirectTransport::new();
    let dyn_t: &dyn Transport = &t;
    assert_eq!(
        dyn_t.kind(),
        "direct",
        "DirectTransport must impl core::Transport with kind()==\"direct\""
    );
    assert!(
        !dyn_t.persistent(),
        "DirectTransport must impl core::Transport with persistent()==false"
    );
}

// ── §8 L-1 open 失败必 Err：不可达远端 → Err(ConnectFailed)，无任何 Ok(Channel) 路径 ─────

/// §8 L-1：给定不可达远端（首次连接即失败桩 → loopback 上无监听者 / 连 closed 端口语义）驱动
/// 机制层 → 返回 `Err(TransportError)`，**无任何** `Ok(Channel)` 路径（连接层据此 deny，绝不
/// 返回伪健康通路；对齐 04 §4.2 D 连接不可建→deny）。
///
/// 钉死：①失败必 `Err`（用 `expect_err`——一个返回伪健康 `Ok(Channel)` 的实现在此被钉红）；
/// ②错误恰为脱敏后的 `TransportError::ConnectFailed`（连接类失败映射，不降级为其它变体）；
/// ③脱敏：`Display`/`Debug` 渲染串均**不含**注入的真实地址子串 [`FAKE_ADDR`]（L-1/L-7 脱敏，
/// 绝不外泄原始地址）。
#[tokio::test]
async fn open_mechanism_unreachable_remote_returns_sanitized_err_no_pseudo_healthy_channel() {
    let (dialer, _attempts) = FailingDialer::new();
    let addr: SocketAddr = "127.0.0.1:9"
        .parse()
        .expect("discard-port loopback addr literal");

    let result = timeout(
        OPEN_DEADLINE,
        connect_and_assemble(&dialer, addr, PERSISTENT),
    )
    .await
    .expect("connect_and_assemble must finish within deadline (no retry/backoff, L-2)");
    // `Channel` 不实现 `Debug`（core 刻意不给——机密薄入口），故经 match 取 Err（不用 expect_err）。
    let err = match result {
        Ok(_ch) => panic!(
            "unreachable remote must yield Err — never a pseudo-healthy Ok(Channel) (L-1, 公理二)"
        ),
        Err(e) => e,
    };

    // 连接类失败恰映射为脱敏 ConnectFailed（不降级 / 不混淆其它变体）。
    assert_eq!(
        err,
        TransportError::ConnectFailed,
        "unreachable dial must map to the sanitized ConnectFailed code (L-1, 对齐 04 §4.2 D)"
    );

    // 脱敏：渲染串不含注入的真实地址（绝不外泄原始地址，L-1/L-7）。
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(
        !display.contains(FAKE_ADDR),
        "sanitized ConnectFailed Display must not leak real address {FAKE_ADDR}: got {display:?}"
    );
    assert!(
        !debug.contains(FAKE_ADDR),
        "sanitized ConnectFailed Debug must not leak real address {FAKE_ADDR}: got {debug:?}"
    );
}

// ── §8 L-2 不重试不退避：首次连接即失败桩 → dial 恰 1 次、单个 Err、无 sleep / 退避等待 ──

/// §8 L-2：用「首次连接即失败」桩调一次机制层 → 下层连接尝试次数**恰为 1**（无重试），返回
/// **单个** `Err(TransportError)` 且无 sleep / 退避等待（构造签名审查：direct 单元内无退避器 /
/// 重试计数器 / 退避时长常量）。
///
/// 钉死：①dial 尝试次数恰为 1（一个失败后重试 N 次的实现会让计数 >1 → 钉红）；②机制层在
/// `OPEN_DEADLINE` 内返回（一个失败后 `sleep` 退避再重试的实现会挂死超时 → 钉红）；③返回是
/// 单个 `Err`（fail-closed，不降级、不静默重试到其它通路，L-2/L-3）。
#[tokio::test]
async fn open_mechanism_first_failure_dials_exactly_once_no_retry_no_backoff() {
    let (dialer, attempts) = FailingDialer::new();
    let addr: SocketAddr = "127.0.0.1:9"
        .parse()
        .expect("discard-port loopback addr literal");

    let result = timeout(
        OPEN_DEADLINE,
        connect_and_assemble(&dialer, addr, PERSISTENT),
    )
    .await
    .expect("first-failure dial must return promptly — no sleep/backoff wait (L-2 构造签名审查)");

    // 下层连接尝试次数恰为 1：无重试（L-2）。一个重试的实现让计数 >1 → 钉红。
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "first-failure dial must attempt the underlay connection exactly once (no retry, L-2)"
    );
    // 返回单个 Err（fail-closed，不降级 / 不静默重试到其它通路，L-2/L-3）。
    assert!(
        matches!(result, Err(TransportError::ConnectFailed)),
        "first-failure dial must return a single Err(ConnectFailed), not retry or downgrade (L-2)"
    );
}

// ── §8 L-1 半建先拆：半建底层后组装失败 → 先关半建底层、再返 Err，不泄漏半成品 ───────────

/// §8 L-1 / §3.2 关键取舍：模拟「连接已半建但后续组装失败」（dial 成功半建底层、注入组装失败）
/// → **半建底层被关闭**（桩隧道记录 `close` 被调用）、机制层返回 `Err`，**不**泄漏挂着隧道的
/// 半成品、**不**返回伪健康 `Channel`。
///
/// 钉死核心不变量（§3.2）：①半建底层的 `close` 被调用**恰 1 次**（先拆半建——一个直接返
/// `Err` 而不关半建底层的实现会让 close 计数为 0 → 钉红，等于静默资源泄漏 + 伪健康温床）；
/// ②机制层返回 `Err`（用 `expect_err`——绝不返回伪健康 `Ok(Channel)`，公理二）；③脱敏：渲染串
/// 不含注入的真实地址子串（L-7）。
#[tokio::test]
async fn open_mechanism_half_built_then_assembly_fails_closes_underlay_before_err() {
    let (dialer, tunnel_close) = HalfBuiltThenFailDialer::new();
    let addr: SocketAddr = "127.0.0.1:1".parse().expect("loopback addr literal");

    let result = timeout(
        OPEN_DEADLINE,
        connect_and_assemble(&dialer, addr, PERSISTENT),
    )
    .await
    .expect("half-built-then-fail must finish within deadline");
    // `Channel` 不实现 `Debug`，经 match 取 Err（不用 expect_err）。
    let err = match result {
        Ok(_ch) => panic!("assembly failure after a half-built underlay must yield Err — never a pseudo-healthy Channel (L-1, 公理二)"),
        Err(e) => e,
    };

    // 半建底层被先关（先拆半建，不泄漏挂着隧道的半成品，§3.2 关键取舍）。
    assert_eq!(
        tunnel_close.load(Ordering::SeqCst),
        1,
        "half-built underlay must be closed exactly once before returning Err (先关半建底层, §3.2/L-1)"
    );

    // 脱敏：渲染串不含注入的真实地址（绝不外泄原始地址，L-7）。
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(
        !display.contains(FAKE_ADDR),
        "sanitized error Display must not leak real address {FAKE_ADDR}: got {display:?}"
    );
    assert!(
        !debug.contains(FAKE_ADDR),
        "sanitized error Debug must not leak real address {FAKE_ADDR}: got {debug:?}"
    );
}

// ── §8 F-1 成功路径远端确收到字节：组装出的 Channel 底层确连到 loopback 远端 ──────────────

/// §8 F-1（端到端机制）：经机制层组装出的 `Channel`，其底层确**连到** loopback 远端——远端
/// `accept` 到一条连接即证明 dial 真发起了 TCP（本地 socket 端点即该连接本地一侧，对齐
/// 04 §4.1 Trace ① [7b]）。这是「最薄一层直发 TCP」的端到端行为锚点（F-1）。
///
/// 钉死：远端在 `OPEN_DEADLINE` 内 `accept` 到一条连接（dial 真连了可达远端，非空壳 `Ok`）。
#[tokio::test]
async fn open_mechanism_dials_real_tcp_to_loopback_remote_accepts_connection() {
    let (addr, listener) = spawn_loopback_remote().await;
    let (dialer, _attempts, _local_peer) = LoopbackDialer::new();

    // 后台 accept：dial 若真发起 TCP 到 addr，远端必 accept 到一条连接。
    let accept_task = tokio::spawn(async move { listener.accept().await.map(|_| ()) });

    let ch = timeout(
        OPEN_DEADLINE,
        connect_and_assemble(&dialer, addr, PERSISTENT),
    )
    .await
    .expect("connect_and_assemble within deadline")
    .expect("reachable remote yields Ok(Channel) (F-1)");
    // 持住 Channel 到 accept 完成（避免底层连接被提前 drop）。
    let _hold = ch;

    let accepted = timeout(OPEN_DEADLINE, accept_task)
        .await
        .expect("loopback remote must accept the dialed TCP connection within deadline (F-1: direct dials real TCP)")
        .expect("accept task join");
    assert!(
        accepted.is_ok(),
        "loopback remote must accept the connection dialed by direct (F-1)"
    );
}
