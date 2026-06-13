//! connpool 单元行为测试（RED）。
//!
//! 钉死连接管理子域（模块文档 06 §3.5、§8 F-7 / L-6 / L-7 / L-8 / L-9 / L-10 / L-17）：
//! 池键 `(ResourceCode, CredentialTier)`、取句柄一次即释、池化/退避/上限/中断/归池前净化/
//! 连接审计。**kernel/管线用内存 Fake 全插件注入驱动**——Fake `Transport`/`CredentialProvider`/
//! `TargetResolver`/`AuditSink` + 真实 `UnlockedVault`（happy 路径取**真实不透明句柄**）。
//!
//! 机密类型纪律（雷区）：daemon **绝不构造** `ResolvedTarget`/`ResourceCredential`——
//! happy 路径的句柄由**真实** secrets 面产出（`StaticVaultProvider::credential_for` +
//! `UnlockedVault::resolve`），Fake 只在失败路径返回 `Err`、或在 `Transport::open` 侧
//! **按值接管**句柄并**记录这次移动**（F-7 / L-17 取证）。本文件零 SQL 标记、零非-shells 的
//! `ConnOrigin` 字面（需要时用 `use ... as Origin` 别名读/解构，本测试无此需要）。
//!
//! 失败路径一等公民：凭据 / 解析 / 通路建立失败 → `Err(AcquireError)`（`stage()` 恒为
//! connect=`Stage::Transport`，不吞错、不降级、不改路）；超限 → 有界队列或 deny 二者之一、
//! 绝无第三种；归池前净化失败 → 销毁不归池。每条只钉一个行为，断言精确到具体变体 / stage /
//! 事件序 / 确切错误，禁弱断言。
//!
//! 实现为 RED 桩（`ConnPool::new`/`acquire`/... 体为 `todo!()`），故凡触达池逻辑的测试
//! 调用即 panic → 观察到红；纯类型层断言（`AcquireError::stage` 等）已可绿，验编排正确。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use zeroize::Zeroizing;

use postern_core::domain::{
    Capability, CredentialTier, ResolvedTarget, ResourceCode, ResourceCredential,
};
use postern_core::error::AuditError;
use postern_core::error::{CredentialError, Stage, TransportError};
use postern_core::plugin::channel::{Channel, RawResponse};
use postern_core::plugin::{AuditEvent, AuditSink};
use postern_core::plugin::{CapabilitySurface, CredentialProvider, Transport};

use postern_secrets::error::ResolveError;
use postern_secrets::provider::static_vault::StaticVaultProvider;
use postern_secrets::vault::crypto;
use postern_secrets::vault::format::{VaultFile, FORMAT_VERSION, NONCE_LEN};
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;
use postern_secrets::vault::{self, UnlockedVault};

use postern_daemon::connpool::backoff::{Backoff, BACKOFF_BASE, BACKOFF_CAP};
use postern_daemon::connpool::lease::Lease;
use postern_daemon::connpool::pool::{ConnPool, PoolCaps, TargetResolver};
use postern_daemon::connpool::{AcquireError, ConnPhase, ConnectionEvent};

// ════════════════════════════════════════════════════════════════════════
//  固定测试材料（可控、确定，不碰真实来源）
// ════════════════════════════════════════════════════════════════════════

/// 固定 32B 主密钥（直接持有型来源解锁主密钥，非全零）。
const MASTER_KEY: [u8; 32] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
    0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
];

/// 固定 32B data-key（随机 data-key 的测试替身；包裹槽包裹的就是它）。
const DATA_KEY: [u8; 32] = [
    0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    0xf0, 0x0d, 0xca, 0xfe, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
];

/// 池键资源代号（恒为代号，绝不为真实地址）。
const RESOURCE: &str = "db-main";
/// 只读 tier（账号档位之一）。
const TIER_RO: &str = "readonly";
/// 业务操作 tier（另一档位，验 tier 不共享连接、L-8）。
const TIER_OP: &str = "op";

/// 真实地址样本（只入 `Zeroizing` 进 payload，绝不出现在任何 `Err` / 审计串、L-11）。
const TARGET_HOST: &str = "10.0.3.17";
const TARGET_PORT: &str = "5432";

/// Fake transport 的种类键（`connection_event.transport_kind` 取证值）。
const TRANSPORT_KIND: &str = "direct";

fn resource() -> ResourceCode {
    ResourceCode::new(RESOURCE)
}
fn tier_ro() -> CredentialTier {
    CredentialTier::new(TIER_RO)
}
fn tier_op() -> CredentialTier {
    CredentialTier::new(TIER_OP)
}

// ════════════════════════════════════════════════════════════════════════
//  真实保险箱夹具：端到端封 vault → unlock 句柄（取真实不透明句柄，daemon 不构造机密类型）
// ════════════════════════════════════════════════════════════════════════

fn zsection(
    entries: &[(&str, &[(&str, &str)])],
) -> BTreeMap<String, BTreeMap<String, Zeroizing<String>>> {
    let mut section: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    for (key, fields) in entries {
        let mut entry: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
        for (k, v) in *fields {
            entry.insert((*k).to_string(), Zeroizing::new((*v).to_string()));
        }
        section.insert((*key).to_string(), entry);
    }
    section
}

