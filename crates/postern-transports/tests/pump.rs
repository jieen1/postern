//! 双向桥接泵（byte-pump 单元）行为测试（RED）。
//!
//! 被测对象：`postern_transports::pump::{spawn_bridge, PumpHandle, BRIDGE_BUFFER_BYTES}`
//! ——在「本地端点」与「底层隧道」两个字节双工端点之间架的**双向字节泵**（§3.2 桥接泵
//! 数据流 / §3.6 并发模型）。语义等价 `tokio::io::copy_bidirectional`：两个方向各一条
//! `read → write` copy 回路，有界缓冲承接、写慢即背压、不无界堆积；泵**只搬字节、不解析
//! 协议**；任一方向 EOF / IO 错误推进该方向收尾，两方向都不可搬运时泵退出并把「死亡」
//! 翻进健康视图（写 [`HealthWriter::mark_dead`]）；以取消令牌 / `abort` 收口，砍在飞、
//! 不等待在途读写返回（§3.5、L-6）。
//!
//! 覆盖 §8 条目（逐条加注释）：
//! - §8 双向搬运：用两条内存双工管道作本地端点与底层隧道，向本地端点写入字节序列、从
//!   底层隧道侧读到**完全相同**的字节；反向亦然（全双工，两方向各自独立搬运）。
//! - §8 EOF→死亡（F-4 / §3.2）：关闭底层隧道（制造 EOF）→ 泵推进收尾、退出后健康视图
//!   被写入 `Dead`（读取返回 `Dead`）。
//! - §8 L-6 abort 砍在飞：用一条永不返回的慢端点（读 future 永久 `Pending`）启动泵，触发
//!   取消令牌 / abort → 泵任务在有限时间内终止，不等待慢端点的在途读返回；以注入的可控
//!   端点观察取消点确被命中。
//! - §8 有界缓冲（§3.6）：泵用固定大小缓冲承接（[`BRIDGE_BUFFER_BYTES`] 为编译期常量、
//!   有界），写慢端点不导致读侧无界堆积（结构 + 背压行为观察）。
//! - 泵只搬字节、不解析协议（§3.2 雷区）：含 SQL / HTTP 形态的字节透传后逐字节不变，泵
//!   不因「字节内容」分支。
//!
//! 本单元不构造机密类型（不写 `ResolvedTarget` / `ResourceCredential` / `ConnOrigin`
//! 字面），不嵌裸数据库写标记，不依赖兄弟单元。机制层端点一律用内存双工管道（loopback）。

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::time::timeout;

use postern_transports::health::Health;
use postern_transports::pump::{spawn_bridge, BRIDGE_BUFFER_BYTES};

/// 单方向健康视图初态读取应得的事实位（§3.4 / F-4）：泵启动前通路是「活」。
/// 用 `health_view()` 取得写半/读半；写半交给泵，读半留在测试侧被动观察死活。
use postern_transports::health::health_view;

/// 取消/退出收口的统一超时上界（CI 容差）：取消后泵任务必须在此时限内终止
/// （§3.5 / L-6 砍在飞，不等待在途）。超时即视为「没有真正砍在飞」= 红。
const CANCEL_DEADLINE: Duration = Duration::from_secs(2);

// ── §8 双向搬运：本地端点 → 底层隧道（前向）逐字节一致 ──────────────────────

/// §8 双向搬运（前向）：向本地端点写入字节序列 → 从底层隧道侧读到**完全相同**的字节。
///
/// 拓扑：`local_pipe = duplex()` → (本地测试侧, 本地泵侧)；`underlay_pipe = duplex()`
/// → (底层泵侧, 底层测试侧)。泵持有 (本地泵侧, 底层泵侧)。测试向「本地测试侧」写入，
/// 经泵搬运后从「底层测试侧」读出，断言逐字节相等（泵只搬字节、不增删、不改）。
#[tokio::test]
async fn forward_local_to_underlay_bytes_identical() {
    let (mut local_test, local_pump) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, mut underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, _health_r) = health_view();

    let _handle = spawn_bridge(local_pump, underlay_pump, health_w);

    let payload: &[u8] = b"the-quick-brown-fox-0123456789";
    local_test
        .write_all(payload)
        .await
        .expect("write to local endpoint");
    local_test.flush().await.expect("flush local endpoint");

    let mut got = vec![0u8; payload.len()];
    underlay_test
        .read_exact(&mut got)
        .await
        .expect("read bytes that the pump forwarded to the underlay");

    // 逐字节一致：泵搬运不增删不改（精确到具体字节序列）。
    assert_eq!(got.as_slice(), payload);
}

