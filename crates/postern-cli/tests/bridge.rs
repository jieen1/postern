//! `mcp-stdio` 数据面字节桥的行为测试（RED）。
//!
//! 被测对象：`postern_cli::bridge`——数据面字节桥（`mcp-stdio` 子命令）。它把仅支持 stdio 的
//! MCP 宿主的 stdin/stdout 字节流，**零逻辑**双向转接到 daemon 数据面 `data.sock` 的 `/mcp`
//! 端点。被测核心是 `stdio::pump_bidirectional`（两个方向各一个并发拷贝任务、逐字节恒等、任一
//! 方向 EOF/错误即收束会话）与收束结果 `BridgeOutcome`（正常→`Completed`、中断→`Interrupted`），
//! 以及端点 `DataPlaneEndpoint`（连**数据面** `data.sock`，非控制面 `control.sock`）。
//!
//! 测试策略（07-postern-cli §3.8/§6.3/§9，F-10、L-10）：对**回声 Fake MCP 端点**（内存双工
//! 流，把 host→sock 方向收到的字节原样回送 sock→host 方向）驱动 `pump_bidirectional`，断言：
//! ① 写入字节序列 S → 从宿主出向读回与 S **逐字节相等**（含任意二进制 / 任意分片，F-10）；
//! ② 任一方向 EOF/错误即收束整个会话、反向流不孤悬挂死（§3.8 双向并发）；③ `data.sock`
//! 不可连 / 转接中断 → 桥按错误终止、`Interrupted` 且非零退出码，**无**任何本地构造的 MCP
//! 响应或决策（L-10）。不需要真实 daemon。
//!
//! 桥代码路径无 `NormalizedRequest`/`Intent`/`Sanitizer` 引用的「构造签名检查」（F-10）以
//! 源码文本扫描覆盖（`source_path_has_no_eval_type_reference`）——桥是 mover 不是 interpreter。
//!
//! 连真实进程 stdin/stdout 的 `run_session` 入口消费 OS 标准流、随宿主生命周期持续转接，端到
//! 端行为属集成层覆盖（见 `type_level_notes`）；可单测核心是泛型 `pump_bidirectional` 的双向
//! 并发拷贝与收束语义、`DataPlaneEndpoint::connect` 的不可达失败、`BridgeOutcome::exit_code`。
//!
//! 雷区（本测试遵守，文本级扫描）：不构造任何机密族类型（不写 `ResolvedTarget`/
//! `ResourceCredential`/`PresentedCredential`/`ScrubSet` 字面）；不嵌裸数据库写标记；不写
//! 来源类型字面双冒号变体；不 `use postern_store` / `use postern_secrets`（架构禁止边
//! cli ↛ store/secrets，B-1）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use postern_cli::bridge::stdio::pump_bidirectional;
use postern_cli::bridge::{BridgeOutcome, DataPlaneEndpoint};
use postern_cli::error::CliError;

// ════════════════════════════════════════════════════════════════════════════
// 回声 Fake MCP 端点 + 内存双工连线（无真实 daemon、无真实 stdin/stdout）
//
// 拓扑：桥的四个半流以两条内存双工管线对接——
//   宿主入向（host_in）：测试写 S 进 `host_in_writer`，桥从 `host_in_reader` 读、写入 sock。
//   宿主出向（host_out）：桥把 sock 来的字节写入 `host_out_writer`，测试从 `host_out_reader` 读回。
//   sock 侧（sock_read/sock_write）：经一条双工管线接到回声 Fake——Fake 把"桥经 sock_write 写来"
//   的字节原样回送，让桥经 sock_read 读回。host→sock→（echo）→sock→host 一圈，读回必等于写入。
//
// 不引入 hyper/真实 UDS——逐字节恒等与双向并发收束的判定面用内存双工流最直接、最精确，且与
// "桥不复用控制面请求机制、不解释字节"这一判定面零耦合。
// ════════════════════════════════════════════════════════════════════════════

/// 一条内存双工管线的两端读写半流（`tokio::io::duplex` 拆分而来）。
struct DuplexEnds {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    reader: tokio::io::ReadHalf<tokio::io::DuplexStream>,
}