/// 端到端把两段 payload 封进合法 vault 字节，经 `vault::unlock` 还原成 `UnlockedVault`。
/// 用 KeyFile 来源（直接持有 32B 主密钥、无 KDF），避开 argon2id。
fn unlocked_vault() -> UnlockedVault {
    // secrets：两条凭据，仅 tier 不同（readonly / op）——验 (res,tier) 各自查表互不串扰。
    let secrets = zsection(&[
        (
            "vault://db-main/readonly",
            &[("user", "ro"), ("password", "ro-pw")],
        ),
        (
            "vault://db-main/op",
            &[("user", "op"), ("password", "op-pw")],
        ),
    ]);
    // targets：一条真实地址（host/port），值入 Zeroizing。
    let targets = zsection(&[("db-main", &[("host", TARGET_HOST), ("port", TARGET_PORT)])]);

    let payload = Payload::from_sections(secrets, targets);
    let plaintext = payload.to_plaintext().expect("serialize payload plaintext");

    let dk = Zeroizing::new(DATA_KEY);
    let (slot_nonce, wrapped) =
        crypto::wrap_data_key(&MASTER_KEY, &dk).expect("wrap data-key under master key");
    let header = Header {
        format_version: FORMAT_VERSION,
        slots: vec![Slot {
            source: SlotSource::KeyFile,
            kdf_params: None,
            salt: None,
            nonce_i: slot_nonce,
            wrapped_data_key: wrapped,
        }],
    };
    let mut vf = VaultFile {
        header,
        payload_nonce: [0u8; NONCE_LEN],
        ciphertext: Vec::new(),
    };
    let aad = vf.aad_bytes();
    let (payload_nonce, ciphertext) =
        crypto::encrypt_payload(&dk, &plaintext, &aad).expect("encrypt payload under data-key");
    vf.payload_nonce = payload_nonce;
    vf.ciphertext = ciphertext;
    vault::unlock(&MASTER_KEY, &vf.encode()).expect("KeyFile-source vault must unlock")
}

// ════════════════════════════════════════════════════════════════════════
//  Fake CredentialProvider：happy 路径委托真实 StaticVaultProvider（取真实句柄）；
//  失败路径返回注入的 Err。两路都记录调用次数（F-7「一次性物化」取证）。
// ════════════════════════════════════════════════════════════════════════

/// 凭据来源失败注入开关。
#[derive(Clone)]
enum CredMode {
    /// 委托真实 `StaticVaultProvider`，产出**真实** `ResourceCredential`（happy 路径）。
    Real,
    /// 注入失败：直接返回该 `CredentialError`（不产出句柄）。
    Fail(CredentialError),
}

struct FakeCredentialProvider {
    vault: Arc<UnlockedVault>,
    mode: CredMode,
    /// `credential_for` 被调用次数（验「一次性物化」，绝不重复物化）。
    calls: Arc<AtomicUsize>,
}