// ── §8 双向搬运：底层隧道 → 本地端点（反向）逐字节一致 ──────────────────────

/// §8 双向搬运（反向，全双工）：向底层隧道侧写入字节 → 从本地端点侧读到完全相同的字节。
///
/// 两方向各自独立搬运（全双工）：此用例只驱动反向，断言反向同样逐字节一致。
#[tokio::test]
async fn reverse_underlay_to_local_bytes_identical() {
    let (mut local_test, local_pump) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, mut underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, _health_r) = health_view();

    let _handle = spawn_bridge(local_pump, underlay_pump, health_w);

    let payload: &[u8] = b"RESPONSE-PAYLOAD-bytes-\x00\x01\x02\xff-end";
    underlay_test
        .write_all(payload)
        .await
        .expect("write to underlay side");
    underlay_test.flush().await.expect("flush underlay side");

    let mut got = vec![0u8; payload.len()];
    local_test
        .read_exact(&mut got)
        .await
        .expect("read bytes the pump forwarded back to the local endpoint");

    assert_eq!(got.as_slice(), payload);
}

// ── §8 双向搬运：两方向同时搬运互不串扰（全双工独立） ──────────────────────

/// §8 双向搬运（全双工独立性）：同一泵上前向与反向**同时**搬运，两方向字节各自落到
/// 对侧、互不串扰（前向字节只出现在底层侧、反向字节只出现在本地侧）。
#[tokio::test]
async fn full_duplex_both_directions_do_not_cross() {
    let (mut local_test, local_pump) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, mut underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, _health_r) = health_view();

    let _handle = spawn_bridge(local_pump, underlay_pump, health_w);

    let fwd: &[u8] = b"FWD-local-to-underlay";
    let rev: &[u8] = b"REV-underlay-to-local";

    local_test
        .write_all(fwd)
        .await
        .expect("write forward payload");
    underlay_test
        .write_all(rev)
        .await
        .expect("write reverse payload");
    local_test.flush().await.expect("flush local");
    underlay_test.flush().await.expect("flush underlay");

    let mut got_fwd = vec![0u8; fwd.len()];
    underlay_test
        .read_exact(&mut got_fwd)
        .await
        .expect("read forward at underlay");
    let mut got_rev = vec![0u8; rev.len()];
    local_test
        .read_exact(&mut got_rev)
        .await
        .expect("read reverse at local");

    // 前向字节恰好出现在底层侧、反向字节恰好出现在本地侧——两方向不串扰。
    assert_eq!(got_fwd.as_slice(), fwd);
    assert_eq!(got_rev.as_slice(), rev);
}

// ── §8 泵只搬字节、不解析协议（§3.2 雷区） ────────────────────────────────

/// §8 泵只搬字节、不解析协议：把「看起来像应用协议」的字节（SQL 文本 / HTTP 请求行）
/// 透传，泵不因字节内容做任何分支，搬运后逐字节不变。
///
/// 字节内容只作「透传不变」的载荷，不在断言里做协议语义判断——泵不认识它是什么。
#[tokio::test]
async fn pump_does_not_interpret_protocol_bytes_passthrough() {
    let (mut local_test, local_pump) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, mut underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, _health_r) = health_view();

    let _handle = spawn_bridge(local_pump, underlay_pump, health_w);

    // 含「像协议」的字节形态——泵对其零感知，只逐字节透传（雷区：不看字节内容分支）。
    let protocol_shaped: &[u8] = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n--mixed-binary-\x00\xfe\x07";
    local_test
        .write_all(protocol_shaped)
        .await
        .expect("write protocol-shaped bytes");
    local_test.flush().await.expect("flush");

    let mut got = vec![0u8; protocol_shaped.len()];
    underlay_test
        .read_exact(&mut got)
        .await
        .expect("read forwarded bytes");

    // 透传后逐字节不变——泵不解析、不重写、不按内容分支。
    assert_eq!(got.as_slice(), protocol_shaped);
}