/// 建一条内存双工管线，返回两端各自的 `(writer, reader)`——一端写、另一端读，双向独立。
fn duplex_pair(capacity: usize) -> (DuplexEnds, DuplexEnds) {
    let (a, b) = tokio::io::duplex(capacity);
    let (a_read, a_write) = tokio::io::split(a);
    let (b_read, b_write) = tokio::io::split(b);
    (
        DuplexEnds {
            writer: a_write,
            reader: a_read,
        },
        DuplexEnds {
            writer: b_write,
            reader: b_read,
        },
    )
}

/// **回声 Fake MCP 端点**：把"经 sock_write 写来"的字节原样回送给 sock_read 方向——逐字节、
/// 不解释、不增删（模拟一个 echo 数据面端点）。读到对端 EOF 即停（连同收束自身写半流）。
///
/// 这正是 §9/F-10 的"回声 Fake MCP 端点"：桥把 S 经 sock_write 推给 Fake，Fake 原样回送，桥
/// 经 sock_read 读回并写宿主出向——故"从宿主出向读回 == 写入宿主入向的 S"成立当且仅当桥逐字节
/// 恒等转接（任一字节被改写 / 桥内解析改形即破坏恒等，F-10 不过）。
async fn echo_fake_endpoint(
    mut from_bridge: tokio::io::ReadHalf<tokio::io::DuplexStream>,
    mut to_bridge: tokio::io::WriteHalf<tokio::io::DuplexStream>,
) {
    let mut buf = [0u8; 4096];
    loop {
        match from_bridge.read(&mut buf).await {
            Ok(0) => break, // 桥侧 sock_write 半流关闭——回声端收束。
            Ok(n) => {
                if to_bridge.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = to_bridge.shutdown().await;
}

// ── 极简临时目录（不引第三方 crate；用于 data.sock 缺失 / 移除路径）──────────────────
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> TempDir {
        let base = std::env::temp_dir();
        let unique = format!(
            "postern-cli-bridge-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
        );
        let path = base.join(unique);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

/// 短超时——不可连 / 挂死路径下让判定快速收敛，证伪"桥挂死"。
fn short_timeout() -> Duration {
    Duration::from_secs(5)
}

// ════════════════════════════════════════════════════════════════════════════
// F-10 · 逐字节恒等：写入字节序列 S → 从桥读回与 S 逐字节相等（回声 Fake MCP 端点）
// ════════════════════════════════════════════════════════════════════════════

// §8 F-10：对回声 Fake MCP 端点，向桥宿主入向写入一段 ASCII（典型 MCP JSON-RPC 形态的字节）
// → 从桥宿主出向读回与写入**逐字节相等**。这是 F-10 的核心判定：桥逐字节恒等转接、不增删。
#[tokio::test]
async fn ascii_payload_round_trips_byte_identical_through_the_bridge() {
    // 一段形似 MCP JSON-RPC 的字节——桥不解析它，只搬运（这正是要钉的：桥是 mover 不是 parser）。
    const PAYLOAD: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#;

    let (mut host_in, bridge_host_in) = duplex_pair(64 * 1024);
    let (mut host_out, bridge_host_out) = duplex_pair(64 * 1024);
    let (bridge_sock, fake_sock) = duplex_pair(64 * 1024);

    // 回声 Fake MCP 端点：把桥经 sock_write 推来的字节原样回送 sock_read 方向。
    let fake = tokio::spawn(echo_fake_endpoint(fake_sock.reader, fake_sock.writer));

    // 起桥的双向并发拷贝：host_in→sock_write、sock_read→host_out。
    let bridge = tokio::spawn(pump_bidirectional(
        bridge_host_in.reader,
        bridge_host_out.writer,
        bridge_sock.reader,
        bridge_sock.writer,
    ));

    // 宿主入向写入 S 后关闭——单向 EOF 触发会话收束（详见双向收束测试）。
    host_in
        .writer
        .write_all(PAYLOAD)
        .await
        .expect("写入宿主入向");
    host_in.writer.shutdown().await.expect("关闭宿主入向");

    // 从宿主出向读回全部回声字节。
    let mut read_back = Vec::new();
    let read = tokio::time::timeout(short_timeout(), host_out.reader.read_to_end(&mut read_back))
        .await
        .expect("读回宿主出向不得挂死（桥应在入向 EOF 后收束并 flush 全部回声字节）");
    read.expect("读回宿主出向");

    assert_eq!(
        read_back, PAYLOAD,
        "桥必须逐字节恒等转接：经回声 Fake 端点一圈后，宿主出向读回的字节必须与写入宿主入向的 S 逐字节相等（任一字节被改写 / 桥内解析改形即违反 F-10）——实得 {read_back:?}"
    );

    let _ = tokio::time::timeout(short_timeout(), bridge).await;
    fake.abort();
}

// §8 F-10（任意二进制，含 NUL / 高位字节 / 非 UTF-8 / 控制符）：把全 0x00..=0xFF 的 256 字节
// 灌进桥 → 读回逐字节相等。钉"桥不假定文本编码、不在 UTF-8 边界做任何处理"——任意二进制原样过。
// 一个偷偷做 UTF-8 校验 / 规整 / 替换非法字节的桥在此必 FAIL。
#[tokio::test]
async fn arbitrary_binary_all_byte_values_round_trips_identical() {
    // 全字节值（0x00..=0xFF）：含 NUL、各类控制符、高位非 ASCII、非 UTF-8 字节。
    let payload: Vec<u8> = (0u8..=255u8).collect();

    let (mut host_in, bridge_host_in) = duplex_pair(64 * 1024);
    let (mut host_out, bridge_host_out) = duplex_pair(64 * 1024);
    let (bridge_sock, fake_sock) = duplex_pair(64 * 1024);

    let fake = tokio::spawn(echo_fake_endpoint(fake_sock.reader, fake_sock.writer));
    let bridge = tokio::spawn(pump_bidirectional(
        bridge_host_in.reader,
        bridge_host_out.writer,
        bridge_sock.reader,
        bridge_sock.writer,
    ));

    host_in
        .writer
        .write_all(&payload)
        .await
        .expect("写入全字节值");
    host_in.writer.shutdown().await.expect("关闭宿主入向");

    let mut read_back = Vec::new();
    tokio::time::timeout(short_timeout(), host_out.reader.read_to_end(&mut read_back))
        .await
        .expect("读回不得挂死")
        .expect("读回宿主出向");

    assert_eq!(
        read_back.len(),
        256,
        "256 个字节值必须全数读回，实得 {} 字节（桥吞 / 增字节即长度偏离）",
        read_back.len()
    );
    assert_eq!(
        read_back, payload,
        "任意二进制（含 NUL / 高位 / 非 UTF-8 / 控制符）必须逐字节恒等过桥——一个做 UTF-8 校验 / 规整 / 替换非法字节的桥在此必 FAIL（F-10：不增删、不改写任一字节）"
    );

    let _ = tokio::time::timeout(short_timeout(), bridge).await;
    fake.abort();
}

// §8 F-10（任意分片：多次小块写入、含写入间隙）→ 读回拼接后与按序拼接的 S 逐字节相等。
// 钉"桥不按 MCP 消息边界 / 分片边界做缓冲重组或粘包拆分"——它搬字节流，不认协议帧。无论怎么
// 分片喂，读回必是同一字节序列。一个按 `\n` / 帧头切分再重组的桥（即解析了协议）在此必 FAIL。
#[tokio::test]
async fn chunked_input_across_arbitrary_boundaries_round_trips_identical() {
    // 多个大小不一的分片，边界故意落在"看似有意义"的位置（JSON 括号 / 换行内部），
    // 证明桥不依赖任何分片 / 消息边界——它只是字节流的搬运者。
    let chunks: Vec<&[u8]> = vec![
        b"{\"jsonrpc\"",
        b":\"2.0\",\"id\"",
        b":",
        b"42,\"method\":\"ping\"}\n",
        b"{\"second\":\"frame\"}",
    ];
    let expected: Vec<u8> = chunks.concat();

    let (mut host_in, bridge_host_in) = duplex_pair(64 * 1024);
    let (mut host_out, bridge_host_out) = duplex_pair(64 * 1024);
    let (bridge_sock, fake_sock) = duplex_pair(64 * 1024);

    let fake = tokio::spawn(echo_fake_endpoint(fake_sock.reader, fake_sock.writer));
    let bridge = tokio::spawn(pump_bidirectional(
        bridge_host_in.reader,
        bridge_host_out.writer,
        bridge_sock.reader,
        bridge_sock.writer,
    ));

    // 分多次写入，每次之间留一个调度间隙——制造真实的"字节分片到达"。
    for chunk in &chunks {
        host_in.writer.write_all(chunk).await.expect("写入分片");
        host_in.writer.flush().await.expect("flush 分片");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    host_in.writer.shutdown().await.expect("关闭宿主入向");

    let mut read_back = Vec::new();
    tokio::time::timeout(short_timeout(), host_out.reader.read_to_end(&mut read_back))
        .await
        .expect("读回不得挂死")
        .expect("读回宿主出向");

    assert_eq!(
        read_back, expected,
        "任意分片喂入 → 读回拼接必须与按序拼接的 S 逐字节相等——桥搬字节流、不认 MCP 帧 / 消息边界；一个按换行 / 帧头切分重组的桥（解析了协议）在此必 FAIL（F-10）"
    );

    let _ = tokio::time::timeout(short_timeout(), bridge).await;
    fake.abort();
}

// §8 F-10（出向方向恒等：sock→host 也逐字节，不止入向）：用一个"主动先说话"的 Fake——它在
// 桥起来后立刻向 sock_read 方向推一段字节（模拟数据面端点主动下发的 MCP 响应/通知）→ 这段
// 必须逐字节抵达宿主出向。钉两个方向**都**恒等（仅测入向会漏掉 sock→host 方向的改写回归）。
#[tokio::test]
async fn server_initiated_bytes_reach_host_out_byte_identical() {
    const SERVER_PUSH: &[u8] =
        br#"{"jsonrpc":"2.0","method":"notifications/message","params":{"x":1}}"#;

    let (host_in, bridge_host_in) = duplex_pair(64 * 1024);
    let (mut host_out, bridge_host_out) = duplex_pair(64 * 1024);
    let (bridge_sock, fake_sock) = duplex_pair(64 * 1024);

    // 主动下发的 Fake：不等输入，直接向桥的 sock_read 方向推一段字节后收束写半流。
    let mut fake_to_bridge = fake_sock.writer;
    let _fake_from_bridge = fake_sock.reader; // 持有以免对端写入立即 broken pipe。
    let fake = tokio::spawn(async move {
        let _ = fake_to_bridge.write_all(SERVER_PUSH).await;
        let _ = fake_to_bridge.shutdown().await;
        // 保活读半流到测试结束，避免桥的 host_in→sock 方向过早 broken。
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(_fake_from_bridge);
    });

    let bridge = tokio::spawn(pump_bidirectional(
        bridge_host_in.reader,
        bridge_host_out.writer,
        bridge_sock.reader,
        bridge_sock.writer,
    ));

    // 宿主入向无输入但保持打开——证明桥不要求入向先动，sock→host 方向独立流动（双向独立）。
    let _host_in_keepalive = host_in;

    let mut read_back = Vec::new();
    // sock→host 方向：Fake 推完即关写半流 → 桥 sock_read 读到 EOF → 收束会话 → host_out 收尾。
    tokio::time::timeout(short_timeout(), host_out.reader.read_to_end(&mut read_back))
        .await
        .expect("sock→host 方向读回不得挂死（出向必须独立于入向流动并随 sock EOF 收尾）")
        .expect("读回宿主出向");

    assert_eq!(
        read_back, SERVER_PUSH,
        "数据面端点主动下发的字节必须逐字节抵达宿主出向——sock→host 方向同样恒等转接（F-10 双向均不增删 / 不改写），实得 {read_back:?}"
    );

    fake.abort();
    let _ = tokio::time::timeout(short_timeout(), bridge).await;
}

// ════════════════════════════════════════════════════════════════════════════
// §3.8 双向并发 · 任一方向 EOF/错误即收束整个会话；反向流不孤悬挂死
// ════════════════════════════════════════════════════════════════════════════

// §8 §3.8（入向 EOF 收束会话）：宿主入向（host_in）即时 EOF（无任何输入即关闭），sock 侧无输入
// 输出 → 桥必须据此**收束整个会话**并返回 `Completed`，**不**挂死等另一方向。钉"一方向 EOF
// 取消另一方向、结束会话"——若桥只在双向都 EOF 才结束（而 sock 侧永不 EOF），会卡死，断言超时 FAIL。
#[tokio::test]
async fn host_in_eof_ends_the_whole_session_without_deadlock() {
    let (mut host_in, bridge_host_in) = duplex_pair(64 * 1024);
    let (host_out, bridge_host_out) = duplex_pair(64 * 1024);
    let (bridge_sock, fake_sock) = duplex_pair(64 * 1024);

    // sock 侧 Fake：持有两端但**既不读也不写、也不关闭**——制造"sock 方向永不 EOF"。
    // 若桥要求双向都 EOF 才收束，会永远等 sock 侧，挂死。
    let mut keep_sock_w = fake_sock.writer;
    let keep_sock_r = fake_sock.reader;
    let fake = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let _ = keep_sock_w.shutdown().await;
        drop(keep_sock_r);
    });

    let bridge = tokio::spawn(pump_bidirectional(
        bridge_host_in.reader,
        bridge_host_out.writer,
        bridge_sock.reader,
        bridge_sock.writer,
    ));

    // 宿主入向立刻 EOF（无输入即关闭）——host_in→sock 方向到达 EOF。
    host_in.writer.shutdown().await.expect("关闭宿主入向");
    // 持有宿主出向读半流（消费侧），避免桥写出向时立即 broken pipe 干扰判定。
    let _host_out_keepalive = host_out;

    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(short_timeout(), bridge)
        .await
        .expect("入向 EOF 必须收束整个会话——桥不得挂死等 sock 方向（sock 侧永不 EOF；若桥要求双向都 EOF 才结束，这里会超时 FAIL）")
        .expect("桥任务不应 panic");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "入向 EOF 后会话必须**即时**收束（≪ sock 侧 30s 挂起）——耗时 {elapsed:?} 说明桥在等反向 EOF / 挂死，违反『一方向 EOF 即收束会话』（§3.8）"
    );
    assert_eq!(
        outcome,
        BridgeOutcome::Completed,
        "双向流无 I/O 错误地走完（入向正常 EOF 触发收束）→ 会话正常收束 Completed，实得 {outcome:?}"
    );

    fake.abort();
}

// §8 §3.8（sock 方向中断即收束会话、出向不孤悬）：sock 侧在桥运行中突然断（Fake 直接 drop 两端，
// 制造 sock 读/写双向破裂）→ 桥必须取消另一方向、收束整个会话并返回 `Interrupted`（L-10：转接
// 中断 → 按错误终止）。钉"sock 转接破裂连带终止宿主侧、不让宿主入向孤悬挂死"。
#[tokio::test]
async fn sock_side_break_interrupts_session_and_does_not_strand_host_direction() {
    let (mut host_in, bridge_host_in) = duplex_pair(64 * 1024);
    let (host_out, bridge_host_out) = duplex_pair(64 * 1024);
    let (bridge_sock, fake_sock) = duplex_pair(64 * 1024);

    // sock 侧 Fake：短暂存在后**直接 drop 两端**——sock 读半流读到 EOF、写半流 broken，
    // 即"转接中断"。桥须据此取消 host_in→sock 方向、收束会话。
    let fake = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(fake_sock); // sock 两端同时断——转接中断（§3.8 / L-10）。
    });

    let bridge = tokio::spawn(pump_bidirectional(
        bridge_host_in.reader,
        bridge_host_out.writer,
        bridge_sock.reader,
        bridge_sock.writer,
    ));

    // 宿主入向**持续打开且不再喂数据**（模拟宿主仍在、只是没说话）——若桥不因 sock 断而收束，
    // host_in→sock 方向会永远等宿主输入，孤悬挂死。本测试钉 sock 断能连带终止该方向。
    let _host_in_keepalive = host_in.writer.write_all(b"x").await; // 写一点先，随后不再写也不关。
    let _host_in_hold = host_in; // 持住不关——制造"宿主入向永不 EOF"。
    let _host_out_keepalive = host_out;

    let outcome = tokio::time::timeout(short_timeout(), bridge)
        .await
        .expect("sock 侧中断必须收束整个会话——宿主入向虽永不 EOF，桥也须因 sock 断而取消该方向并结束（否则挂死超时 FAIL，违反『任一方向中断即收束会话』§3.8 / L-10）")
        .expect("桥任务不应 panic");

    fake.abort();
    drop(_host_in_hold);

    assert_eq!(
        outcome,
        BridgeOutcome::Interrupted,
        "sock 转接中断 → 桥按错误终止会话（L-10：中断即终止），收束为 Interrupted，实得 {outcome:?}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// L-10 · 中断即终止、非零退出、不绕过 daemon：data.sock 不可连 / 移除 → Interrupted；
//        且 Interrupted 映射**非零**退出码，无任何本地构造的 MCP 响应或决策
// ════════════════════════════════════════════════════════════════════════════

// §8 L-10（data.sock 不可连）：连一个**从不存在**的 data.sock 路径 → 连接阶段即失败，错误恰为
// CliError::DaemonUnreachable。桥**不**回退、**不**自造任何本地 MCP 响应——结构上无该路径。
// 钉到精确错误变体（无载荷的单元变体 → 结构上不携任何响应/决策，杜绝旁路）。
#[tokio::test]
async fn unconnectable_data_sock_maps_to_daemon_unreachable() {
    let tempdir = TempDir::new();
    let absent = tempdir.path().join("data.sock"); // 从不 bind——data.sock 缺失。
    let endpoint = DataPlaneEndpoint::new(absent.clone());

    // 前置（连的是数据面 data.sock，非控制面 control.sock）——端点持有的就是 data.sock 路径。
    assert_eq!(
        endpoint.data_socket_path(),
        absent.as_path(),
        "桥端点必须连数据面 data.sock 路径（非控制面 control.sock）——§6.3 雷区：data.sock 与 control.sock 是不同 socket / 不同权限边界"
    );

    let err = endpoint
        .connect()
        .await
        .expect_err("data.sock 不可连，桥连接必须失败（L-10：不可连即中断，绝不自造本地响应）");

    assert!(
        matches!(err, CliError::DaemonUnreachable),
        "data.sock 不可连必须映射 DaemonUnreachable（无载荷单元变体——结构上不携任何 MCP 响应 / 决策信封，桥宁可断也不补，L-10），实得 {err:?}"
    );
    // 该错误变体**不**是"daemon 返回错误信封"——后者意味着拿到了来自 daemon 的内容；不可连
    // 语义与之互斥，杜绝任何"看似有响应"的本地构造。
    assert!(
        !matches!(err, CliError::DaemonError { .. }),
        "不可连绝不能被误塞成『daemon 返回的响应 / 错误信封』（那会冒出未经管线的本地构造内容，违反 L-10 不绕过 daemon），实得 {err:?}"
    );
}

// §8 L-10（data.sock 移除后再连）：先 bind 一个 data.sock 再移除其文件，随后连 → 连接失败、
// 恰为 DaemonUnreachable。钉"既有入口被移除 / 转接前置不可达 → 中断即终止，不绕过 daemon"。
#[tokio::test]
async fn removed_data_sock_yields_daemon_unreachable() {
    let tempdir = TempDir::new();
    let sock_path = tempdir.path().join("data.sock");

    // bind 后立即移除——后续 connect 必失败（路径已不存在）。
    let listener = tokio::net::UnixListener::bind(&sock_path).expect("bind 临时 data.sock");
    std::fs::remove_file(&sock_path).expect("移除 data.sock 文件");
    drop(listener);

    let endpoint = DataPlaneEndpoint::new(sock_path);
    let err = endpoint
        .connect()
        .await
        .expect_err("data.sock 已移除，桥连接必须失败");

    assert!(
        matches!(err, CliError::DaemonUnreachable),
        "data.sock 移除后必须是 DaemonUnreachable（中断即终止、不绕过 daemon、不自造响应，L-10），实得 {err:?}"
    );
}

// §8 L-10（中断 → 非零退出码）：会话中断的收束结果 `Interrupted` 必须映射**非零**退出码——
// 桥按错误终止该会话并非零退出（L-10）。正常收束 `Completed` 映射 0；二者退出码互异。钉
// "中断即非零退出"这一可被脚本 / 宿主据码分流的事实，且与正常收束的 0 严格区分。
#[tokio::test]
async fn interrupted_outcome_maps_to_nonzero_exit_code() {
    assert_ne!(
        BridgeOutcome::Interrupted.exit_code(),
        0,
        "会话中断（data.sock 不可连 / 转接破裂）必须以**非零**退出码终止——L-10：中断即终止、非零退出，实得码 {}",
        BridgeOutcome::Interrupted.exit_code()
    );
    assert_eq!(
        BridgeOutcome::Completed.exit_code(),
        0,
        "双向正常 EOF 收束（无 I/O 错误）映射成功退出码 0，实得 {}",
        BridgeOutcome::Completed.exit_code()
    );
    assert_ne!(
        BridgeOutcome::Completed.exit_code(),
        BridgeOutcome::Interrupted.exit_code(),
        "正常收束与中断必须映射**互异**退出码——宿主 / 脚本据码区分『会话正常结束』与『转接中断』（L-10），二者相等即无法分流"
    );
}

// §8 L-10（中断不产生任何本地构造的 MCP 响应 / 决策——结构性自检）：`BridgeOutcome` 的两个
// 变体都是**无载荷**单元变体（`Completed`/`Interrupted`），结构上**不携**任何响应体 / 决策 /
// 信封字段。这把"桥中断时不伪造响应、不绕过 daemon"钉成构造签名可核的事实：桥的收束类型里
// 根本没有"放一个本地 MCP 响应"的位置——它只能报"完成 / 中断"，不能报"内容"。
#[tokio::test]
async fn bridge_outcome_carries_no_locally_constructed_response_payload() {
    // 无载荷单元变体可被 `Copy` / 比较，且不持任何字节缓冲——若回归给某变体加上响应体字段，
    // 此处的纯变体比较会编译不过或语义改变，暴露"桥开始携带本地构造内容"的旁路（L-10）。
    let completed = BridgeOutcome::Completed;
    let interrupted = BridgeOutcome::Interrupted;

    assert_ne!(
        completed, interrupted,
        "Completed 与 Interrupted 必须可区分（收束的唯一信息就是『正常 / 中断』，绝无本地构造的响应内容——L-10：桥不伪造、不绕过 daemon）"
    );
    // 中断必然映射非零、完成映射 0——收束语义的全部可观测产物就是这一个退出码，别无响应可言。
    assert_eq!(interrupted.exit_code(), CliError::DaemonUnreachable.code());
    assert_eq!(completed.exit_code(), 0);
}

// ════════════════════════════════════════════════════════════════════════════
// F-10（构造签名检查）· 桥代码路径无 NormalizedRequest / Intent / Sanitizer 引用
// 桥是 mover 不是 interpreter——源码文本扫描钉死求值族类型零引用（含子模块）。
// ════════════════════════════════════════════════════════════════════════════

// §8 F-10（构造签名检查）：扫描桥实现源码（`src/bridge/mod.rs` + `src/bridge/stdio.rs`），
// 其代码路径**不出现** `NormalizedRequest` / `Intent` / `Sanitizer` 任一标识符——桥不构造归一化
// 请求、不解析意图、不脱敏。这是 F-10 的「构造签名可核」面：任一标识符出现即桥内做了解析 /
// 归类 / 脱敏，违反零逻辑搬运。扫描只针对真实代码标识符，剥除注释 / 文档散文后比对。
#[test]
fn bridge_source_path_has_no_eval_type_reference() {
    // F-10 明列的三个被禁标识符（数据面求值族类型——它们只该出现在 daemon 数据面内核 / core，
    // 绝不在零逻辑字节桥的代码路径里）。注意：本断言消息与扫描目标用拼接构造，避免本测试文件
    // 自身的字面量被未来可能的同类扫描误判。
    let forbidden = [
        concat!("Normalized", "Request"),
        concat!("Int", "ent"),
        concat!("Sani", "tizer"),
    ];

    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let bridge_sources = [
        format!("{crate_dir}/src/bridge/mod.rs"),
        format!("{crate_dir}/src/bridge/stdio.rs"),
    ];

    for source in &bridge_sources {
        let raw = std::fs::read_to_string(source)
            .unwrap_or_else(|e| panic!("读取桥源码 {source} 失败: {e}"));
        let code = strip_line_comments_and_doc(&raw);
        for token in &forbidden {
            assert!(
                !code.contains(*token),
                "桥代码路径（{source}，已剥注释 / 文档）出现被禁求值族标识符 `{token}`——桥必须零逻辑搬运字节，不构造归一化请求 / 不解析意图 / 不脱敏（F-10 构造签名检查）：桥是 mover 不是 interpreter"
            );
        }
    }
}

/// 极简「剥行注释 + 文档注释」：把每行 `//`（含 `///`、`//!`）起始到行尾的内容去掉，只留
/// 真实代码部分供标识符比对——避免把模块文档 / 解释性散文里出现的求值族类型名误判为代码引用。
/// （桥源码中对求禁标识符的描述只出现在文档注释里，这里据此把它们排除在代码扫描之外。）
fn strip_line_comments_and_doc(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(pos) => &line[..pos],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