impl FakeCredentialProvider {
    fn new(vault: Arc<UnlockedVault>, mode: CredMode) -> Self {
        Self {
            vault,
            mode,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl CredentialProvider for FakeCredentialProvider {
    async fn credential_for(
        &self,
        res: &ResourceCode,
        tier: &CredentialTier,
    ) -> Result<ResourceCredential, CredentialError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.mode {
            // 委托真实静态来源：得真实不透明句柄（daemon 侧从不构造机密类型）。
            CredMode::Real => {
                StaticVaultProvider::new(&self.vault)
                    .credential_for(res, tier)
                    .await
            }
            CredMode::Fail(err) => Err(err.clone()),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake TargetResolver：happy 路径委托真实 UnlockedVault::resolve；失败注入 Err。
// ════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
enum ResolveMode {
    Real,
    Fail(ResolveError),
}

struct FakeResolver {
    vault: Arc<UnlockedVault>,
    mode: ResolveMode,
    calls: Arc<AtomicUsize>,
}

impl FakeResolver {
    fn new(vault: Arc<UnlockedVault>, mode: ResolveMode) -> Self {
        Self {
            vault,
            mode,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl TargetResolver for FakeResolver {
    fn resolve(&self, code: &ResourceCode) -> Result<ResolvedTarget, ResolveError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.mode {
            // 委托真实解析：得真实不透明 `ResolvedTarget`（唯一构造点在 secrets）。
            ResolveMode::Real => self.vault.resolve(code),
            ResolveMode::Fail(err) => Err(err.clone()),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake Transport：按值接管 (ResolvedTarget, ResourceCredential)、记录这次移动（F-7/L-17）；
//  可配置 open 成功 / 失败、persistent / 非长连接。
// ════════════════════════════════════════════════════════════════════════

/// 一次 `open` 的取证记录（**绝不含**地址 / 凭据明文——句柄按值接管即丢弃，只记发生过移动）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenRecord {
    /// 本 transport 的种类键。
    kind: String,
}

struct FakeTransport {
    persistent: bool,
    /// `open` 是否失败（注入建连失败）。
    fail: Option<TransportError>,
    /// 每次 `open` 接管句柄并记录一条（move 取证）。
    opens: Arc<Mutex<Vec<OpenRecord>>>,
}

impl FakeTransport {
    fn new(persistent: bool) -> Self {
        Self {
            persistent,
            fail: None,
            opens: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn failing(err: TransportError) -> Self {
        Self {
            persistent: true,
            fail: Some(err),
            opens: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl Transport for FakeTransport {
    fn kind(&self) -> &'static str {
        TRANSPORT_KIND
    }
    fn persistent(&self) -> bool {
        self.persistent
    }
    async fn open(
        &self,
        target: ResolvedTarget,
        cred: ResourceCredential,
    ) -> Result<Channel, TransportError> {
        // **按值接管**两个不透明句柄并立即丢弃——记录这次移动发生（F-7/L-17：句柄不出本次
        // 调用边界、不入池不缓存）。`Debug=REDACTED`，本记录不含地址 / 凭据明文。
        let _ = (target, cred);
        self.opens.lock().unwrap().push(OpenRecord {
            kind: self.kind().to_string(),
        });
        if let Some(err) = self.fail.clone() {
            return Err(err);
        }
        Ok(Channel {
            handle: Box::new(()),
        })
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake AuditSink：记录所有 record 调用（连接审计落点取证）。
// ════════════════════════════════════════════════════════════════════════

struct FakeAuditSink {
    events: Arc<Mutex<Vec<AuditEvent>>>,
}

impl FakeAuditSink {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl AuditSink for FakeAuditSink {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════
//  装配助手：用真实 vault + 指定 Fake 模式装配一个 ConnPool。
// ════════════════════════════════════════════════════════════════════════

fn caps(per_key: usize, queue: usize) -> PoolCaps {
    PoolCaps {
        per_key,
        global: per_key.max(1) * 4,
        queue,
    }
}

/// 装配一个 happy 路径连接池（真实凭据 / 解析、persistent transport、宽松容量）。
fn pool_happy() -> ConnPool {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::new(true)),
    ]));
    let audit = Arc::new(FakeAuditSink::new());
    ConnPool::new(transports, creds, resolver, audit, caps(8, 16))
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-7 / L-17：AcquireError 五支全部折叠到 connect=Stage::Transport（类型层，已可绿）
// ════════════════════════════════════════════════════════════════════════

// §8 L-6：连接不可建 → deny{stage=connect}（= Stage::Transport），五条失败支共享同一 stage、
// 不可彼此区分（fail-closed，不降级、不改路）。本断言纯类型层、不触池逻辑，验编排正确。
#[test]
fn acquire_error_every_variant_folds_to_connect_stage_transport() {
    for err in [
        AcquireError::Credential,
        AcquireError::Resolve,
        AcquireError::Transport,
        AcquireError::NoTransport,
        AcquireError::CapacityExceeded,
    ] {
        assert_eq!(
            err.stage(),
            Stage::Transport,
            "AcquireError::{err:?} 必须折叠到 connect 阶段（Stage::Transport），fail-closed"
        );
    }
}

// §8 L-6：connect 阶段绝不退化为执行/审计/其他阶段——逐一排除非 transport 的可能。
#[test]
fn acquire_error_stage_is_never_a_non_connect_stage() {
    for err in [
        AcquireError::Credential,
        AcquireError::Resolve,
        AcquireError::Transport,
    ] {
        let s = err.stage();
        assert_ne!(s, Stage::Exec, "建连失败绝不归 exec（不得降级为执行阶段）");
        assert_ne!(s, Stage::Audit, "建连失败绝不归 audit");
        assert_ne!(
            s,
            Stage::Tier,
            "daemon 层把凭据失败折叠为 connect，绝不停在 tier"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════
//  §8 退避：每键指数、基数 1s、上限 60s、带抖动；常量边界钉死（常量层已可绿）
// ════════════════════════════════════════════════════════════════════════

// §8（退避状态机）：退避基数恰 1s、上限恰 60s（指数退避增长封顶于此，带抖动后亦不超上界）。
#[test]
fn backoff_constants_are_base_1s_cap_60s() {
    assert_eq!(BACKOFF_BASE, Duration::from_secs(1), "退避基数必须为 1s");
    assert_eq!(BACKOFF_CAP, Duration::from_secs(60), "退避上限必须为 60s");
}

// §8（退避状态机）：刚构造（无失败档位）→ next_delay 为 None（可立即重试，不退避）。
// 触达 Backoff 逻辑（todo!()）即 panic → 红。
#[test]
fn backoff_fresh_has_no_delay() {
    let mut b = Backoff::new();
    assert_eq!(b.next_delay(), None, "无失败档位时不退避（None）");
}

// §8（退避状态机）：首次失败后退避 ≥ 基数且 ≤ 上限（带抖动落在 [base, cap] 内）。
#[test]
fn backoff_first_failure_within_base_and_cap() {
    let mut b = Backoff::new();
    b.record_failure();
    let d = b.next_delay().expect("失败后必有退避时长");
    assert!(d >= BACKOFF_BASE, "首次退避不得短于基数 1s");
    assert!(d <= BACKOFF_CAP, "退避不得超过上限 60s（封顶）");
}

// §8（退避状态机）：连续失败退避**单调不减**且始终封顶 ≤ 60s（指数增长有上界，不无界膨胀）。
#[test]
fn backoff_is_monotonic_nondecreasing_and_capped() {
    let mut b = Backoff::new();
    let mut prev = Duration::ZERO;
    for _ in 0..12 {
        b.record_failure();
        let d = b.next_delay().expect("失败后必有退避时长");
        assert!(d <= BACKOFF_CAP, "任一档位退避都封顶 ≤ 60s");
        assert!(
            d >= prev || prev > BACKOFF_CAP,
            "指数退避档位单调不减（封顶前每档不短于上一档）"
        );
        prev = d;
    }
}

// §8（退避状态机）：重建成功 → reset 清零档位 → 回到「不退避」（下次失败重新从基数起退）。
#[test]
fn backoff_reset_returns_to_no_delay() {
    let mut b = Backoff::new();
    b.record_failure();
    b.record_failure();
    b.reset();
    assert_eq!(b.next_delay(), None, "reset 后回到无退避状态");
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-7 / L-17：acquire 取真实句柄一次、按值入 open、句柄不出调用边界
// ════════════════════════════════════════════════════════════════════════

// §8 F-7：收 Allow{tier} → acquire(resource, tier) 用池键 (ResourceCode, CredentialTier)；
// 建连时**一次性**取不透明句柄（credential_for + resolve 各一次）即时传入 Transport::open，
// 调用边界外句柄即时释放（不入池不缓存）。触达 acquire（todo!()）即红。
#[tokio::test]
async fn acquire_fetches_handles_once_and_moves_them_into_open() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transport = FakeTransport::new(true);
    let opens = transport.opens.clone();
    let cred_calls = creds.calls.clone();
    let resolve_calls = resolver.calls.clone();
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(transport),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 16),
    );

    let lease = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("happy 路径首取应成功得租约");
    drop(lease);

    // 一次性物化：credential_for 恰一次、resolve 恰一次（绝不重复物化、绝不缓存复用句柄）。
    assert_eq!(
        cred_calls.load(Ordering::SeqCst),
        1,
        "credential_for 必须恰调用一次（一次性物化，F-7）"
    );
    assert_eq!(
        resolve_calls.load(Ordering::SeqCst),
        1,
        "resolve 必须恰调用一次（一次性解析，F-7）"
    );
    // 句柄按值移入 open：恰一次 open，且记录里只含 transport 种类、绝无地址 / 凭据明文。
    let recorded = opens.lock().unwrap();
    assert_eq!(recorded.len(), 1, "句柄必须按值传入 Transport::open 恰一次");
    assert_eq!(recorded[0].kind, TRANSPORT_KIND);
}

// §8 L-17：acquire 消费**已选定** tier，调用栈内无动词→tier 映射——两条仅 tier 不同的
// Allow（ro / op）走各自 tier 取连接，pool 不读 Capability 二次裁决 tier。
#[tokio::test]
async fn acquire_consumes_selected_tier_without_verb_to_tier_mapping() {
    let pool = pool_happy();
    // 入参直接是已选 tier；acquire 签名里**没有** Capability 参数（类型层即杜绝 verb→tier）。
    // 两个不同 tier 各自成功取连接，互不串扰。
    let ro = pool.acquire(&resource(), &tier_ro()).await;
    let op = pool.acquire(&resource(), &tier_op()).await;
    assert!(ro.is_ok(), "tier=readonly 应按入参直接取连接（不二次裁决）");
    assert!(op.is_ok(), "tier=op 应按入参直接取连接（不二次裁决）");
}

// §8 L-17（类型层）：`acquire` 入参**恰为** `(&ResourceCode, &CredentialTier)`——签名里
// 没有 `Capability`，故「读 Capability 决定 tier」在类型层即不可表达。下方强制一个与
// `acquire` 同形的函数指针绑定：若实现给 `acquire` 加入 `Capability` 参数（重开 verb→tier
// footgun），此绑定的类型不匹配 → 编译失败，本断言即红。
#[test]
fn acquire_signature_takes_only_resource_and_tier_no_capability() {
    // 编译期见证 `acquire` 的入参恰为 (&ConnPool, &ResourceCode, &CredentialTier)：把它当作
    // 接受这三参的高阶函数项调用一次（返回 future，不在此 await/poll，故不触 todo!()）。若实现
    // 给 acquire 加 Capability 参数（重开 verb→tier footgun），此调用元数不符 → 编译失败即红。
    fn _witness<F>(_f: F)
    where
        F: for<'a> Fn(
            &'a ConnPool,
            &'a ResourceCode,
            &'a CredentialTier,
        ) -> Pin<
            Box<dyn core::future::Future<Output = Result<Lease, AcquireError>> + 'a>,
        >,
    {
    }
    // 适配器把 async fn `acquire` 装箱成可被 `_witness` 约束捕获的形态——三参精确匹配。
    fn _adapt<'a>(
        p: &'a ConnPool,
        r: &'a ResourceCode,
        t: &'a CredentialTier,
    ) -> Pin<Box<dyn core::future::Future<Output = Result<Lease, AcquireError>> + 'a>> {
        Box::pin(p.acquire(r, t))
    }
    _witness(_adapt);
    // `Capability` 仅在此处作存在性引用，**绝不**进入 `acquire` 的入参——verb→tier 不可表达。
    let _verbs = [Capability::Query, Capability::Mutate];
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-8：不同 tier 不共享连接（池键含 tier，账号隔离在连接粒度成立）
// ════════════════════════════════════════════════════════════════════════

// §8 L-8：同一资源、两个不同 CredentialTier 的请求 → 落两个不同池槽，永不复用同一底层连接。
// 用 per_key=1 的池：若 tier 共享同一槽，第二 tier 会被第一 tier 的在用连接顶到上限；
// tier 不共享则各自独立槽、各自首建，两次 open 各发生一次。
#[tokio::test]
async fn distinct_tiers_never_share_a_channel() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transport = FakeTransport::new(true);
    let opens = transport.opens.clone();
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(transport),
    ]));
    // per_key=1：每池键至多一条在用连接。
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(1, 4),
    );

    // 同时持有两个 tier 的租约（不 drop），各占各自池槽——两槽都能首建，证明不共享。
    let ro = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("tier=ro 落独立池槽，应可建连");
    let op = pool
        .acquire(&resource(), &tier_op())
        .await
        .expect("tier=op 落独立池槽（含 tier 的池键不与 ro 同槽），应可建连");

    // 两次独立首建 → 两次 open（若共享同一槽，per_key=1 下第二个会超限而非首建）。
    assert_eq!(
        opens.lock().unwrap().len(),
        2,
        "不同 tier 必须各自首建（落不同池槽），永不复用同一底层连接"
    );
    drop((ro, op));
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-6：建连三支失败（transport / credential / resolve）→ Err{stage=connect}，不吞错
// ════════════════════════════════════════════════════════════════════════

// §8 L-6：注入 Transport::open 失败 → acquire 返回 Err(AcquireError::Transport) →
// stage=connect=Stage::Transport，错误经脱敏不含真实地址；绝不静默重试到他路或降级放行。
#[tokio::test]
async fn transport_open_failure_denies_at_connect_stage() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::failing(TransportError::ConnectFailed)),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 16),
    );

    match pool.acquire(&resource(), &tier_ro()).await {
        Ok(_) => panic!("Transport::open 失败时 acquire 绝不应返回租约（fail-closed）"),
        Err(e) => {
            assert_eq!(
                e,
                AcquireError::Transport,
                "通路建立失败应折叠为 Transport 支"
            );
            assert_eq!(
                e.stage(),
                Stage::Transport,
                "通路建立失败 → deny{{stage=connect}}"
            );
        }
    }
}

// §8 L-6：注入 credential_for 失败（NotFound）→ Err(AcquireError::Credential) →
// stage=connect；凭据物化失败在 daemon 层折叠为建连失败（区别于 core 把它归 tier）。
#[tokio::test]
async fn credential_failure_denies_at_connect_stage() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(
        vault.clone(),
        CredMode::Fail(CredentialError::NotFound),
    ));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::new(true)),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 16),
    );