// ── §8 EOF→死亡（F-4 / §3.2 泵退出与死亡互为收口） ────────────────────────

/// §8 EOF→死亡：关闭底层隧道（drop 测试侧 → 泵的底层读得 EOF、底层写得断管）→ 两方向
/// 都不可搬运 → 泵退出，并把「死亡」翻进健康视图（读半 `get()` 返回 `Dead`）。
///
/// 钉死：泵退出**必**写 `Dead`，不静默退出留伪 `Alive` 通路（§3.2 收口、F-4）。
#[tokio::test]
async fn underlay_eof_drives_pump_exit_and_marks_dead() {
    let (_local_test, local_pump) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, health_r) = health_view();

    let handle = spawn_bridge(local_pump, underlay_pump, health_w);

    // 启动即查健康：泵尚未遇断开，应仍为 Alive（初态事实位）。
    assert_eq!(health_r.get(), Health::Alive);

    // 关闭底层隧道：drop 测试侧两个半 → 泵的底层读 EOF + 底层写断管 → 两方向皆死。
    drop(underlay_test);

    // 泵在有限时间内退出（join 完成）——不无限挂着。
    let cancelled = timeout(CANCEL_DEADLINE, handle.join())
        .await
        .expect("pump must exit after the underlay reaches EOF / breaks");
    // 自然因底层断开退出（非被 abort 取消）。
    assert!(!cancelled, "pump exited on underlay EOF, not via abort");

    // 退出后健康视图被写入死亡——读取返回 Dead（绝不留伪 Alive）。
    assert_eq!(health_r.get(), Health::Dead);
}

// ── §8 EOF→死亡（前向源 EOF 亦驱动收尾） ──────────────────────────────────

/// §8 EOF→死亡（本地端点关闭）：drop 本地测试侧（前向源 EOF + 反向目的断管）→ 两方向
/// 不可搬运 → 泵退出并写 `Dead`。对齐 §3.2「两方向不可搬运即死亡」。
#[tokio::test]
async fn local_endpoint_close_drives_pump_exit_and_marks_dead() {
    let (local_test, local_pump) = tokio::io::duplex(64 * 1024);
    let (underlay_pump, _underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, health_r) = health_view();

    let handle = spawn_bridge(local_pump, underlay_pump, health_w);
    assert_eq!(health_r.get(), Health::Alive);

    drop(local_test);

    let cancelled = timeout(CANCEL_DEADLINE, handle.join())
        .await
        .expect("pump must exit after the local endpoint closes");
    assert!(
        !cancelled,
        "pump exited on local-endpoint EOF, not via abort"
    );
    assert_eq!(health_r.get(), Health::Dead);
}

// ── §8 EOF→死亡（IO 错误 / 对端 RST 路径，copy 返回 Err → Dead） ──────────

