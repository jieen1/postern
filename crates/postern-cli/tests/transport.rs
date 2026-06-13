//! 一次性 HTTP-over-UDS 往返的行为测试（RED）。
//!
//! 被测对象：`postern_cli::transport` 的一次性客户端——`UdsTransport`（包装 `control.sock`
//! 路径 + 超时，每次 `round_trip` 新建连接、发一次、读完整、关闭，无池化 / 无保活 / 无重试）
//! 与 `HttpResponse`（状态码 + 完整体字节，交渲染面分流）。
//!
//! 测试策略（07-postern-cli §3.2/§3.5/§3.6/§9，F-3、L-2/L-5/L-7）：对内存 Fake 控制面
//! （临时路径 UDS 监听器）跑一次往返，断言两侧——请求侧（method/path/query/body）与连接侧
//! （观测到的连接数恰为 1、命令结束连接关闭）。失败路径一等公民：socket 移除 → daemon 不
//! 可达且无决策结论；`409` → 冲突原样交还且 Fake 无后续自动重写请求；写端点 5xx → 如实交还
//! 且无本地补写 / 回滚 / 重试动作。不需要真实 daemon。
//!
//! L-9（崩溃不改变 daemon 行为）是**运行期行为观察**不变量，须起真实 daemon、命令往返中途
//! 杀 CLI 进程、观察 daemon 求值 / 连接 / 审计无差异——非内存 Fake 单测可覆盖，见
//! `type_level_notes`。L-11 的"两条顺序读命令删工作 / 缓存目录后行为一致 + 无 CLI 落地文件"
//! 部分以本文件 §8 测试覆盖（无本地状态、命令间无残留），文件系统残留断言亦在此。
//!
//! 雷区（本测试遵守，文本级扫描）：不构造任何机密族类型（`ResolvedTarget`/`ResourceCredential`/
//! `PresentedCredential`/`ScrubSet`）；不嵌裸数据库写标记；不写 `ConnOrigin` 字面双冒号；
//! 不 `use postern_store` / `use postern_secrets`（架构禁止边 cli ↛ store/secrets，B-1）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use postern_cli::error::CliError;
use postern_cli::reqspec::{audit_spec, elevate_spec, revoke_grant_spec, Method, RequestSpec};
use postern_cli::transport::{HttpResponse, UdsTransport};

// ════════════════════════════════════════════════════════════════════════════
// 内存 Fake 控制面（临时路径 UDS）— 观测请求侧 + 连接计数 + 可编程应答
//
// 这是一个手写的极简 HTTP/1.1 应答器：每个被接受的连接 = 一次"观测到的请求连接"，由
// `connections` 原子计数（F-3 的判定面）；它解析请求行（method/path?query）+ 头 + 体，
// 记进 `observed`，按预置 `status`/`body` 应答一次，然后关闭该连接（不保活）。
//
// 不引入 hyper server——一次往返的请求侧 / 连接侧观测用裸字节解析最直接、最精确，且与"被测
// 客户端不复用连接"这一判定面零耦合。
// ════════════════════════════════════════════════════════════════════════════

/// Fake 观测到的一次请求的请求侧事实（§9 请求侧断言面）。
#[derive(Debug, Clone, Default)]
struct ObservedRequest {
    /// 请求行方法（GET/POST/PUT/DELETE）。
    method: String,
    /// 请求行路径（`?` 左侧，6.5 的 `/v1/...`）。
    path: String,
    /// 查询串（`?` 右侧原文，未给则空串）。
    raw_query: String,
    /// 请求体原始字节（写端点有，读端点空）。
    body: Vec<u8>,
}

impl ObservedRequest {
    /// 把 `raw_query` 解析为 `(key, value)` 映射（键值精确、顺序不限，对齐 F-6 判定）。
    fn query_pairs(&self) -> BTreeMap<String, String> {
        let mut pairs = BTreeMap::new();
        if self.raw_query.is_empty() {
            return pairs;
        }
        for kv in self.raw_query.split('&') {
            if let Some((k, v)) = kv.split_once('=') {
                pairs.insert(k.to_string(), v.to_string());
            } else {
                pairs.insert(kv.to_string(), String::new());
            }
        }
        pairs
    }
}

/// Fake 控制面句柄：持监听任务、连接计数、观测记录与预置应答。
struct FakeControlPlane {
    /// `control.sock` 临时路径（命令端据此构造 `UdsTransport`）。
    socket_path: PathBuf,
    /// 被接受的连接累计数——F-3 判定面（恰 1 即过；0 或 ≥2 即不过）。
    connections: Arc<AtomicUsize>,
    /// 历次观测到的请求（按到达顺序）。
    observed: Arc<Mutex<Vec<ObservedRequest>>>,
    /// 监听任务句柄；drop 时连同临时 socket 文件一并清理。
    listener_task: tokio::task::JoinHandle<()>,
    /// 临时目录守卫（drop 即递归删，确保不在磁盘留 socket 残留）。
    _tempdir: TempDir,
}

impl Drop for FakeControlPlane {
    fn drop(&mut self) {
        self.listener_task.abort();
    }
}