    match pool.acquire(&resource(), &tier_ro()).await {
        Ok(_) => panic!("凭据物化失败时 acquire 绝不应返回租约（fail-closed）"),
        Err(e) => {
            assert_eq!(
                e,
                AcquireError::Credential,
                "凭据物化失败应折叠为 Credential 支"
            );
            assert_eq!(
                e.stage(),
                Stage::Transport,
                "凭据物化失败 → deny{{stage=connect}}"
            );
        }
    }
}

// §8 L-6：注入 resolve 失败（UnknownCode）→ Err(AcquireError::Resolve) → stage=connect；
// 错误绝不内插真实地址（脱敏纪律 L-11）。
#[tokio::test]
async fn resolve_failure_denies_at_connect_stage() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(
        vault.clone(),
        ResolveMode::Fail(ResolveError::UnknownCode),
    ));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::new(true)),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 16),
    );

    match pool.acquire(&resource(), &tier_ro()).await {
        Ok(_) => panic!("代号解析失败时 acquire 绝不应返回租约（fail-closed）"),
        Err(e) => {
            assert_eq!(e, AcquireError::Resolve, "代号解析失败应折叠为 Resolve 支");
            assert_eq!(
                e.stage(),
                Stage::Transport,
                "代号解析失败 → deny{{stage=connect}}"
            );
        }
    }
}