/// §8 EOF→死亡（来源②对端 RST / IO 错误，§3.2 / §3.4 / §8 fail-closed 镜头）：
/// 本地端点的读**以 IO 错误返回**（模拟对端 RST / 半截 IO 错误，使前向 `copy_buf`
/// 回路以 `Err` 完结）→ 泵据此收口为通路死亡、退出，并把 `Dead` 翻进健康视图。
///
/// 钉死 fail-closed 不变量：无论方向收尾是 `Ok`（干净 EOF）还是 `Err`（RST / IO
/// 错误），都必须收口为死亡——IO 错误即作死活信号，**绝不被静默吞掉、不留伪 `Alive`
/// 通路**（§3.2 注释 / bridge.rs:117-118）。一个吞掉 `Err` 只对 `Ok` 才 `mark_dead`
/// 的泵会留下幽灵 `Alive`，本用例将其钉红。
#[tokio::test]
async fn local_read_io_error_drives_pump_exit_and_marks_dead() {
    // 本地端点读半即时返回 IO 错误（ConnectionReset，模拟对端 RST）；写半正常吞下。
    let err_local = ErrorOnRead::new();
    // 底层用正常内存管道；保留测试侧句柄避免其 drop 抢先制造 EOF（确保死亡确由
    // 「本地读 IO 错误」驱动，而非底层断开）。
    let (underlay_pump, _underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, health_r) = health_view();

    let handle = spawn_bridge(err_local, underlay_pump, health_w);

    // 泵在有限时间内退出（前向 copy_buf 以 Err 完结 → 方向收尾 → 泵退出）。
    let cancelled = timeout(CANCEL_DEADLINE, handle.join())
        .await
        .expect("pump must exit after a read returns an IO error (RST), not hang");
    // 自然因 IO 错误收尾退出（非被外部 abort 取消）。
    assert!(!cancelled, "pump exited on read IO error, not via abort");

    // 退出后健康视图被写入死亡——读取返回 Dead（IO 错误 / RST 绝不被静默吞掉、
    // 不留伪 Alive）。吞 Err 留 Alive 的泵在此被钉红。
    assert_eq!(
        health_r.get(),
        Health::Dead,
        "a read IO error (RST / half-broken) must be reported as Dead, never swallowed into false Alive"
    );
}

// ── §8 L-6 abort 砍在飞：cancel 触点不等待在途读返回 ──────────────────────

/// §8 L-6 abort 砍在飞（cancel 路径）+ F-5 关闭收口：用一条**永不返回**的慢端点
/// （读 future 永久 `Pending`）作本地端点启动泵，触发取消令牌（`cancel()`）→ 泵任务
/// 在有限时间内终止（`join()` 完成），**不**等待慢端点在途读返回；且 cancel 是泵在
/// 取消点**自身收尾**——必须把「关闭」事实写进健康视图（`mark_closed` → 读半 `Closed`），
/// 与 `abort()`（外部硬砍、不写事实）区分（§3.5 / pump.rs:49-50）。
///
/// 以注入的可控端点观察取消点确被命中：慢端点记录「读被 poll 过」（取消前泵确在等读），
/// 而 `join()` 在 `CANCEL_DEADLINE` 内完成 = 取消真正砍在飞（没等读自然返回）。
///
/// 钉死两条相邻行为（防硬 abort 冒充 cancel）：
/// - cancel 退出**必**写 `Closed`（绝不静默退出留伪 `Alive`，fail-closed 收口）；
/// - cancel 写的是 `Closed`（按指令关闭终态），**不是** `Dead`（通路死亡）——区分 §3.5
///   cancel 路径与 EOF/RST 死亡路径。
#[tokio::test]
async fn cancel_aborts_in_flight_slow_read_without_waiting() {
    let probe = Arc::new(AtomicBool::new(false));
    let slow_local = NeverReturnsRead::new(Arc::clone(&probe));
    // 底层用正常内存管道；保留测试侧句柄以免其因 drop 提前制造 EOF。
    let (underlay_pump, _underlay_test) = tokio::io::duplex(64 * 1024);
    // 保留读半：cancel 收尾后须断言「关闭」事实落入健康视图（不丢弃读半）。
    let (health_w, health_r) = health_view();

    let handle = spawn_bridge(slow_local, underlay_pump, health_w);

    // 启动即查健康：泵尚未取消、底层未断，应仍为 Alive（初态事实位）。
    assert_eq!(health_r.get(), Health::Alive);

    // 让泵跑起来、确把慢端点的读 poll 到（取消点要命中「在途读」）。
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        probe.load(Ordering::SeqCst),
        "pump must have polled the slow endpoint's read before cancel (in-flight read exists)"
    );

    // 触发协作取消——必须砍在飞。
    handle.cancel();

    // 泵任务在有限时间内终止；慢端点的读永不返回，故若超时即「没砍在飞」= 红。
    let finished = timeout(CANCEL_DEADLINE, handle.join()).await;
    assert!(
        finished.is_ok(),
        "cancel must abort the pump in flight without awaiting the never-returning read"
    );
    // join 报告非 abort 取消（cancel 是泵自身在取消点协作收尾，非外部 abort 硬砍）。
    assert!(
        !finished.unwrap(),
        "cancel exits via the pump's own cancellation point (cooperative), not via JoinHandle::abort"
    );

    // cancel 收尾后健康视图被写入「关闭」事实——读取返回 Closed（绝不静默退出留伪
    // Alive；亦非 Dead）。一个 cancel 时硬 abort、不写 mark_closed 的泵在此被钉红。
    assert_eq!(
        health_r.get(),
        Health::Closed,
        "cancel must record the 'closed' fact (mark_closed) — never a silent exit leaving false Alive, \
         and never Dead (cancel is an instructed close, not a channel death)"
    );
}