/// Fake 在每个被接受连接上的应答形态（含失败路径形态，fail-closed 镜头）。
#[derive(Debug, Clone, Copy)]
enum ServeMode {
    /// 正常：解析一次请求、记入 observed、回完整 `Content-Length` 一致的固定应答后关连接。
    Full { status: u16, body: &'static [u8] },
    /// **静默 daemon**（超时路径）：accept 后既不读也不应答，把连接挂起直到对端超时关闭。
    /// 用于钉"connect 成功但 daemon 不应答 / 慢应答 → 读取超时 → DaemonUnreachable"（§3.9/L-2）。
    Silent,
    /// **半截 / 截断响应**（fail-closed 镜头"半截"路径）：解析完请求后，发声明
    /// `Content-Length: declared` 的状态行 + 头，但**只发 `sent` 个 body 字节**（`sent < declared`）
    /// 后 `shutdown`——制造"读到的体短于声明长度"的半截读失败，钉"半截 body 不当 `Ok` 交还"。
    Truncated {
        status: u16,
        declared: usize,
        sent: &'static [u8],
    },
}

impl FakeControlPlane {
    /// 起一个固定应答（`status` + `body`）的 Fake：监听临时路径 UDS，每个连接解析一次请求、
    /// 记入 `observed`、应答一次后关连接。`connections` 累计被接受连接数。
    async fn start(status: u16, body: &'static [u8]) -> FakeControlPlane {
        FakeControlPlane::start_with_mode(ServeMode::Full { status, body }).await
    }

    /// 起一个**静默 daemon** Fake：accept 后不应答，连接挂起——驱动超时失败路径（failclosed-1）。
    async fn start_silent() -> FakeControlPlane {
        FakeControlPlane::start_with_mode(ServeMode::Silent).await
    }

    /// 起一个**半截响应** Fake：声明 `declared` 字节但只发 `sent`（`sent.len() < declared`）后关连接——
    /// 驱动半截 / 截断读失败路径（failclosed-2）。
    async fn start_truncated(
        status: u16,
        declared: usize,
        sent: &'static [u8],
    ) -> FakeControlPlane {
        FakeControlPlane::start_with_mode(ServeMode::Truncated {
            status,
            declared,
            sent,
        })
        .await
    }

    /// 按给定 [`ServeMode`] 起 Fake——监听临时路径 UDS，`connections` 累计被接受连接数，
    /// 每连接按模式应答。所有模式共享同一 bind / accept / 计数骨架。
    async fn start_with_mode(mode: ServeMode) -> FakeControlPlane {
        let tempdir = TempDir::new();
        let socket_path = tempdir.path().join("control.sock");

        let listener = UnixListener::bind(&socket_path).expect("bind fake control.sock");
        let connections = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(Vec::new()));

        let conns = connections.clone();
        let obs = observed.clone();
        let listener_task = tokio::spawn(async move {
            loop {
                let (stream, _addr) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                // 每个被接受的连接 = 一次"观测到的请求连接"（F-3 判定面）。
                conns.fetch_add(1, Ordering::SeqCst);
                let obs = obs.clone();
                tokio::spawn(async move {
                    match mode {
                        ServeMode::Full { status, body } => {
                            serve_one(stream, status, body, obs).await;
                        }
                        ServeMode::Silent => {
                            // 静默：既不读也不应答，把连接长时间挂起，让对端读取超时收敛。
                            serve_silent(stream).await;
                        }
                        ServeMode::Truncated {
                            status,
                            declared,
                            sent,
                        } => {
                            serve_truncated(stream, status, declared, sent, obs).await;
                        }
                    }
                });
            }
        });

        FakeControlPlane {
            socket_path,
            connections,
            observed,
            listener_task,
            _tempdir: tempdir,
        }
    }

    /// 当前累计被接受连接数（F-3 判定）。
    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// 取观测到的请求快照（按到达顺序）。
    async fn requests(&self) -> Vec<ObservedRequest> {
        self.observed.lock().await.clone()
    }
}