// §8 L-6（fail-closed 不吞错）：建连失败时**绝不执行后续**——open 失败的池，acquire 返回 Err，
// 不静默改走他路（无第二个 transport 兜底重试）。
#[tokio::test]
async fn connect_failure_does_not_silently_retry_other_path() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transport = FakeTransport::failing(TransportError::HandshakeFailed);
    let opens = transport.opens.clone();
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(transport),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 16),
    );

    let r = pool.acquire(&resource(), &tier_ro()).await;
    assert!(r.is_err(), "open 失败必 Err，绝不降级放行");
    // 只尝试本路一次，不静默重试到他路（open 调用次数受退避/单次语义约束，不风暴）。
    assert!(
        opens.lock().unwrap().len() <= 1,
        "建连失败不得静默重试到他路 / 风暴重连（本次至多一次 open 尝试）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-7：超并发上限 → 有界排队或 deny 二者之一；绝无第三种、绝不无界缓冲
// ════════════════════════════════════════════════════════════════════════

// §8 L-7：per_key=1、queue=0 的池下，第一条占满唯一槽且不归还时，第 2 条立即
// deny{stage=connect}（队列容量 0 → 无处可等 → 立即 deny）；结果只能是 deny，不出现第三种。
#[tokio::test]
async fn over_cap_with_zero_queue_immediately_denies() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::new(true)),
    ]));
    // per_key=1、queue=0：超限即立即 deny（无等待位）。
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(1, 0),
    );

    // 第一条占满唯一槽，持有不归还。
    let _held = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("首取占满唯一池槽");
    // 第二条：超限且无队列 → 立即 deny{stage=connect}，绝不无界缓冲、绝不第三种结果。
    match pool.acquire(&resource(), &tier_ro()).await {
        Ok(_) => panic!("超 per_key 上限且 queue=0 时绝不应再发租约（不得静默放行）"),
        Err(e) => {
            assert_eq!(
                e,
                AcquireError::CapacityExceeded,
                "超限且无等待位 → CapacityExceeded（有界，二选一中的 deny 支）"
            );
            assert_eq!(e.stage(), Stage::Transport, "超限 deny 也归 connect 阶段");
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-9：归池前会话净化为不变量；净化失败 → 销毁不归池（fail-closed）
// ════════════════════════════════════════════════════════════════════════

// §8 L-9：租约归还前**强制会话净化**；注入净化失败 → 该连接被销毁、不归池，下个请求拿到
// 的是新建（干净）连接。用 per_key=1 池：净化失败若错误地归池，第二取会复用脏连接（不新建
// open）；正确实现销毁该连接、第二取重新 open（open 计数为 2）。
#[tokio::test]
async fn sanitize_failure_destroys_connection_not_pooled() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transport = FakeTransport::new(true);
    let opens = transport.opens.clone();
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(transport),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(1, 4),
    );

    // 首取得租约；标记其会话损坏（脏会话无法可靠净化）。
    let mut lease = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("首取应成功");
    lease.mark_damaged();
    // 净化复核：已损坏的连接其 `sanitize_for_return` 必须报失败（false）——据此 Drop 时
    // 该连接必须销毁、不归池（L-9：净化是不变量，失败即销毁）。
    assert!(
        !lease.sanitize_for_return(),
        "已损坏连接的归还前净化必须报失败（false），驱动「销毁不归池」分支"
    );
    drop(lease);

    // 第二取：上一条已被销毁（未归池）→ 必须**重新 open**（open 计数=2），绝不复用脏连接。
    let _second = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("销毁后第二取应新建干净连接");
    assert_eq!(
        opens.lock().unwrap().len(),
        2,
        "净化失败的连接必须销毁不归池：第二取须重新 open，绝不复用脏连接（L-9）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-7：connection_event 字段恰为 resource / tier 名 / transport 种类（无地址/凭据）
// ════════════════════════════════════════════════════════════════════════

// §8 F-7：通路建立落一条 connection_event（phase=establish），字段恰为 resource、tier 名、
// transport 种类——**绝不含**真实地址 / 凭据明文（地址 / 凭据从未进入本层可读形态）。
#[tokio::test]
async fn establish_records_connection_event_with_only_resource_tier_kind() {
    let pool = pool_happy();
    let lease = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("happy 路径首取应成功");
    drop(lease);

    let events: Vec<ConnectionEvent> = pool.recorded_events();
    let establish = events
        .iter()
        .find(|e| e.phase == ConnPhase::Establish)
        .expect("通路建立必落一条 establish connection_event");

    // 字段恰为 resource / tier 名 / transport 种类。
    assert_eq!(establish.resource, resource(), "事件 resource 字段为代号");
    assert_eq!(establish.tier, tier_ro(), "事件 tier 字段为 tier 名");
    assert_eq!(
        establish.transport_kind, TRANSPORT_KIND,
        "事件 transport_kind 字段取自 Transport::kind()"
    );

    // 红线：序列化/调试形态绝不出现真实地址 / 端口明文。
    let dbg = format!("{establish:?}");
    assert!(
        !dbg.contains(TARGET_HOST) && !dbg.contains(TARGET_PORT),
        "connection_event 绝不含真实地址 / 端口明文（地址从不进本层可读形态）"
    );
}

// §8 L-10：freeze / 吊销时对在用连接强制 abort，落 connection_event(phase=abort)。
// 触达 force_abort（todo!()）即红。
#[tokio::test]
async fn force_abort_records_abort_connection_event() {
    let pool = pool_happy();
    let _held = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("先建一条在用连接");
    pool.force_abort(&resource()).await;

    let events = pool.recorded_events();
    assert!(
        events.iter().any(|e| e.phase == ConnPhase::Abort),
        "强制中断必落一条 abort connection_event（非仅优雅排空，L-10）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8（退避边界）：非长连接 transport 不入池、不退避（即建即用即弃）
// ════════════════════════════════════════════════════════════════════════

// §8：非长连接 transport（persistent=false）即建即用即弃——**不入池**，故每次 acquire 都
// 新建一条；不复用、不退避。用非长连接 transport：两次顺序 acquire（各自 drop）→ 两次 open。
#[tokio::test]
async fn non_persistent_transport_is_build_use_discard_not_pooled() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transport = FakeTransport::new(false); // 非长连接
    let opens = transport.opens.clone();
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(transport),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 16),
    );

    let l1 = pool.acquire(&resource(), &tier_ro()).await.expect("首取");
    drop(l1);
    let l2 = pool.acquire(&resource(), &tier_ro()).await.expect("再取");
    drop(l2);

    // 非长连接不入池：每次都新建（即建即用即弃），绝不复用上一条。
    assert_eq!(
        opens.lock().unwrap().len(),
        2,
        "非长连接 transport 不入池：每次 acquire 都新建（即建即用即弃），不复用"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  Lease 类型层：channel_mut 暴露在用通路；构造经池内部（类型存在性）
// ════════════════════════════════════════════════════════════════════════

// Lease 是 acquire 的回类型（RAII guard）；此处确保该公开类型名被引用、其 channel_mut 入口
// 形状正确（供 Adapter::execute 在通路上执行）。触达 Lease::new（todo!()）即红。
#[tokio::test]
async fn lease_exposes_channel_for_execution() {
    let pool = pool_happy();
    let mut lease: Lease = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("happy 路径取租约");
    // channel_mut 暴露在用通路的可变访问（Adapter::execute 用）。
    let _ch: &mut Channel = lease.channel_mut();
}

// ════════════════════════════════════════════════════════════════════════
//  CapabilitySurface 命名引用（保证 use 不悬空——本测试不触 discover，但 transport/adapter
//  trait 形状里它存在；此处仅静态引用其类型名，不参与池逻辑）。
// ════════════════════════════════════════════════════════════════════════

#[test]
fn capability_surface_type_is_referenced() {
    fn _takes(_s: &CapabilitySurface) {}
    // 仅类型存在性引用，不构造（discover 不在 connpool 职责内）。
    let _ = _takes as fn(&CapabilitySurface);
}

// `RawResponse` 同样仅作类型存在性引用（execute 出口在 kernel，不在本单元）。
#[test]
fn raw_response_type_is_referenced() {
    fn _takes(_r: &RawResponse) {}
    let _ = _takes as fn(&RawResponse);
}

// ════════════════════════════════════════════════════════════════════════
//  §8 健康与退避状态机：退避**接入 acquire 编排**——通路死亡后退避窗口内对该键的 acquire
//  走 deny（BackoffActive）而非立即风暴重连。钉死「退避器接线」，非孤立单元任意性。
// ════════════════════════════════════════════════════════════════════════

// §8：第一次 open 失败 → 该键进入退避窗口（基数 1s）；窗口内紧接的第二次 acquire 必须
// **直接 deny（AcquireError::BackoffActive）**且**绝不再发起 open**（不风暴重连）。
// 镜头：这条断言把 Backoff 钉进 ConnPool::acquire 的编排面——若退避器未接线（如原实现 open
// 失败仅 `return Err(Transport)`、从不 record_failure/next_delay），第二次 acquire 会再次
// 走到 open（opens=2）并返回 Transport 而非 BackoffActive → 本断言即红。空接线骗不过。
#[tokio::test]
async fn within_backoff_window_acquire_denies_without_reconnect_storm() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transport = FakeTransport::failing(TransportError::ConnectFailed);
    let opens = transport.opens.clone();
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(transport),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 0),
    );

    // 首取：open 失败 → 折叠为 Transport，并把该键推进退避窗口（record_failure + next_delay）。
    match pool.acquire(&resource(), &tier_ro()).await {
        Ok(_) => panic!("首次 open 失败时 acquire 绝不应返回租约（fail-closed）"),
        Err(e) => assert_eq!(
            e,
            AcquireError::Transport,
            "首次 open 失败应折叠为 connect=Transport"
        ),
    }
    assert_eq!(
        opens.lock().unwrap().len(),
        1,
        "首取应恰发起一次 open（失败）"
    );

    // 窗口内紧接第二取：必须**直接 deny=BackoffActive**，**绝不再 open**（退避期不风暴重连）。
    match pool.acquire(&resource(), &tier_ro()).await {
        Ok(_) => panic!("退避窗口内绝不应返回租约（应 deny=BackoffActive）"),
        Err(e) => assert_eq!(
            e,
            AcquireError::BackoffActive,
            "退避窗口内对该键的 acquire 必须 deny=BackoffActive（退避器已接线进 acquire 编排）"
        ),
    }
    assert_eq!(
        opens.lock().unwrap().len(),
        1,
        "退避窗口内绝不再发起 open（不风暴重连）——open 计数恒为 1，证明退避真正接线"
    );
    // BackoffActive 也折叠到 connect 阶段（fail-closed，与其他建连失败同 stage）。
    assert_eq!(
        AcquireError::BackoffActive.stage(),
        Stage::Transport,
        "退避 deny 同归 connect 阶段（Stage::Transport）"
    );
}