// ── §8 L-6 abort 砍在飞：JoinHandle::abort 硬中止路径 ─────────────────────

/// §8 L-6 abort 砍在飞（abort 路径）：对挂在永不返回慢读上的泵下达 `abort()` →
/// 泵任务被硬取消、在有限时间内终止，`join()` 报告其为 abort 取消（`true`）。
#[tokio::test]
async fn abort_hard_cancels_pump_stuck_on_slow_read() {
    let probe = Arc::new(AtomicBool::new(false));
    let slow_local = NeverReturnsRead::new(Arc::clone(&probe));
    let (underlay_pump, _underlay_test) = tokio::io::duplex(64 * 1024);
    let (health_w, _health_r) = health_view();

    let handle = spawn_bridge(slow_local, underlay_pump, health_w);

    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        probe.load(Ordering::SeqCst),
        "pump polled the slow read before abort"
    );

    handle.abort();

    let cancelled = timeout(CANCEL_DEADLINE, handle.join())
        .await
        .expect("abort must terminate the pump task in flight");
    // abort 路径：join 报告任务被取消（true），不等待在途读自然返回。
    assert!(
        cancelled,
        "abort hard-cancelled the pump task (in-flight read never returned)"
    );
}

// ── §8 有界缓冲（§3.6 资源边界）：结构保证——固定大小缓冲、非无界 ────────────

/// §8 有界缓冲（结构/代码形态）：泵单方向承接缓冲是**编译期常量**、有界且非零
/// （[`BRIDGE_BUFFER_BYTES`]）——非 `read_to_end` 式无界累积。钉死「有界缓冲」是
/// 结构保证，不随运行时流量增长。
#[test]
fn bridge_buffer_is_a_bounded_compile_time_constant() {
    // 经运行时绑定读取常量（避免 clippy 把整条断言判为常量真值）。
    let buf_bytes = std::hint::black_box(BRIDGE_BUFFER_BYTES);
    // 有界：非零下限 + 合理上限（固定块大小，绝非无界堆积）。
    assert!(buf_bytes > 0, "bridge buffer must be non-zero");
    assert!(
        buf_bytes <= 1024 * 1024,
        "bridge buffer must be a fixed bounded block, not unbounded accumulation"
    );
}

// ── §8 有界缓冲（行为：常量真正接入搬运回路，非装饰常量） ────────────────────