/// 在单个连接上解析一次 HTTP/1.1 请求并回一次固定应答，然后关闭连接（不保活）。
async fn serve_one(
    mut stream: UnixStream,
    status: u16,
    body: &'static [u8],
    observed: Arc<Mutex<Vec<ObservedRequest>>>,
) {
    // 读到头尾分界 `\r\n\r\n`，再按 Content-Length 读体。
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut tmp).await {
            Ok(0) => return, // 对端在发完前关闭——不记请求。
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    break pos;
                }
            }
            Err(_) => return,
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let (path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };

    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }

    let body_start = header_end + 4;
    let mut body_bytes = buf[body_start..].to_vec();
    while body_bytes.len() < content_length {
        match stream.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => body_bytes.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    body_bytes.truncate(content_length);

    observed.lock().await.push(ObservedRequest {
        method,
        path,
        raw_query,
        body: body_bytes,
    });

    let reason = reason_phrase(status);
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// **静默 daemon**：连接已建立（connect / 握手成功），但 Fake 既不读请求也不发任何应答字节，
/// 只把流挂起一段远超被测超时的时长——制造"connect 成功但 daemon 不应答"的读取超时（§3.9/L-2）。
/// 被测 transport 应在其超时窗口内收敛为 `DaemonUnreachable`，而非挂死 / 伪造成功。
async fn serve_silent(stream: UnixStream) {
    // 持有 stream 不读不写，挂起远超测试超时的时间；transport 超时后会主动关连接。
    tokio::time::sleep(Duration::from_secs(30)).await;
    drop(stream);
}

/// **半截 / 截断响应**：正常解析一次请求并记入 `observed`，随后发出声明 `declared` 字节的
/// 状态行 + `Content-Length` 头，但**只写 `sent`（`sent.len() < declared`）**后即 `shutdown`——
/// 制造"声明 N 字节实发 M<N 字节"的半截读。被测 transport 读体不足声明长度即应 fail-closed
/// 为 `DaemonUnreachable`，**绝不**把残缺字节当 `Ok(HttpResponse)` 交还（fail-closed/L-2）。
async fn serve_truncated(
    mut stream: UnixStream,
    status: u16,
    declared: usize,
    sent: &'static [u8],
    observed: Arc<Mutex<Vec<ObservedRequest>>>,
) {
    // 先把请求读完整并记录（与 serve_one 一致），确保"连接被接受 + 请求已抵达"，
    // 把被测点收敛到"响应半截"而非"请求没发出去"。
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut tmp).await {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    break pos;
                }
            }
            Err(_) => return,
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let (path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    observed.lock().await.push(ObservedRequest {
        method,
        path,
        raw_query,
        body: Vec::new(),
    });

    // 声明 `declared` 字节但只发 `sent`（短于声明），随后关连接——半截 body。
    let reason = reason_phrase(status);
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(sent).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// 最小状态短语映射（仅测试需要的码）；其余给通用短语，应答解析不依赖短语文本。
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// 在字节缓冲里找子串首位置。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ── 极简临时目录（不引第三方 crate；drop 即递归删，断言"无磁盘残留"用得到）────────
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> TempDir {
        let base = std::env::temp_dir();
        let unique = format!(
            "postern-cli-transport-{}-{}",
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

/// 一个稳定的单条成功响应体（雪花 id 恒为字符串），多处复用。
const OK_SINGLE_BODY: &[u8] = br#"{"id":"7300000000000000123","version":3}"#;

/// 短超时——不可达 / 无响应路径下让 daemon-unreachable 判定快速收敛，不拖测试。
fn short_timeout() -> Duration {
    Duration::from_secs(2)
}

/// 极短超时——专用于静默 daemon（读取超时）路径：让超时分支在亚秒级收敛，既证伪"挂死"
/// （超时若不生效，测试会卡在 serve_silent 的 30s 挂起上），又把测试时长压到几百毫秒。
fn tiny_timeout() -> Duration {
    Duration::from_millis(400)
}

// ════════════════════════════════════════════════════════════════════════════
// F-3 · 一次命令 = 对 control.sock 恰一次请求连接（无后台保活、命令结束连接关闭）
// ════════════════════════════════════════════════════════════════════════════

// §8 F-3：对 Fake 跑一条集合命令（GET /v1/audit）→ Fake 侧观测到**恰一次**请求连接。
// 0 次或 ≥2 次（隐式重试 / 保活）即不过。这是 F-3 的核心判定。
#[tokio::test]
async fn one_collection_command_produces_exactly_one_connection() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let spec: RequestSpec = audit_spec(Some("agent3"), None, Some(1), Some(20));
    let resp = transport
        .round_trip(&spec)
        .await
        .expect("Fake 在监听且应答 200，一次往返必须成功");

    assert_eq!(resp.status, 200, "Fake 应答 200，状态原样交还");
    assert_eq!(
        resp.body, OK_SINGLE_BODY,
        "成功响应体必须原样、完整交还（含 >2^53 雪花 id 字符串与 version 信封）——读完整响应体是 transport 的核心产出（§3.2/§3.3）；一个把 body 丢弃 / 截断成空的 transport 在此必须 FAIL"
    );
    assert_eq!(
        fake.connection_count(),
        1,
        "一条命令必须对 control.sock 恰建一次连接——观测到 {} 次即违反 F-3（0=未发，≥2=隐式重试/保活）",
        fake.connection_count()
    );
}

// §8 F-3（无后台保活）：单次往返返回后再观察一小段时间，连接数仍恒为 1——客户端不在后台
// 另起保活 / 预连接。若实现持连接池或起保活任务，这里会冒出第 2 个连接，断言 FAIL。
#[tokio::test]
async fn no_background_or_keepalive_connection_after_round_trip() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let spec = audit_spec(None, None, None, None);
    let _ = transport.round_trip(&spec).await.expect("一次往返应成功");

    // 给潜在的后台保活 / 重连任务一个暴露窗口。
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        fake.connection_count(),
        1,
        "往返结束后不得有任何后台 / 保活连接——连接数必须仍恒为 1，实测 {}",
        fake.connection_count()
    );
}

// §8 F-3（连接结束即关，drop transport 不再产生连接）：两条**各自独立**的命令各建一次连接，
// 累计恰 2——即"每命令 1 条、命令结束即关"，绝非一条复用连接（复用会让计数停在 1）也绝非
// 隐式重试（会让计数 >2）。同一 transport 实例复用配置发两条命令，仍是两次独立短命连接。
#[tokio::test]
async fn each_command_opens_its_own_fresh_connection() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let _ = transport
        .round_trip(&audit_spec(None, None, None, None))
        .await
        .expect("第一条往返成功");
    assert_eq!(fake.connection_count(), 1, "第一条命令后恰 1 次连接");

    let _ = transport
        .round_trip(&audit_spec(Some("agent7"), None, None, None))
        .await
        .expect("第二条往返成功");
    assert_eq!(
        fake.connection_count(),
        2,
        "第二条命令必须新建独立连接（每命令一次性短命连接，不复用、不保活）——实测 {}",
        fake.connection_count()
    );
}

// ════════════════════════════════════════════════════════════════════════════
// F-2 (请求侧) · 一次往返的 method/path/query/body 恰为请求规格所述（喂 Fake 观测）
// 传输层把 RequestSpec 忠实落到线上请求行 / 查询串 / 体——这是 F-3 单往返"内容正确"的一面。
// ════════════════════════════════════════════════════════════════════════════