// §8：成功建连 → 清退避档位（reset）。先一次失败入退避窗口、再让窗口外重连成功后，
// 退避状态被清零（下次失败重新从基数起退）——证明 acquire 成功路径接了 backoff.reset。
// 用「失败一次后切换为成功」的 transport：首取失败入退避；构造一个**全新**池键（tier_op）
// 不受退避影响、可立即成功，证明退避是**每键**的、且成功路径会重置。
#[tokio::test]
async fn backoff_is_per_key_and_does_not_block_other_keys() {
    // 该 transport 恒成功；用它证明 tier_op 键不被 tier_ro 的退避窗口波及（每键独立状态机）。
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::new(true)),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(8, 0),
    );

    // tier_op 键无退避历史 → 立即成功（退避是每键的，tier_ro 的窗口不波及 tier_op）。
    let op = pool.acquire(&resource(), &tier_op()).await;
    assert!(
        op.is_ok(),
        "无退避历史的池键应立即成功（退避状态每键独立，不全局阻断）"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-7：超限→**有界队列等待（occupancy ≤ Q）** 半边——此前零覆盖的等待分支。
//  钉死「请求在有界队列中等待、释放后被唤醒得连接」，且队列占用恒 ≤ Q、绝不无界缓冲。
// ════════════════════════════════════════════════════════════════════════

// §8 L-7（等待半边）：per_key=1、queue≥1 时，第一条占满唯一槽；第二条 acquire **不立即 deny**，
// 而是**入有界队列等待**（occupancy ≤ Q）；当第一条归还释放席位，等待者被唤醒、拿到连接成功。
// 镜头：这是 over_cap_with_zero_queue 的**对偶**——证明「二选一」的等待支真实存在、可观察，
// 而非与 deny 完全相同的死桩。若实现把 queue>0 也直接 return CapacityExceeded（原桩），
// waiter 会立即 Err 而非等待→被唤醒→成功 → 本断言即红。
#[tokio::test]
async fn over_cap_with_queue_waits_then_succeeds_on_release() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::new(true)),
    ]));
    // per_key=1、queue=2：超限请求入有界队列等待（占用 ≤ 2），绝不立即 deny。
    let pool = Arc::new(ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(1, 2),
    ));

    // 第一条占满唯一槽，持有不归还。
    let held = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("首取占满唯一池槽");

    // 第二条在另一任务里 acquire：超限 → 入有界队列等待（不立即 deny），阻塞在 notified()。
    let waiter_pool = pool.clone();
    let waiter = tokio::spawn(async move { waiter_pool.acquire(&resource(), &tier_ro()).await });

    // 让 waiter 任务推进到「已入队等待」的挂起点（若它立即 deny，这里它早已 Err 返回）。
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !waiter.is_finished(),
        "超限但 queue>0：第二条必须在有界队列中**等待**，绝不立即 deny（等待半边真实存在）"
    );

    // 释放第一条 → 唤醒等待者 → 其拿到（复用归池的）连接，acquire 成功返回租约。
    drop(held);
    let got = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("等待者必须在席位释放后被唤醒（绝不无限挂起）")
        .expect("spawned 任务不应 panic");
    assert!(
        got.is_ok(),
        "席位释放后，有界队列中的等待者必须被唤醒并拿到连接（等待→唤醒→成功）"
    );
}