/// §8 有界缓冲（行为绑定 — 杀「装饰性常量」突变）：把一个**记录单次被请求读字节数**
/// 的可控读端点装进泵，推入远大于 [`BRIDGE_BUFFER_BYTES`] 的载荷，断言泵向底层读单次
/// 请求的最大字节数**恰等于** `BRIDGE_BUFFER_BYTES`——证明该常量真正决定搬运承接缓冲
/// （泵以容量恰为该常量的缓冲做 `read → write`），而非一个对搬运零作用的装饰常量。
///
/// 这条断言把「有界缓冲」从「常量落在某区间」（同义反复）升级为「常量即搬运承接上界」
/// 的行为事实：
/// - 若实现退回 `tokio::io::copy`（内部缓冲 8KB 硬编码、与 `BRIDGE_BUFFER_BYTES`=16KB
///   解耦），单次请求峰值不再等于该常量 → 本断言钉红；
/// - 唯有把读半包进容量恰为 `BRIDGE_BUFFER_BYTES` 的缓冲（`copy_buf` + `BufReader`），
///   单次请求峰值才会精确锁到该常量 → 绿。
///
/// 因此即便有人把常量改成别的值，该值仍会被实测搬运峰值跟随（常量↔行为耦合），
/// 装饰常量不再能蒙混过关。
#[tokio::test]
async fn bridge_buffer_constant_bounds_actual_pump_read_size() {
    let max_read = Arc::new(AtomicUsize::new(0));
    // 本地端点：读半在单次 poll 内尽量填满被请求的缓冲，并记录「单次被请求容量」的峰值。
    let recording_local = RecordingReader::new(Arc::clone(&max_read), BRIDGE_BUFFER_BYTES * 8);
    // 底层测试侧持续读走前向字节，保证前向 copy 回路持续运转、缓冲被反复填满到峰值。
    let (underlay_pump, mut underlay_test) = tokio::io::duplex(BRIDGE_BUFFER_BYTES * 16);
    let (health_w, _health_r) = health_view();

    let _handle = spawn_bridge(recording_local, underlay_pump, health_w);

    // 持续把前向字节抽走，给泵反复填满承接缓冲到其上界的机会（远超单缓冲容量的总量）。
    let mut sink = vec![0u8; BRIDGE_BUFFER_BYTES * 4];
    underlay_test
        .read_exact(&mut sink)
        .await
        .expect("drain forwarded bytes so the pump keeps filling its bounded buffer");

    // 给调度器若干轮，让记录的峰值稳定到「单次请求 == 缓冲容量」。
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    let observed = max_read.load(Ordering::SeqCst);
    // 单次请求峰值恰为 BRIDGE_BUFFER_BYTES：常量即搬运承接缓冲上界（接入回路、非装饰）。
    assert_eq!(
        observed, BRIDGE_BUFFER_BYTES,
        "the pump's per-read request size must equal BRIDGE_BUFFER_BYTES — the constant must drive the \
         actual transfer buffer (copy_buf + BufReader::with_capacity), not be a decorative value that \
         the transfer path ignores"
    );
}

// ── §8 有界缓冲：写慢消费端不致读侧无界堆积（背压行为观察） ────────────────

/// §8 有界缓冲（背压行为）：以**慢/停滞消费端**（始终不读的底层）驱动，向本地端点持续
/// 写入远超单缓冲的字节 → 因有界缓冲 + 写慢背压，本地侧 `write_all` 不会无界地把全部
/// 字节吞下（被背压挡住），从而泵不在读侧无界堆积。
///
/// 钉死：在底层消费端完全停滞时，本地侧一次性 `write_all` 一个远大于缓冲与管道容量的
/// 载荷**不能**在限定时间内完成（被背压阻塞）——证明搬运受有界缓冲约束、非无界吞吐。
#[tokio::test]
async fn stalled_consumer_backpressures_writer_no_unbounded_buildup() {
    // 本地端点管道容量取小，确保背压能传导回 write_all（非测试管道本身吸收一切）。
    let (mut local_test, local_pump) = tokio::io::duplex(BRIDGE_BUFFER_BYTES);
    // 底层测试侧**从不读取**——消费端停滞，泵的底层写很快被填满阻塞。
    let (underlay_pump, _underlay_test_never_reads) = tokio::io::duplex(BRIDGE_BUFFER_BYTES);
    let (health_w, _health_r) = health_view();

    let _handle = spawn_bridge(local_pump, underlay_pump, health_w);

    // 远超缓冲 + 管道容量总和的载荷：若泵无界堆积则会全部吞下并完成；有界缓冲 + 背压
    // 下，write_all 应被阻塞、在限定时间内**无法**完成。
    let huge = vec![0xABu8; BRIDGE_BUFFER_BYTES * 64];
    let push = timeout(Duration::from_millis(300), local_test.write_all(&huge)).await;

    assert!(
        push.is_err(),
        "bounded buffer + backpressure must block the writer when the consumer stalls \
         (no unbounded read-side buildup)"
    );
}