// §8 F-2（请求侧，对 docs/examples/07 §4.1-A）：audit --principal agent3 --page-no 1
// --page-size 20 → Fake 观测到 method=GET、path=/v1/audit、查询集恰含
// principal=agent3 / page_no=1 / page_size=20（键值精确，顺序不限），且读端点无请求体。
#[tokio::test]
async fn round_trip_sends_get_audit_with_exact_query_and_no_body() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let spec = audit_spec(Some("agent3"), None, Some(1), Some(20));
    let _ = transport.round_trip(&spec).await.expect("往返成功");

    let reqs = fake.requests().await;
    assert_eq!(reqs.len(), 1, "恰观测到一次请求");
    let req = &reqs[0];

    assert_eq!(req.method, "GET", "audit 是读端点，请求行方法必须 GET");
    assert_eq!(
        req.path, "/v1/audit",
        "请求行路径必须 /v1/audit（6.5 端点）"
    );

    let pairs = req.query_pairs();
    assert_eq!(pairs.get("principal").map(String::as_str), Some("agent3"));
    assert_eq!(pairs.get("page_no").map(String::as_str), Some("1"));
    assert_eq!(pairs.get("page_size").map(String::as_str), Some("20"));
    assert!(
        req.body.is_empty(),
        "读端点 audit 不得发请求体，实测 {} 字节",
        req.body.len()
    );
}

// §8 F-2/F-6（请求侧，差分守卫）：不给分页 → 线上查询串不含 page_no/page_size 任一键
// （由 daemon 取默认 20），过滤键 principal 仍按命令携带。把"缺则不带键"钉到真实线上请求。
#[tokio::test]
async fn round_trip_omits_pagination_keys_on_the_wire_when_absent() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let spec = audit_spec(Some("agent3"), None, None, None);
    let _ = transport.round_trip(&spec).await.expect("往返成功");

    let reqs = fake.requests().await;
    let pairs = reqs[0].query_pairs();
    assert_eq!(
        pairs.get("principal").map(String::as_str),
        Some("agent3"),
        "过滤键仍携带"
    );
    assert!(
        !pairs.contains_key("page_no") && !pairs.contains_key("page_size"),
        "缺分页 → 线上查询串不得含 page_no/page_size，实得键集: {:?}",
        pairs.keys().collect::<Vec<_>>()
    );
}

// §8 F-2/F-7（请求侧，对 docs/examples/06 §3.1）：elevate → POST /v1/grants/temp，
// 线上请求体 JSON 含 principal/capability/ttl 三字段且值精确。钉写端点的方法 / 路径 / 体内容。
#[tokio::test]
async fn round_trip_sends_post_grants_temp_with_body_fields() {
    let fake = FakeControlPlane::start(201, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let spec = elevate_spec("agent2", "redis-main", "destroy", "30m");
    let _ = transport.round_trip(&spec).await.expect("往返成功");

    let reqs = fake.requests().await;
    let req = &reqs[0];
    assert_eq!(req.method, "POST", "elevate 是写端点，方法 POST");
    assert_eq!(req.path, "/v1/grants/temp", "路径必须 /v1/grants/temp");

    let body: serde_json::Value =
        serde_json::from_slice(&req.body).expect("写端点请求体必须是合法 JSON");
    assert_eq!(body["principal"], "agent2", "体含 principal=agent2");
    assert_eq!(
        body["capability"], "destroy",
        "体含 capability=destroy（取自 --cap verb 段）"
    );
    assert_eq!(body["ttl"], "30m", "体含 ttl=30m");
}

// ════════════════════════════════════════════════════════════════════════════
// L-2 · daemon 不可达即拒绝（socket 缺失 / 连接失败 → DaemonUnreachable，无路径回退）
// ════════════════════════════════════════════════════════════════════════════

// §8 L-2（对 docs/examples/06 §4.2-12）：socket 不存在（从未起 Fake）→ 连接阶段即失败，
// 错误恰为 CliError::DaemonUnreachable。**绝不**回退本地策略 / 缓存——结构上无该路径。
// 钉到精确错误变体，非泛化失败。
#[tokio::test]
async fn missing_socket_maps_to_daemon_unreachable() {
    let tempdir = TempDir::new();
    let absent = tempdir.path().join("control.sock"); // 从不 bind——socket 缺失。
    let transport = UdsTransport::new(absent, short_timeout());

    let err = transport
        .round_trip(&audit_spec(None, None, None, None))
        .await
        .expect_err("socket 缺失，一次往返必须失败");

    assert!(
        matches!(err, CliError::DaemonUnreachable),
        "socket 缺失必须映射 DaemonUnreachable（连接阶段失败），实得 {err:?}"
    );
}

// §8 L-2（socket 移除后跑命令）：Fake 起后即移除其 socket 文件，再跑命令 → DaemonUnreachable，
// 退出码恰为该类的非零码（3），区别于本地语法拒绝（2）/ daemon 错误信封（4）/ 解析失败（5）。
// 钉"daemon 不可达"既是该错误变体、又据此映射其互异非零退出码（无任何决策结论输出）。
#[tokio::test]
async fn removed_socket_yields_daemon_unreachable_with_its_exit_code() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let path = fake.socket_path.clone();
    // 移除 socket 文件——后续 connect 必失败（路径已不存在）。
    std::fs::remove_file(&path).expect("移除 fake socket 文件");

    let transport = UdsTransport::new(path, short_timeout());
    let err = transport
        .round_trip(&revoke_grant_spec("7300000000000000123", Some(3)))
        .await
        .expect_err("socket 已移除，往返必须失败");

    assert!(
        matches!(err, CliError::DaemonUnreachable),
        "socket 移除后必须是 DaemonUnreachable，实得 {err:?}"
    );
    assert_eq!(
        err.code(),
        3,
        "daemon 不可达映射互异非零退出码 3（≠ 本地拒绝 2 / 错误信封 4 / 解析失败 5）"
    );
}