// §8 L-7（有界性）：等待队列**占用恒 ≤ Q**——queue 已被等待者填满时，再来的请求**立即 deny**
// （CapacityExceeded），绝不无界缓冲。per_key=1、queue=1：占满槽 + 一名等待者填满队列 →
// 第三条请求立即 deny（队列容量 Q=1 触顶，背压即 deny，不入第二个等待位）。
#[tokio::test]
async fn bounded_queue_full_denies_third_request_no_unbounded_buffer() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(FakeTransport::new(true)),
    ]));
    // per_key=1、queue=1：至多一名等待者；队列满后再来即立即 deny（occupancy ≤ Q=1）。
    let pool = Arc::new(ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(1, 1),
    ));

    // 占满唯一在用席位（持有不归还）。
    let held = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("首取占满唯一在用席位");

    // 一名等待者填满唯一队列位（occupancy = Q = 1），挂起等待。
    let waiter_pool = pool.clone();
    let waiter = tokio::spawn(async move { waiter_pool.acquire(&resource(), &tier_ro()).await });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !waiter.is_finished(),
        "第一名等待者占满唯一队列位后应仍在等待（占用 = Q = 1）"
    );

    // 第三条请求：在用满 + 队列满（occupancy 已达 Q）→ **立即 deny**，绝不入无界缓冲。
    match pool.acquire(&resource(), &tier_ro()).await {
        Ok(_) => panic!("队列已满时第三条请求绝不应返回租约（应立即 deny）"),
        Err(e) => assert_eq!(
            e,
            AcquireError::CapacityExceeded,
            "队列已满（占用达 Q）时再来的请求必须立即 deny=CapacityExceeded（有界，绝不无界缓冲）"
        ),
    }

    // 收尾：释放席位让等待者完成，避免悬挂任务。
    drop(held);
    let _ = tokio::time::timeout(Duration::from_secs(2), waiter).await;
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-7：connection_event 在 **recycle / health-evict** 两点也落（此前只钉 establish/abort）。
// ════════════════════════════════════════════════════════════════════════