// ── 注入的可控慢端点：读 future 永久 Pending（用于 L-6 砍在飞观察） ────────

/// 永不返回的可控端点：`poll_read` 恒返回 `Poll::Pending`（并记录已被 poll 过，供取消点
/// 命中观察）；`poll_write` 即时吞下字节。用作 L-6「在途读永不返回」的注入端点——泵若
/// 真砍在飞，则不等待此读返回即可终止。本类型不含任何机密 / 协议解析。
struct NeverReturnsRead {
    /// 记录读是否被 poll 过——取消前泵确在等读（取消点命中观察）。
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
        // 标记「读已被 poll」（取消前泵在途读存在），随后永久挂起——绝不返回。
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
        // 写即时吞下（反向目的端不阻塞）——本端点只在「读」上制造永不返回。
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// ── 注入的可控端点：读即时返回 IO 错误（用于 RST / IO 错误 → Dead 观察） ────

/// 读即时以 IO 错误返回的可控端点：`poll_read` 恒返回 `Poll::Ready(Err(ConnectionReset))`
/// （模拟对端 RST / 半截 IO 错误），使搭在其上的 `copy_buf` 回路以 `Err` 完结；`poll_write`
/// 即时吞下字节。用于钉死「方向以 Err 收尾亦收口为死亡，不被静默吞掉留伪 Alive」。
/// 本类型不含任何机密 / 协议解析。
struct ErrorOnRead;

impl ErrorOnRead {
    fn new() -> Self {
        Self
    }
}

impl AsyncRead for ErrorOnRead {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // 读即时以 IO 错误返回（对端 RST 形态）——copy 回路据此以 Err 完结。
        Poll::Ready(Err(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )))
    }
}

impl AsyncWrite for ErrorOnRead {
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

// ── 注入的可控端点：记录「单次被请求读字节数」峰值（用于常量↔搬运缓冲绑定） ──

/// 记录单次读被请求字节数峰值的可控端点：每次 `poll_read` 把 `buf.remaining()`（本次被
/// 请求的容量）以 `fetch_max` 记入 `max_read`，再用任意载荷填满（受总载荷 `remaining`
/// 限制）；载荷耗尽后返回 `Poll::Pending`（不 EOF，避免提前杀泵）。`poll_write` 即时吞下。
///
/// 配合泵以容量恰为 `BRIDGE_BUFFER_BYTES` 的 `BufReader` + `copy_buf` 承接搬运时，本端点
/// 单次被请求的容量峰值即等于该缓冲容量——从而把「常量」钉成「搬运承接缓冲上界」的行为
/// 事实。本类型不含任何机密 / 协议解析。
struct RecordingReader {
    /// 单次被请求读字节数的峰值（跨 poll 取 max）。
    max_read: Arc<AtomicUsize>,
    /// 剩余可供给的载荷字节数（远大于单缓冲，确保缓冲被反复填满到上界）。
    remaining: usize,
}

impl RecordingReader {
    fn new(max_read: Arc<AtomicUsize>, total_payload: usize) -> Self {
        Self {
            max_read,
            remaining: total_payload,
        }
    }
}

impl AsyncRead for RecordingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // 记录本次被请求的容量（搬运承接缓冲的单次上界）——跨 poll 取峰值。
        let offered = buf.remaining();
        self.max_read.fetch_max(offered, Ordering::SeqCst);

        if self.remaining == 0 {
            // 载荷已尽：永久挂起（不 EOF，避免泵因 EOF 提前退出干扰峰值观察）。
            return Poll::Pending;
        }

        // 用任意载荷填满本次被请求的容量（受总载荷限制），制造持续搬运。
        let n = offered.min(self.remaining);
        buf.initialize_unfilled_to(n);
        buf.advance(n);
        self.remaining -= n;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for RecordingReader {
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