// §8 L-2（无路径回退的结构性自检）：不可达失败既不是成功、也不是 daemon 错误信封类
// （DaemonError）——后者意味着"连上了 daemon 并拿到了脱敏错误信封"，与不可达语义互斥。
// 钉不可达**不会**被误塞成"daemon 返回的决策 / 错误信封"，杜绝任何"看似有决策结论"的输出。
//
// L-2 输出面（无决策结论）：transport 的可观测产出是 `Result<HttpResponse, CliError>`——
// 不可达路径**必须**落在 `Err` 侧，绝不返回任何 `Ok(HttpResponse)`。一个 `Ok(HttpResponse)`
// 携 `status`+`body` 即会被上层渲染面据 status 分流、把 body 当信封呈现，从而冒出"看似有结论"
// 的 allow/deny/授权视图输出（违反 L-2"emits NO decision conclusion；any local decision
// output fails"）。本测试钉死：不可达**无任何 HttpResponse 交还**，且错误变体是无载荷的
// `DaemonUnreachable`（结构上不携 status/body/信封 → 渲染面无可呈现的决策结论）。
#[tokio::test]
async fn unreachable_is_not_misclassified_as_daemon_error_envelope() {
    let tempdir = TempDir::new();
    let absent = tempdir.path().join("control.sock");
    let transport = UdsTransport::new(absent, short_timeout());

    let outcome: Result<HttpResponse, CliError> = transport
        .round_trip(&audit_spec(None, None, None, None))
        .await;

    // 不可达**绝不**返回任何 HttpResponse——没有 status / body 可供渲染面变成决策结论输出。
    assert!(
        outcome.is_err(),
        "不可达必须落在 Err 侧、绝不交还任何 Ok(HttpResponse)（一旦交还 status+body，渲染面会据此呈现『看似有结论』的 allow/deny/授权视图，违反 L-2『any local decision output fails』），实得 {outcome:?}"
    );
    let err = outcome.expect_err("不可达必失败");

    assert!(
        !matches!(err, CliError::DaemonError { .. }),
        "不可达绝不能被误判为 daemon 返回错误信封（那意味着拿到了决策结论），实得 {err:?}"
    );
    assert!(
        matches!(err, CliError::DaemonUnreachable),
        "唯一正确归类是 DaemonUnreachable（无载荷的单元变体——结构上不携 status/body/决策信封，渲染面无可呈现的 allow/deny/授权视图），实得 {err:?}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// L-2 / fail-closed（失败路径一等公民）· 超时与半截响应——transport 层独有的真实失败分支
// §3.9：连接或读取超时 → DaemonUnreachable；半截 / 截断响应读失败 → DaemonUnreachable，
// 绝不把残缺字节当 Ok(HttpResponse) 交还（否则渲染面拿到残缺字节、可能呈现『看似有结论』）。
// ════════════════════════════════════════════════════════════════════════════

// §8 L-2/§3.9（读取超时 → daemon 不可达）：daemon 接受连接、握手成功，但**不应答 / 慢应答**
// （serve_silent 把连接挂起 30s）→ 被测 transport 必须在其超时窗口（tiny_timeout 400ms）内
// 收敛为 `DaemonUnreachable`，**而非**挂死（若超时分支失效，本测试会卡在 30s 挂起上而非快速返回）、
// 误分类（非 DaemonError）、或伪造成功（非 Ok）。这是 transport 层独有、socket-缺失用例覆盖不到的
// 真实失败分支（connect 成功但读取阶段超时）。
#[tokio::test]
async fn silent_daemon_read_timeout_maps_to_daemon_unreachable() {
    let fake = FakeControlPlane::start_silent().await;
    let transport = UdsTransport::new(fake.socket_path.clone(), tiny_timeout());

    let started = std::time::Instant::now();
    let outcome: Result<HttpResponse, CliError> = transport
        .round_trip(&audit_spec(Some("agent3"), None, Some(1), Some(20)))
        .await;
    let elapsed = started.elapsed();

    // connect 成功（连接被接受），故必有恰一次连接——证实走的是"连上但读超时"分支，
    // 而非"根本没连上"——把被测点钉到读取超时本身。
    assert_eq!(
        fake.connection_count(),
        1,
        "静默 daemon 测试前提：连接必须被接受恰一次（connect 成功），才证明走的是读取超时分支而非 connect 失败，实测 {}",
        fake.connection_count()
    );
    // 超时必须真正生效——收敛远早于 serve_silent 的 30s 挂起（给宽松上界，证伪『挂死』）。
    assert!(
        elapsed < Duration::from_secs(5),
        "读取超时必须快速收敛（≪ serve_silent 的 30s 挂起）——耗时 {elapsed:?} 说明超时分支失效 / 挂死"
    );
    // 失败必须落 Err 且恰为 DaemonUnreachable（超时归 daemon 不可达，§3.9/L-2）——
    // 绝不伪造成功（Ok）、绝不误分类成 daemon 错误信封（DaemonError = 拿到了决策结论）。
    assert!(
        matches!(outcome, Err(CliError::DaemonUnreachable)),
        "静默 daemon 读取超时必须恰映射 DaemonUnreachable（§3.9：连接或读取超时归 daemon 不可达）——绝不挂死 / 误分类 / 伪造成功，实得 {outcome:?}"
    );
}

// §8 L-2/fail-closed（半截 / 截断响应 → daemon 不可达）：daemon 接受连接、发声明
// `Content-Length: 100` 的头但**只发 3 字节 body** 后 shutdown（serve_truncated）→ 被测
// transport 读体不足声明长度即 fail-closed 为 `DaemonUnreachable`，**绝不**把残缺字节当
// `Ok(HttpResponse)` 交还。这是 fail-closed 镜头明列的"半截"路径、且最危险——若回归把半截读
// 当 Ok 交还，渲染面会拿到残缺字节、可能呈现"看似有结论"的输出（违反 L-2 无决策结论 + fail-closed）。
#[tokio::test]
async fn truncated_response_body_maps_to_daemon_unreachable_not_ok() {
    // 声明 100 字节，实发 3 字节后关连接——半截 body。
    let fake = FakeControlPlane::start_truncated(200, 100, b"{\"i").await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let started = std::time::Instant::now();
    let outcome: Result<HttpResponse, CliError> = transport
        .round_trip(&audit_spec(Some("agent3"), None, Some(1), Some(20)))
        .await;
    let elapsed = started.elapsed();

    // 连接被接受恰一次（请求已抵达），证明走的是"响应半截"分支而非"请求没发出"。
    assert_eq!(
        fake.connection_count(),
        1,
        "半截响应测试前提：连接必须被接受恰一次（请求抵达 Fake），才证明走的是响应半截分支，实测 {}",
        fake.connection_count()
    );
    // 半截读失败必须落 Err 且恰为 DaemonUnreachable——绝不把残缺 3 字节当 Ok(HttpResponse) 交还。
    assert!(
        matches!(outcome, Err(CliError::DaemonUnreachable)),
        "半截 / 截断响应（声明 100 字节实发 3 字节后关连接）必须 fail-closed 为 DaemonUnreachable，绝不把残缺字节当 Ok(HttpResponse) 交还渲染面（否则可能呈现『看似有结论』输出，违反 L-2 + fail-closed），实得 {outcome:?}"
    );
    // 半截读应在超时窗口内即时收敛（对端 shutdown 触发读结束），不至于拖到超时上界。
    assert!(
        elapsed < short_timeout(),
        "半截读应随对端 shutdown 即时收敛为失败，不应拖满超时窗口——耗时 {elapsed:?}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// L-5 · 409 Conflict 不静默重试（原样交还冲突状态；Fake 观测无后续自动重写请求）
// ════════════════════════════════════════════════════════════════════════════

// §8 L-5（对 docs/examples/02 §4.2-E7 等）：写命令收到 409 → 传输层原样交还
// status=409（交上层呈现冲突 + 提示重读 version），且 Fake 侧只观测到**一次**请求——
// 绝无"自动重写覆盖"的第 2 次写请求。同时连接数恰 1（不重试 = 不重连）。
#[tokio::test]
async fn conflict_409_is_surfaced_verbatim_without_auto_rewrite() {
    const CONFLICT_ENVELOPE: &[u8] =
        br#"{"error":{"code":"version_conflict","message":"version mismatch"}}"#;
    let fake = FakeControlPlane::start(409, CONFLICT_ENVELOPE).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let resp: HttpResponse = transport
        .round_trip(&revoke_grant_spec("7300000000000000123", Some(3)))
        .await
        .expect("Fake 应答 409 是一次正常往返（HTTP 层成功，业务层冲突由上层呈现）");

    assert_eq!(
        resp.status,
        HttpResponse::STATUS_CONFLICT,
        "409 状态必须原样交还（交上层呈现冲突 + 提示重读 version，不在传输层吞掉）"
    );
    assert_eq!(
        resp.body, CONFLICT_ENVELOPE,
        "409 冲突信封正存活于 body 中（L-5 要求原样呈现冲突 + 提示重读最新 version）——body 必须逐字交还渲染面；一个丢弃 / 截断 body 的 transport 在此必须 FAIL（仅断 status 无法区分『交还冲突体』与『丢弃 body』两种 transport）"
    );
    assert_eq!(
        fake.requests().await.len(),
        1,
        "收到 409 后绝不自动重写——Fake 必须只观测到 1 次请求（出现第 2 次即自动重试覆盖，违反 L-5）"
    );
    assert_eq!(
        fake.connection_count(),
        1,
        "409 不触发重连——连接数必须恒 1，实测 {}",
        fake.connection_count()
    );
}

// §8 L-5（差分守卫，给后台重试一个暴露窗口）：收到 409 返回后再观察一小段时间，
// Fake 侧请求数仍恒为 1——客户端不在后台延迟重试覆盖。若实现对 409 做任何自动重写，
// 第 2 次写请求会在此窗口冒出，断言 FAIL。
#[tokio::test]
async fn conflict_409_does_not_trigger_delayed_background_rewrite() {
    let fake = FakeControlPlane::start(
        409,
        br#"{"error":{"code":"version_conflict","message":"stale version"}}"#,
    )
    .await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let _ = transport
        .round_trip(&revoke_grant_spec("7300000000000000999", Some(7)))
        .await
        .expect("409 往返成功交还");

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        fake.requests().await.len(),
        1,
        "409 后给延迟重试一个窗口，请求数仍须恒 1（无后台自动重写），实测 {}",
        fake.requests().await.len()
    );
}

// ════════════════════════════════════════════════════════════════════════════
// L-7 · 写端点 5xx 不本地补偿（如实交还失败状态；无本地补写 / 回滚 / 重试动作）
// ════════════════════════════════════════════════════════════════════════════

// §8 L-7（对 docs/examples/06 §4.2-14）：写端点返回 5xx → 传输层如实交还 status=500，
// 不假定部分生效、不本地补写 / 回滚 / 重试——Fake 侧只观测到一次请求、连接数恰 1。
// 钉"写失败即终止于交还"，杜绝传输层任何补偿动作（一切事务三联动都在 daemon）。
#[tokio::test]
async fn write_5xx_is_surfaced_as_is_without_local_compensation() {
    const FAILURE_ENVELOPE: &[u8] = br#"{"error":{"code":"internal","message":"write failed"}}"#;
    let fake = FakeControlPlane::start(500, FAILURE_ENVELOPE).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let resp = transport
        .round_trip(&elevate_spec("agent2", "redis-main", "destroy", "30m"))
        .await
        .expect("Fake 应答 500 是一次正常 HTTP 往返（失败语义由上层据状态呈现）");

    assert_eq!(
        resp.status, 500,
        "写端点 5xx 状态必须如实交还，不在传输层吞掉"
    );
    assert_eq!(
        resp.body, FAILURE_ENVELOPE,
        "写端点 5xx 失败信封必须原样交还 body（如实呈现失败、不本地补偿，L-7）——一个丢弃 / 截断 body 的 transport 在此必须 FAIL（仅断 status 无法证伪『body 被吞掉』）"
    );
    assert_eq!(
        fake.requests().await.len(),
        1,
        "5xx 后绝不本地重试 / 补写——Fake 必须只观测到 1 次请求（出现第 2 次即重试 / 补偿，违反 L-7）"
    );
    assert_eq!(
        fake.connection_count(),
        1,
        "5xx 不触发重连——连接数必须恒 1，实测 {}",
        fake.connection_count()
    );
}

// §8 L-7（差分守卫）：5xx 返回后再观察一小段时间，Fake 侧请求数仍恒 1——无后台延迟补写 /
// 回滚 / 重试。若实现把 5xx 当"可能部分生效"而发任何补偿请求，第 2 次请求会冒出，断言 FAIL。
#[tokio::test]
async fn write_5xx_does_not_trigger_delayed_compensation_request() {
    let fake =
        FakeControlPlane::start(500, br#"{"error":{"code":"internal","message":"boom"}}"#).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let _ = transport
        .round_trip(&revoke_grant_spec("7300000000000000123", Some(3)))
        .await
        .expect("500 往返成功交还");

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        fake.requests().await.len(),
        1,
        "5xx 后给补偿动作一个暴露窗口，请求数仍须恒 1（无本地补写 / 回滚 / 重试），实测 {}",
        fake.requests().await.len()
    );
}

// ════════════════════════════════════════════════════════════════════════════
// L-11 · 零本地状态（两条顺序读命令行为一致；命令间 / 命令后磁盘无 CLI 落地状态文件）
// 注：L-11 的"删 CLI 工作 / 缓存目录后第二条仍一致"在此以"transport 不持任何跨命令本地
// 状态、第二条读命令独立成立"覆盖；磁盘残留断言对一个空工作目录做"命令前后文件集不变"核对。
// ════════════════════════════════════════════════════════════════════════════

// §8 L-11：对同一 Fake 顺序跑两条读命令，二者各自独立成立（同输入 → 同响应交还），
// 且第二条不依赖第一条落地的任何本地状态——transport 无跨命令缓存。钉"命令无状态"。
#[tokio::test]
async fn two_sequential_read_commands_behave_identically() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let first = transport
        .round_trip(&audit_spec(Some("agent3"), None, Some(1), Some(20)))
        .await
        .expect("第一条读命令成功");
    let second = transport
        .round_trip(&audit_spec(Some("agent3"), None, Some(1), Some(20)))
        .await
        .expect("第二条读命令成功");

    assert_eq!(
        first, second,
        "同输入两条顺序读命令必须交还完全一致的响应（命令无状态、不依赖上次落地）"
    );
    // 各自独立一次往返：两条命令累计恰 2 次连接（无复用、无残留连接）。
    assert_eq!(
        fake.connection_count(),
        2,
        "两条独立读命令累计恰 2 次连接，实测 {}",
        fake.connection_count()
    );
}

// §8 L-11：一条命令往返前后，CLI **真实**工作 / 缓存落点内文件集**不变**——transport 不向
// 磁盘落地任何策略 / 凭据 / 审计 / 决策缓存文件（仅允许标准输出与显式 --format/export 目标，
// 本测试两者皆不涉及）。钉"命令结束磁盘无 CLI 写出的本地状态缓存文件"。
//
// 防空转（assertion-1/trace-1）：被测的目录必须是 transport 运行期间**真实**会落地缓存的位置，
// 否则"一个 transport 根本不认识的目录里没出现文件"无条件为真、对实现无约束力。故本测试在
// round_trip 期间把进程 **CWD** chdir 进受观测的 workdir，并把 `HOME` 与全套 `XDG_*`
// （CACHE/CONFIG/DATA/STATE）env 重定向到各自受观测的空目录——覆盖 CLI 缓存可能落地的所有
// 约定位置（`./`、`~/`、`~/.cache`、`~/.config`、`~/.local/share|state`、`$XDG_*`）。命令后断言
// **每一个**受观测目录仍为空：任一处出现文件即说明 transport 落地了本地状态缓存，L-11 不过。
// （CWD / env 是进程级全局，故用 ENV_LOCK 串行化并经 EnvGuard 在退出 / panic 时复原。）
#[tokio::test]
async fn round_trip_writes_no_local_state_file_to_working_dir() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;

    // 受观测的真实落点：CWD、HOME，以及全套 XDG 缓存 / 配置 / 数据 / 状态目录。
    let workdir = TempDir::new(); // 真实 CWD（`./` 相对落点）。
    let home = TempDir::new(); // 真实 HOME（`~/`、`~/.cache`、`~/.config`、`~/.local/...` 落点）。
    let xdg_cache = TempDir::new();
    let xdg_config = TempDir::new();
    let xdg_data = TempDir::new();
    let xdg_state = TempDir::new();
    let observed_dirs = [
        ("CWD", workdir.path()),
        ("HOME", home.path()),
        ("XDG_CACHE_HOME", xdg_cache.path()),
        ("XDG_CONFIG_HOME", xdg_config.path()),
        ("XDG_DATA_HOME", xdg_data.path()),
        ("XDG_STATE_HOME", xdg_state.path()),
    ];

    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    // 串行化 CWD/env 改动（进程级全局），并在作用域结束 / panic 时由 EnvGuard 复原。
    // 用 tokio 异步 Mutex（可安全跨 await 持有），独占整段"绑定 → 往返 → 观测"窗口。
    let _lock = ENV_LOCK.lock().await;
    let _guard = EnvGuard::bind(
        workdir.path(),
        home.path(),
        xdg_cache.path(),
        xdg_config.path(),
        xdg_data.path(),
        xdg_state.path(),
    );

    // 在 CWD/HOME/XDG 全部指向受观测空目录的状态下跑这一次往返——若 transport 把任何
    // decision / credential / audit 缓存写到 `./` 或 `~/.cache/postern/` 等约定位置，会落进某个受观测目录。
    let _ = transport
        .round_trip(&audit_spec(Some("agent3"), None, None, None))
        .await
        .expect("读命令往返成功");

    // 命令后每一个受观测落点仍须为空——任一处出现文件即 transport 落地了本地状态缓存，L-11 不过。
    for (label, dir) in observed_dirs {
        let after = list_dir(dir);
        assert!(
            after.is_empty(),
            "命令后受观测落点 {label}（{dir:?}）仍须为空——CLI 不得向其落地任何策略 / 凭据 / 审计 / 决策缓存文件，实得 {after:?}（L-11：CLI 落地任何本地状态缓存即不过）"
        );
    }
}

/// 进程级 CWD/env 改动的串行锁——`round_trip_writes_no_local_state_file_to_working_dir` 持它独占
/// 改 CWD/HOME/XDG，避免与并行测试相互踩踏。用 tokio 异步 Mutex，可安全跨 await 持有整段窗口。
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// 把进程 CWD / `HOME` / 全套 `XDG_*` 绑定到受观测目录的作用域守卫——`bind` 时记下原值并切换，
/// `Drop`（含 panic 退栈）时原样复原，确保测试副作用不外泄到其它测试 / 后续运行。
struct EnvGuard {
    prev_cwd: PathBuf,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn bind(
        cwd: &Path,
        home: &Path,
        xdg_cache: &Path,
        xdg_config: &Path,
        xdg_data: &Path,
        xdg_state: &Path,
    ) -> EnvGuard {
        let prev_cwd = std::env::current_dir().expect("读当前工作目录");
        let bindings = [
            ("HOME", home),
            ("XDG_CACHE_HOME", xdg_cache),
            ("XDG_CONFIG_HOME", xdg_config),
            ("XDG_DATA_HOME", xdg_data),
            ("XDG_STATE_HOME", xdg_state),
        ];
        let prev = bindings
            .iter()
            .map(|(key, value)| {
                let old = std::env::var_os(key);
                std::env::set_var(key, value);
                (*key, old)
            })
            .collect();
        std::env::set_current_dir(cwd).expect("chdir 进受观测工作目录");
        EnvGuard { prev_cwd, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // 先复原 CWD，再复原 env——任一失败不掩盖（best-effort 复原，测试进程随后退出）。
        let _ = std::env::set_current_dir(&self.prev_cwd);
        for (key, old) in &self.prev {
            match old {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

/// 列目录内文件名集合（排序后稳定比对）。
fn list_dir(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("读工作目录")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ════════════════════════════════════════════════════════════════════════════
// 端点方法一致性（请求侧补钉）：DELETE 乐观锁写端点的方法 / 路径忠实落到线上请求行。
// ════════════════════════════════════════════════════════════════════════════

// §8 F-2（请求侧）：revoke-grant <id> → DELETE /v1/grants/temp/{id}，{id}（>2^53 雪花 id）
// 原样落入线上请求行路径，不丢精度、不数值化。钉删除端点的方法 / 路径填充。
#[tokio::test]
async fn round_trip_sends_delete_with_snowflake_id_in_path() {
    let fake = FakeControlPlane::start(200, OK_SINGLE_BODY).await;
    let transport = UdsTransport::new(fake.socket_path.clone(), short_timeout());

    let spec = revoke_grant_spec("7300000000000000123", Some(3));
    assert_eq!(
        spec.method,
        Method::Delete,
        "前置：revoke-grant 方法为 DELETE"
    );

    let _ = transport.round_trip(&spec).await.expect("往返成功");

    let reqs = fake.requests().await;
    assert_eq!(reqs[0].method, "DELETE", "线上请求行方法必须 DELETE");
    assert_eq!(
        reqs[0].path, "/v1/grants/temp/7300000000000000123",
        "雪花 id 必须原样落入请求行路径（>2^53 不丢精度、不数值化）"
    );
}