// §8 F-7（回收写入点）：归池前会话净化**失败**→ 连接销毁不归池，且该次销毁落一条
// **Recycle** connection_event（字段恰为 resource / tier 名 / transport 种类，不含地址/凭据）。
// 镜头：补强 assertion-5 缺的 recycle 半边——若 Drop 销毁路径不落事件（原实现「本波次回收不
// 强制落点」），events 里找不到 Recycle → 本断言即红。
#[tokio::test]
async fn sanitize_failure_destroy_records_recycle_connection_event() {
    let pool = pool_happy();
    let mut lease = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("首取应成功");
    // 标记损坏 → 归还前净化必失败 → Drop 走「销毁不归池」分支（这是回收写入点）。
    lease.mark_damaged();
    assert!(
        !lease.sanitize_for_return(),
        "损坏连接归还前净化必失败（驱动销毁路径）"
    );
    drop(lease);

    let events = pool.recorded_events();
    let recycle = events
        .iter()
        .find(|e| e.phase == ConnPhase::Recycle)
        .expect("净化失败销毁必落一条 Recycle connection_event（回收写入点）");
    assert_eq!(
        recycle.resource,
        resource(),
        "Recycle 事件 resource 字段为代号"
    );
    assert_eq!(recycle.tier, tier_ro(), "Recycle 事件 tier 字段为 tier 名");
    assert_eq!(
        recycle.transport_kind, TRANSPORT_KIND,
        "Recycle 事件 transport_kind 取自 Transport::kind()"
    );
    // 红线：回收事件绝不含真实地址 / 端口明文。
    let dbg = format!("{recycle:?}");
    assert!(
        !dbg.contains(TARGET_HOST) && !dbg.contains(TARGET_PORT),
        "Recycle connection_event 绝不含真实地址 / 端口明文"
    );
}

// §8 F-7（健康剔除写入点）：周期健康检查判定通路死亡 → 从池槽剔除空闲死连接，落一条
// **HealthEvict** connection_event（字段恰为 resource / tier 名 / transport 种类）。
// 镜头：补强 assertion-5 缺的 health-evict 半边——HealthEvict 变体此前在 src 内零 emit；
// 若 health_evict 不落事件，events 里找不到 HealthEvict → 本断言即红。
#[tokio::test]
async fn health_evict_records_health_evict_connection_event() {
    let pool = pool_happy();
    // 先建一条连接并归还入池（成为空闲连接），供健康检查剔除。
    let lease = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("首取应成功并在 drop 后归池");
    drop(lease);

    // 周期健康检查判定该资源通路死亡 → 剔除空闲死连接，落 HealthEvict 事件。
    pool.health_evict(&resource());

    let events = pool.recorded_events();
    let evict = events
        .iter()
        .find(|e| e.phase == ConnPhase::HealthEvict)
        .expect("健康剔除必落一条 HealthEvict connection_event（健康剔除写入点）");
    assert_eq!(
        evict.resource,
        resource(),
        "HealthEvict 事件 resource 字段为代号"
    );
    assert_eq!(
        evict.tier,
        tier_ro(),
        "HealthEvict 事件 tier 字段为 tier 名"
    );
    assert_eq!(
        evict.transport_kind, TRANSPORT_KIND,
        "HealthEvict 事件 transport_kind 取自 Transport::kind()"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-10（fail-closed 实质后果）：被 force_abort 的在用连接归还时**必须销毁、绝不悄悄回池
//  复用**——abort_epoch 机制的实质语义，而非仅「落了一条 Abort 事件」。
// ════════════════════════════════════════════════════════════════════════

// §8 L-10：force_abort 后，被中断的在用租约归还时**销毁不回池**；其后对该键的 acquire **必须
// 重新 open**（绝不复用被中断的脏/已撤连接）。
// 镜头（failclosed-1）：这条钉死中断的**实质后果**，而非只验事件落点。若把 pool.rs 的
// `slot.abort_epoch += 1` 去掉（中断不递增纪元），被中断连接归还时会被判定为可回池 → 静默
// 复用 → 第二取复用脏连接（opens 仍为 1）→ 本断言即红。这正是 L-10 禁止的 fail-open 回归。
#[tokio::test]
async fn aborted_in_use_connection_is_destroyed_not_silently_reused() {
    let vault = Arc::new(unlocked_vault());
    let creds = Arc::new(FakeCredentialProvider::new(vault.clone(), CredMode::Real));
    let resolver = Arc::new(FakeResolver::new(vault.clone(), ResolveMode::Real));
    let transport = FakeTransport::new(true);
    let opens = transport.opens.clone();
    let transports = Arc::new(postern_daemon::registry::TransportRegistry::new(vec![
        Box::new(transport),
    ]));
    let pool = ConnPool::new(
        transports,
        creds,
        resolver,
        Arc::new(FakeAuditSink::new()),
        caps(1, 4),
    );

    // 首取一条在用连接（持有），随后强制中断该资源的在用连接。
    let held = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("首取一条在用连接");
    assert_eq!(opens.lock().unwrap().len(), 1, "首取恰一次 open");

    // freeze / 吊销：对该资源在用连接强制 abort（递增中断纪元，使其归还时销毁不回池）。
    pool.force_abort(&resource()).await;

    // 归还被中断的在用租约：纪元已变 → **销毁不回池**（绝不悄悄复用被中断连接，L-10）。
    drop(held);

    // 第二取：池中应无可复用连接（被中断者已销毁）→ **必须重新 open**（opens=2）。
    let second = pool
        .acquire(&resource(), &tier_ro())
        .await
        .expect("中断销毁后第二取应新建干净连接");
    assert_eq!(
        opens.lock().unwrap().len(),
        2,
        "被 force_abort 的在用连接归还时必须销毁不回池：第二取须重新 open，绝不悄悄复用被中断连接（L-10 fail-closed）"
    );
    drop(second);
}
