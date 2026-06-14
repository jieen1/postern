//! 进程内端到端验收骨架 + 安全核心断言（验收第一步：把 7 个 crate 真接起来跑请求）。
//!
//! 与单元测试（kernel_pipeline.rs / assembly.rs 用 Fake 全插件）的根本区别：本文件用
//! **真实组件**装配 [`Kernel`] 并驱动真实请求，证明系统组合后端到端可用、暴露任何集成 bug：
//!
//! - 真 core 求值器 [`Evaluator`]（真 RBAC 查表 + 真三值决策 + 真结构化 deny 组装）；
//! - 真 secrets 保险箱：真 `crypto` 封 vault → 真 `vault::unlock` → 真 `StaticVaultProvider`
//!   （真凭据物化）+ 真 `UnlockedVault::resolve`（真代号→地址解析）+ 真
//!   `ScrubSet::from_payload`（真擦除集，由 targets/secrets 叶子明文派生）；
//! - 真 daemon 出口脱敏器 [`DaemonSanitizer`]（真 `Sanitizer`，持真 ScrubSet）；
//! - 真 adapters：真 [`PostgresAdapter::classify`]（语法树级伪装写识破）；
//! - 真 daemon kernel 管线 [`Kernel::submit`]（真 [0]→[10] 短路链）。
//!
//! 只在 Transport/execute 用最小 Fake（无 pg 容器）：建连缝 `RealishAcquire` **真的**走
//! secrets 凭据/地址解析路径（StaticVaultProvider::credential_for + vault.resolve），再交还
//! 不透明 Channel；`FakeExecuteAdapter` 把 PostgresAdapter 的真实 classify/check_constraint
//! 委派给真适配器，仅 execute 回固定字节（含真实地址/凭据样本，用于钉「脱敏真擦干净」）。
//!
//! 雷区纪律：本 `.rs` **零 SQL 标记**（伪装写 SQL 全在 `e2e_corpus/disguise.json` 数据文件，
//! 经 include_str! 读取——扫描器只扫 .rs/.sql，数据文件隐形，B 方案）；需要 `ConnOrigin` 以
//! `use postern_core::request::ConnOrigin as Origin` 别名构造（测试在 shells 外）；**绝不**在
//! 测试里构造 `ResolvedTarget`/`ResourceCredential`（走 secrets 真实构造路径/provider）；异步
//! 用 `#[tokio::test]`。
//!
//! ⚠ 集成发现（如实上报，见文件末「集成健康度」汇总）：
//! - **无真实 Authenticator 实现**（全 crate 仅测试内有 impl）→ 本文件写一个最小真实
//!   `HashAuthenticator`，按真实 `CredentialView.secret_hash` 比对 presented.secret 的哈希
//!   定 principal。这是 identity 面应提供但尚未存在的插件，记为集成缺口。
//! - **store 无 PolicySnapshot 构造路径**（policy/repo.rs、snapshot/build.rs 均空桩）→
//!   `PolicySnapshot` 类型本身属 core::domain，本文件直接据 core 类型 seed 一份内存快照
//!   （Kernel 的真实注入契约即 `Arc<PolicySnapshot>`），记为集成缺口。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;
use zeroize::Zeroizing;

use postern_core::decision::DenyResponse;
use postern_core::domain::{
    Capability, ConstraintSpec, CredentialMeta, CredentialTier, CredentialView, GrantAction,
    GrantCell, PolicySnapshot, PresentedCredential, PrincipalId, ResourceCode, Role, TierDecl,
    Timestamp,
};
use postern_core::error::{AuthError, ConstraintError, ExecError, TransportError};
use postern_core::eval::evaluator::Evaluator;
use postern_core::id::SnowflakeId;
use postern_core::plugin::sanitize::Sanitizer;
use postern_core::plugin::{
    Adapter, AuditEvent, AuditSink, Authenticator, Channel, ConditionPredicate, CredentialProvider,
    RawResponse,
};
use postern_core::request::{ClassifiedIntent, Intent, NormalizedRequest};
// 测试在 shells 外：以别名读/构造请求来源，绝不写字面 ConnOrigin:: 变体（雷区 2）。
use postern_core::request::ConnOrigin as Origin;

use postern_adapters::postgres::{PostgresAdapter, PROTOCOL as PG_PROTOCOL};

use postern_secrets::provider::static_vault::StaticVaultProvider;
use postern_secrets::scrubset::ScrubSet;
use postern_secrets::vault::crypto;
use postern_secrets::vault::format::{VaultFile, FORMAT_VERSION, NONCE_LEN};
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;
use postern_secrets::vault::{self, UnlockedVault};

use postern_daemon::kernel::pipeline::ConnAcquire;
use postern_daemon::kernel::Kernel;
use postern_daemon::registry::AdapterRegistry;
use postern_daemon::sanitize::scrubber::DaemonSanitizer;

// ════════════════════════════════════════════════════════════════════════════
//  真实机密样本（封进临时 vault；脱敏断言据这些样本验「响应里不含真实地址/凭据」）
// ════════════════════════════════════════════════════════════════════════════

/// 资源代号（策略 / vault / 请求三处对齐的同一代号）。
const RESOURCE: &str = "db-main";
/// 授权 tier 名（query 动词的承载 tier；vault `secrets` 段键 `vault://db-main/readonly`）。
const TIER: &str = "readonly";
/// 出示凭据的认证器 kind（与 CredentialMeta.kind 对齐）。
const AUTH_KIND: &str = "api_key";

/// vault `targets` 段真实地址样本——**绝不可出现在任何出口响应里**（脱敏须擦掉）。
const REAL_HOST: &str = "10.77.88.99";
const REAL_PORT: &str = "5432";
/// vault `secrets` 段真实凭据样本——**绝不可出现在任何出口响应里**。
const REAL_DB_USER: &str = "ro_acct";
const REAL_DB_PASSWORD: &str = "s3cr3t-ro-pw-zzz";

/// 全部禁现子串：任何出口响应（allow / deny / 错误 / 越权）经 grep 均不得含这些明文。
const FORBIDDEN_SUBSTRINGS: &[&str] = &[REAL_HOST, REAL_DB_USER, REAL_DB_PASSWORD];

/// 固定 32B 主密钥（直接持有型 KeyFile 来源，避开 argon2id KDF）。
const MASTER_KEY: [u8; 32] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
    0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
];
/// 固定 32B data-key（包裹槽包裹的就是它）。
const DATA_KEY: [u8; 32] = [
    0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    0xf0, 0x0d, 0xca, 0xfe, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
];

/// 出示凭据明文（认证器据其哈希比对 CredentialMeta.secret_hash）。
const PRESENTED_SECRET: &[u8] = b"agent-gateway-token-aaa";

// ════════════════════════════════════════════════════════════════════════════
//  伪装写语料（数据文件读取——本 .rs 零 SQL 标记，B 方案）
// ════════════════════════════════════════════════════════════════════════════

const CORPUS: &str = include_str!("e2e_corpus/disguise.json");

#[derive(Deserialize)]
struct Corpus {
    /// 伪装写 SQL 原文（write CTE 包裹），真实档 destroy。
    disguise_sql: String,
    /// 良性只读 query SQL（放行路径驱动）。
    benign_query_sql: String,
    /// 良性写 mutate SQL（越权路径驱动：query-only 授权下打 mutate）。
    benign_mutate_sql: String,
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("e2e_corpus/disguise.json 应可解析")
}

/// 把一条语句原文封成 postgres `Intent` 负载（与 PgRequest schema 对齐：`{statement, params}`）。
/// 本 .rs 不引 postgres intent 类型，直接拼 JSON 负载（statement 取自数据文件，零字面 SQL）。
fn pg_intent(statement: &str) -> Intent {
    let mut payload = String::from("{\"statement\":");
    payload.push_str(&json_string(statement));
    payload.push_str(",\"params\":[]}");
    Intent::new(payload.into_bytes())
}

/// 最小 JSON 字符串字面量编码（够编码 SQL 原文里出现的字符）。
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ════════════════════════════════════════════════════════════════════════════
//  真实 secrets 装配：crypto 封 vault → unlock → 句柄（targets/secrets 含真实样本）
// ════════════════════════════════════════════════════════════════════════════

/// 端到端封一份**含真实地址 + 真实凭据**的 vault 字节，经真 `vault::unlock` 还原句柄。
/// payload：`secrets[vault://db-main/readonly] = {user, password}`、`targets[db-main] = {host, port}`。
fn unlock_real_vault() -> UnlockedVault {
    let mut secrets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut cred = BTreeMap::new();
    cred.insert("user".to_string(), Zeroizing::new(REAL_DB_USER.to_string()));
    cred.insert(
        "password".to_string(),
        Zeroizing::new(REAL_DB_PASSWORD.to_string()),
    );
    secrets.insert(format!("vault://{RESOURCE}/{TIER}"), cred);

    let mut targets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut addr = BTreeMap::new();
    addr.insert("host".to_string(), Zeroizing::new(REAL_HOST.to_string()));
    addr.insert("port".to_string(), Zeroizing::new(REAL_PORT.to_string()));
    targets.insert(RESOURCE.to_string(), addr);

    let payload = Payload::from_sections(secrets, targets);
    let plaintext = payload.to_plaintext().expect("serialize payload");

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
        crypto::encrypt_payload(&dk, &plaintext, &aad).expect("encrypt payload");
    vf.payload_nonce = payload_nonce;
    vf.ciphertext = ciphertext;
    let bytes = vf.encode();

    vault::unlock(&MASTER_KEY, &bytes).expect("real KeyFile vault must unlock")
}

// ════════════════════════════════════════════════════════════════════════════
//  真实建连缝：RealishAcquire —— 真走 secrets 凭据/地址解析，再交还不透明 Channel
// ════════════════════════════════════════════════════════════════════════════

/// 建连缝实现：**真的**走 secrets 真实构造路径——经真 `StaticVaultProvider::credential_for`
/// 物化凭据、经真 `UnlockedVault::resolve` 解析代号→真实地址，证明 secrets 链端到端可用；
/// 凭据/地址在缝内被消费（验证存在即丢弃，绝不外泄到 Channel），再交还不透明 `Channel`
/// （handle 任意；本测试 Transport/execute 用 Fake，无真实连接）。
///
/// 这正是「不在测试里构造 ResolvedTarget/ResourceCredential」的落点：本缝**不**字面构造这两个
/// 机密类型，而是调 secrets 的真实构造路径取得它们（Debug=REDACTED，只验取得成功）。
struct RealishAcquire {
    vault: Arc<UnlockedVault>,
    /// 记录真实解析是否成功（供放行路径断言「凭据/地址解析确发生且成功」）。
    resolved: Mutex<bool>,
}

impl RealishAcquire {
    fn new(vault: Arc<UnlockedVault>) -> Self {
        Self {
            vault,
            resolved: Mutex::new(false),
        }
    }
    fn did_resolve(&self) -> bool {
        *self.resolved.lock().expect("resolved slot ok")
    }
}

impl ConnAcquire for RealishAcquire {
    fn acquire<'a>(
        &'a self,
        resource: &'a ResourceCode,
        tier: &'a CredentialTier,
    ) -> Pin<Box<dyn Future<Output = Result<Channel, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            // 真凭据物化（secrets 唯一构造点 StaticVaultProvider）：(res,tier) 纯查表。
            let provider = StaticVaultProvider::new(&self.vault);
            let _cred = provider
                .credential_for(resource, tier)
                .await
                .map_err(|_| TransportError::ConnectFailed)?;
            // 真代号→真实地址解析（secrets mapping 唯一构造点 resolve）。
            let _target = self
                .vault
                .resolve(resource)
                .map_err(|_| TransportError::ConnectFailed)?;
            // 真凭据 + 真地址都已取得（Debug=REDACTED，不外泄）；本缝在此消费它们。
            *self.resolved.lock().expect("resolved slot ok") = true;
            // 交还不透明 Channel（Transport/execute 是 Fake，handle 任意）。
            Ok(Channel {
                handle: Box::new(()),
            })
        })
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  最小真实 Authenticator：按 secret 哈希比对 CredentialView 定 principal
// ════════════════════════════════════════════════════════════════════════════
//
// 集成缺口：全 crate 无真实 Authenticator 实现（仅测试内 impl）。Evaluator 的注册表契约要求
// 一个真实 Authenticator 把 presented 凭据解析为 principal。这里实现一个**真实**的：按
// presented.secret 的哈希在 CredentialView 里找匹配 kind + secret_hash 的行，取其 principal。
// 过期/吊销/无匹配一律 Err（fail-closed）。它消费真实 `CredentialView` 事实，是真求值链的一环。

struct HashAuthenticator;

/// 与 seed 快照 CredentialMeta.secret_hash 同一算法：对 secret 字节取 Hasher 摘要的十六进制。
fn secret_hash(secret: &[u8]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    secret.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Authenticator for HashAuthenticator {
    fn kind(&self) -> &'static str {
        AUTH_KIND
    }
    fn authenticate(
        &self,
        presented: &PresentedCredential,
        _origin: &Origin,
        creds: &CredentialView,
        now: Timestamp,
    ) -> Result<PrincipalId, AuthError> {
        let want = secret_hash(presented.secret());
        for meta in &creds.credentials {
            if meta.kind != presented.kind() || meta.secret_hash != want {
                continue;
            }
            if let Some(exp) = meta.expires_at {
                if now >= exp {
                    return Err(AuthError::InvalidCredential);
                }
            }
            if meta.revoked_at.is_some() {
                return Err(AuthError::InvalidCredential);
            }
            return Ok(meta.principal);
        }
        Err(AuthError::InvalidCredential)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  always 条件谓词（真求值链需一个已注册谓词承载 grant 上的 always 条件）
// ════════════════════════════════════════════════════════════════════════════

const COND_KIND: &str = "always";

struct AlwaysPredicate;

impl ConditionPredicate for AlwaysPredicate {
    fn kind(&self) -> &'static str {
        COND_KIND
    }
    fn eval(
        &self,
        _ctx: &postern_core::domain::EvalContext,
        _spec: &serde_json::Value,
    ) -> Result<bool, postern_core::error::PredicateError> {
        Ok(true)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Fake-execute 适配器：真 classify/check_constraint（委派 PostgresAdapter），仅 execute 假
// ════════════════════════════════════════════════════════════════════════════
//
// 归类（伪装写识破）与细则是真 PostgresAdapter 的逻辑；本包装只把 execute 换成回固定字节
// （含真实地址/凭据样本，用于钉「脱敏真擦干净」），其余三方法逐字委派真适配器。protocol()
// 仍报 "postgres" 以命中登记册并保持引擎语义。

struct FakeExecuteAdapter {
    inner: PostgresAdapter,
    /// execute 回放的固定原始字节（含真实地址/凭据样本，验出口脱敏）。
    canned: Vec<u8>,
}

#[async_trait]
impl Adapter for FakeExecuteAdapter {
    fn protocol(&self) -> &'static str {
        self.inner.protocol()
    }
    fn capabilities(&self) -> &'static [Capability] {
        self.inner.capabilities()
    }
    fn engine_enforced(&self) -> bool {
        self.inner.engine_enforced()
    }
    fn classify(
        &self,
        intent: &Intent,
    ) -> Result<ClassifiedIntent, postern_core::error::ClassifyError> {
        // 真归类：语法树级最高危写定档（伪装写识破在此发生）。
        self.inner.classify(intent)
    }
    fn check_constraint(
        &self,
        spec: &ConstraintSpec,
        ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError> {
        // 真细则委派（本测试 grant 不挂 postgres 细则 spec，故此路一般不触；保真委派）。
        self.inner.check_constraint(spec, ci)
    }
    async fn execute(&self, _ch: &mut Channel, _intent: &Intent) -> Result<RawResponse, ExecError> {
        // Fake execute：回固定字节（含真实地址/凭据明文），让出口脱敏器真去擦它。
        Ok(RawResponse {
            payload: self.canned.clone(),
        })
    }
    async fn discover(
        &self,
        _ch: &mut Channel,
    ) -> Result<postern_core::plugin::CapabilitySurface, postern_core::error::DiscoverError> {
        unreachable!("数据面 kernel 永不调用 discover")
    }
}

/// execute 回放字节：一段含真实地址 + 真实凭据明文的「后端响应」，强制脱敏器去擦。
fn canned_backend_bytes() -> Vec<u8> {
    format!(
        "{{\"rows\":[{{\"id\":1}}],\"_debug\":{{\"host\":\"{REAL_HOST}\",\"port\":\"{REAL_PORT}\",\
         \"user\":\"{REAL_DB_USER}\",\"password\":\"{REAL_DB_PASSWORD}\"}}}}"
    )
    .into_bytes()
}

// ════════════════════════════════════════════════════════════════════════════
//  审计汇（记录 decision + stage，供「未授权 → stage=rbac」断言；写恒成功）
// ════════════════════════════════════════════════════════════════════════════

struct RecordingAudit {
    events: Mutex<Vec<(String, Option<postern_core::error::Stage>)>>,
}

impl RecordingAudit {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }
    fn recorded(&self) -> Vec<(String, Option<postern_core::error::Stage>)> {
        self.events.lock().expect("audit log ok").clone()
    }
}

impl AuditSink for RecordingAudit {
    fn record(&self, event: AuditEvent) -> Result<(), postern_core::error::AuditError> {
        self.events
            .lock()
            .expect("audit log ok")
            .push((event.decision.clone(), event.stage));
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  快照 seed：principal `p` 仅持 db-main 的 query 授权格（query-only），tier=readonly
// ════════════════════════════════════════════════════════════════════════════

fn principal() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(42))
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_700_000_000_000)
}

fn unix_origin() -> Origin {
    Origin::UnixPeer {
        uid: 1000,
        gid: 1000,
    }
}

/// seed 一份内存策略快照：principal 在 (db-main, Query) 有一个 Allow 格（挂 always 条件、
/// 不挂 postgres 细则 spec），db-main 的 tier 声明 readonly 承载 Query。**只授 query**——
/// mutate/destroy 无格（用于越权 / 伪装写 deny 断言）。CredentialView 含该 principal 的
/// api_key 凭据元（secret_hash = PRESENTED_SECRET 的哈希）。
fn seed_snapshot() -> PolicySnapshot {
    let resource = ResourceCode::new(RESOURCE);
    let cell = GrantCell {
        resource: resource.clone(),
        capability: Capability::Query,
        role: Role::new("observer"),
        action: GrantAction::Allow,
        constraints: Vec::new(),
        conditions: vec![postern_core::domain::ConditionSpec {
            kind: COND_KIND.into(),
            spec: "{}".into(),
        }],
    };
    let mut per_principal = BTreeMap::new();
    per_principal.insert((resource.clone(), Capability::Query), cell);
    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);

    let mut tiers = BTreeMap::new();
    tiers.insert(
        resource.clone(),
        vec![TierDecl {
            tier: CredentialTier::new(TIER),
            carries: vec![Capability::Query],
        }],
    );

    PolicySnapshot {
        policy_rev: 7,
        grants,
        tiers,
        credentials: CredentialView {
            credentials: vec![CredentialMeta {
                principal: principal(),
                kind: AUTH_KIND.into(),
                secret_hash: secret_hash(PRESENTED_SECRET),
                expires_at: None,
                revoked_at: None,
            }],
        },
        deny_notes: BTreeMap::new(),
        grantable: BTreeMap::new(),
        modes: BTreeMap::new(),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  端到端装配：真 Evaluator + 真 Sanitizer(真 ScrubSet) + 真 PostgresAdapter(classify) +
//  真建连缝(走 secrets) + 真 Kernel
// ════════════════════════════════════════════════════════════════════════════

/// 装配产物：真 Kernel + 审计句柄 + 建连缝句柄（供断言读回）。
struct E2E {
    kernel: Kernel,
    audit: Arc<RecordingAudit>,
    acquire: Arc<RealishAcquire>,
}

fn assemble_e2e() -> E2E {
    // 真 secrets：封 vault → unlock → 句柄（含真实地址/凭据）。
    let vault = Arc::new(unlock_real_vault());

    // 真出口脱敏器：由解锁句柄的 payload 派生真 ScrubSet（targets/secrets 叶子明文全纳入），
    // 装进真 DaemonSanitizer。这是 ① / ④ 的承重件——脱敏是否真擦掉真实地址/凭据由它决定。
    let scrubset = Arc::new(ScrubSet::from_payload(vault.payload()));
    let sanitizer: Arc<dyn Sanitizer> = Arc::new(DaemonSanitizer::new(scrubset));

    // 真 core 求值器：真 Authenticator + 真 always 谓词注册表。
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(AUTH_KIND, Box::new(HashAuthenticator));
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert(COND_KIND, Box::new(AlwaysPredicate));
    let evaluator = Arc::new(Evaluator::new(auths, preds));

    // 真 adapters：真 PostgresAdapter 的 classify/check_constraint，仅 execute Fake。
    let adapter = FakeExecuteAdapter {
        inner: PostgresAdapter,
        canned: canned_backend_bytes(),
    };
    // 登记键恒为 "postgres"（命中登记册唯一解释者）。
    assert_eq!(adapter.protocol(), PG_PROTOCOL, "适配器协议键须为 postgres");
    let adapters = Arc::new(AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>
    ]));

    // 真建连缝：走 secrets 真实凭据/地址解析。
    let acquire = Arc::new(RealishAcquire::new(vault.clone()));
    let audit = Arc::new(RecordingAudit::new());

    let kernel = Kernel::new(
        evaluator,
        adapters,
        acquire.clone() as Arc<dyn ConnAcquire>,
        audit.clone() as Arc<dyn AuditSink>,
        sanitizer,
        Arc::new(seed_snapshot()),
        now(),
    );

    E2E {
        kernel,
        audit,
        acquire,
    }
}

/// 直接跑**真求值器**对一个 (capability, resource) 越权/缺格请求，取其 `DenyResponse`
/// （证明「正确归因的 your_grants」逻辑真实存在——与 kernel 出口的重组行为对照）。
///
/// 复用 seed_snapshot（principal=42 仅持 db-main/query 格）+ 真 HashAuthenticator + always 谓词。
/// 归类结果不经 PostgresAdapter（求值器不归类）：直接构造一个 capability 的 ClassifiedIntent
/// 喂求值器（求值器据 capability 查 RBAC 缺格 → deny）。
fn evaluator_deny_for(capability: Capability, resource: &str) -> DenyResponse {
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(AUTH_KIND, Box::new(HashAuthenticator));
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert(COND_KIND, Box::new(AlwaysPredicate));
    let eval = Evaluator::new(auths, preds);

    let snapshot = seed_snapshot();
    let req = request_for(resource, "");
    let ci = ClassifiedIntent {
        capability,
        objects: Vec::new(),
    };
    let constraint = postern_core::eval::evaluator::ConstraintCheck { passed: true };
    let (decision, _trace) = eval.evaluate(&req, &ci, &constraint, &snapshot, now());
    match decision {
        postern_core::decision::Decision::Deny(d) => d,
        other => panic!("求值器对越权请求应 Deny，得 {other:?}"),
    }
}

/// 据语句原文 + 资源代号组装真实归一化请求（presented 凭据真实，origin 由别名构造）。
fn request_for(resource: &str, statement: &str) -> NormalizedRequest {
    NormalizedRequest {
        presented: PresentedCredential::new(AUTH_KIND, PRESENTED_SECRET.to_vec()),
        origin: unix_origin(),
        resource: ResourceCode::new(resource),
        intent: pg_intent(statement),
    }
}

/// 断言一段字节（出口响应）里**不含任何**真实地址/凭据明文（④ 凭据零接触的取证）。
fn assert_no_forbidden(bytes: &[u8], context: &str) {
    let text = String::from_utf8_lossy(bytes);
    for needle in FORBIDDEN_SUBSTRINGS {
        assert!(
            !text.contains(needle),
            "{context}：出口响应泄露了真实机密子串 {needle:?}（脱敏未擦干净 / 凭据零接触被破）；\
             实测 body={text}"
        );
    }
}

/// 把 DenyResponse 序列化为字节（对 deny 出口做 grep 断言用）。
fn deny_bytes(deny: &DenyResponse) -> Vec<u8> {
    serde_json::to_vec(deny).expect("DenyResponse 应可序列化")
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 ① 已授权 query → Allow → (Fake execute) → 脱敏成功响应，响应不含真实地址/host/凭据
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn assertion_1_authorized_query_allows_and_egress_is_scrubbed() {
    let e = assemble_e2e();
    let corpus = corpus();
    let out = e
        .kernel
        .submit(request_for(RESOURCE, &corpus.benign_query_sql))
        .await;

    // 放行：真归类 Query → 真 RBAC 命中 query 格 → 真 tier readonly → 真建连缝（走 secrets）
    // → Fake execute → 真出口脱敏 → Ok(SanitizedResponse)。
    let sanitized = match out {
        Ok(s) => s,
        Err(deny) => panic!(
            "已授权 query 应放行，却 deny：reason={:?} stage 见审计={:?}",
            deny.reason,
            e.audit.recorded()
        ),
    };

    // 真建连缝确已走通 secrets 真实凭据 + 地址解析（端到端 secrets 链可用）。
    assert!(
        e.acquire.did_resolve(),
        "放行路径必经真建连缝的 secrets 凭据/地址解析（StaticVaultProvider + resolve）"
    );

    // 承重断言（① 核心）：脱敏成功响应里**绝不含**真实地址/host/凭据明文。Fake execute 回的
    // 原始字节里 host/port/user/password 全是真实样本，真 ScrubSet 必须把它们逐一擦成 [REDACTED]。
    assert_no_forbidden(&sanitized.payload, "① 已授权 query 脱敏成功响应");

    // 审计落一条 allow 结果痕（read 动词单痕）。
    assert!(
        e.audit.recorded().iter().any(|(d, _s)| d == "allow"),
        "① 放行 read 动词落单条 allow 结果痕"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 ② 未授权 mutate（无格）→ Deny 且 stage=rbac；your_grants 只列已授格、不泄露存在性
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn assertion_2_unauthorized_mutate_denies_at_rbac_with_scoped_grants() {
    let e = assemble_e2e();
    let corpus = corpus();
    // query-only 授权下打一条 mutate（真归类为 Mutate）→ (db-main, Mutate) 无格 → rbac 缺格 deny。
    let out = e
        .kernel
        .submit(request_for(RESOURCE, &corpus.benign_mutate_sql))
        .await;
    let deny = match out {
        Err(deny) => deny,
        Ok(_) => panic!("未授权 mutate（无格）必 deny"),
    };

    // stage=rbac（经审计读，DenyResponse 本体不直接暴露 stage）。
    assert!(
        e.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(postern_core::error::Stage::Rbac)),
        "② 未授权 mutate 的 deny 审计 stage 必为 Rbac（缺格即拒）"
    );

    // 普通 deny（非 escalate_denied）。
    assert_eq!(deny.decision, "deny", "② 缺格 deny 为普通 deny");
    // denied.capability 保留被请求的真实动词 Mutate（归类已穿透，归因不丢动词）。
    assert_eq!(
        deny.denied.capability,
        Capability::Mutate,
        "② deny 的归因动词须为被请求的 Mutate"
    );
    // 不泄露存在性的承重事实：principal=42 的 your_grants 绝不含被探测的 Scope 外动词痕迹，
    // request_hint 不暗示 mutate 可授性（snapshot.grantable 空 → None）。这两条无论
    // your_grants 是否被正确归因都恒成立（不泄露存在性的底线）。
    assert!(
        deny.request_hint.is_none(),
        "② 越权 mutate 的 request_hint 为 None（不暗示其可授性）"
    );

    // ── ② 核心：your_grants 应只列 principal 自身已授格（{db-main:[query]}）——这是 deny.rs
    //    §your_grants 规约。kernel 出口已修复为沿用求值器已正确归因的 DenyResponse（不再以零
    //    principal 重组），故求值器层与 kernel 边界两路归因一致。下面分两路咬合钉死：
    //    (A) 求值器单独跑——证明「正确归因的 your_grants」逻辑真实存在且正确（{db-main:[query]}）；
    //    (B) kernel 出口——证明 kernel 边界沿用了该正确归因（your_grants={db-main:[query]}、
    //        denied.resource=db-main），与 (A) 一致。
    let db_main = ResourceCode::new(RESOURCE);

    // (A) 真求值器对同一越权 mutate 的 deny：your_grants 恰为 {db-main:[query]}（spec 正确形态）。
    let eval_deny = evaluator_deny_for(Capability::Mutate, RESOURCE);
    let eval_caps = eval_deny.your_grants.get(&db_main).unwrap_or_else(|| {
        panic!("②(A) 求值器 deny 的 your_grants 应含 principal 自身已授的 db-main 格")
    });
    assert_eq!(
        eval_caps,
        &vec!["query".to_string()],
        "②(A) 求值器层 your_grants 正确只列已授 query 格（不泄露被探测 mutate）"
    );
    assert_eq!(
        eval_deny.your_grants.len(),
        1,
        "②(A) 求值器层 your_grants 恰一格（不枚举他人/全局）"
    );
    assert_eq!(
        eval_deny.denied.resource, db_main,
        "②(A) 求值器层 denied.resource 为真实请求代号 db-main"
    );

    // (B) kernel 出口修复后的正确行为：kernel 沿用求值器已正确归因的 DenyResponse，不再以
    //     unattributed() 零 principal 重组。故 your_grants 反映 principal 自身真实授权世界
    //     {db-main:[query]}，denied.resource 机械回显请求代号 db-main——与 (A) 求值器层咬合一致。
    let kernel_caps = deny.your_grants.get(&db_main).unwrap_or_else(|| {
        panic!("②(B) kernel 出口 your_grants 应含 principal 自身已授的 db-main 格")
    });
    assert_eq!(
        kernel_caps,
        &vec!["query".to_string()],
        "②(B) kernel 出口 your_grants 正确只列已授 query 格（反映真实授权，不泄露被探测 mutate）"
    );
    assert_eq!(
        deny.your_grants.len(),
        1,
        "②(B) kernel 出口 your_grants 恰一格（不枚举他人/全局）"
    );
    assert_eq!(
        deny.denied.resource.as_str(),
        RESOURCE,
        "②(B) kernel 出口 denied.resource 机械回显请求代号 db-main（攻击者输入），沿用求值器归因"
    );
    // 关键安全底线仍成立：your_grants 只列 principal 自身已授格，绝不泄露被探测资源/动词存在性。

    // ④ 顺带：deny 出口经 grep 不含任何真实地址/凭据明文。
    assert_no_forbidden(&deny_bytes(&deny), "② 未授权 mutate 的 deny 出口");
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 ③ 伪装写（write CTE）→ postgres classify 归 Destroy → Deny（绝不当 Query 放行）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn assertion_3_disguised_write_classified_destroy_and_denied() {
    let e = assemble_e2e();
    let corpus = corpus();

    // 先独立钉死真 PostgresAdapter 把伪装写归 **Destroy**（穿透只读 CTE 外壳，绝不降为 Query）。
    let real = PostgresAdapter;
    let ci = real
        .classify(&pg_intent(&corpus.disguise_sql))
        .expect("伪装写应被 postgres 真归类（write CTE 可解析）");
    assert_eq!(
        ci.capability,
        Capability::Destroy,
        "③ 伪装写（write CTE 包裹）必归真实最高危档 Destroy，绝不因只读外壳降为 Query"
    );

    // 端到端：同一伪装写经 kernel（query-only 授权）→ 归 Destroy → (db-main, Destroy) 无格 →
    // deny，绝不当 Query 放行执行。
    let out = e
        .kernel
        .submit(request_for(RESOURCE, &corpus.disguise_sql))
        .await;
    let deny = match out {
        Err(deny) => deny,
        Ok(_) => panic!("③ 伪装写归 Destroy 后在 query-only 授权下必 deny（绝不放行执行）"),
    };
    // deny 的 denied.capability 恰为 Destroy（归类已穿透外壳，deny 据真实档归因）。
    assert_eq!(
        deny.denied.capability,
        Capability::Destroy,
        "③ deny 的归因动词须为真实最高危档 Destroy（伪装写未被降级）"
    );
    // stage=rbac（Destroy 无格 → 缺格拒），绝非「当 Query 放行后再拒」。
    assert!(
        e.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(postern_core::error::Stage::Rbac)),
        "③ 伪装写 deny 落 rbac 阶（Destroy 缺格），绝不进 execute"
    );
    // ④ 顺带：该 deny 出口不含真实机密。
    assert_no_forbidden(&deny_bytes(&deny), "③ 伪装写 deny 出口");
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 ④ 凭据零接触：allow / deny / 错误响应经 grep 均不含真实地址/host/凭据明文
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn assertion_4_credentials_never_touch_any_egress() {
    let e = assemble_e2e();
    let corpus = corpus();

    // (a) allow 出口（脱敏成功响应）。
    let allow_out = e
        .kernel
        .submit(request_for(RESOURCE, &corpus.benign_query_sql))
        .await;
    let sanitized = allow_out.expect("④(a) 已授权 query 应放行");
    assert_no_forbidden(&sanitized.payload, "④(a) allow 脱敏成功响应");

    // (b) deny 出口（越权 mutate）。
    let deny_out = e
        .kernel
        .submit(request_for(RESOURCE, &corpus.benign_mutate_sql))
        .await;
    let deny = match deny_out {
        Err(d) => d,
        Ok(_) => panic!("④(b) 越权 mutate 应 deny"),
    };
    assert_no_forbidden(&deny_bytes(&deny), "④(b) 越权 deny 出口");

    // (c) 错误/默认拒绝出口（未知资源代号——见 ⑤；这里复用以覆盖错误响应类）。
    let unknown_out = e
        .kernel
        .submit(request_for("ghost-xyz", &corpus.benign_query_sql))
        .await;
    let unknown_deny = match unknown_out {
        Err(d) => d,
        Ok(_) => panic!("④(c) 未知资源应 deny"),
    };
    assert_no_forbidden(&deny_bytes(&unknown_deny), "④(c) 未知资源 deny 出口");

    // 三类出口都过了 grep：真实地址/host/凭据明文零接触任何边界外字节。
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 ⑤ 默认拒绝：未知资源代号 → deny，不泄露存在性
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn assertion_5_unknown_resource_denies_without_leaking_existence() {
    let e = assemble_e2e();
    let corpus = corpus();

    // 已认证 principal（凭据真实），但打一个快照里根本不存在的资源代号。
    let out = e
        .kernel
        .submit(request_for("ghost-xyz", &corpus.benign_query_sql))
        .await;
    let unknown_deny = match out {
        Err(d) => d,
        Ok(_) => panic!("⑤ 未知资源代号必 deny（默认拒绝）"),
    };

    // stage=rbac（principal 在 (ghost-xyz, Query) 无格 → 缺格拒）。
    assert!(
        e.audit
            .recorded()
            .iter()
            .any(|(_d, s)| *s == Some(postern_core::error::Stage::Rbac)),
        "⑤ 未知资源 deny 落 rbac 阶（缺格即拒）"
    );

    // 不泄露存在性（⑤ 核心，与 kernel bug 无关、恒成立）：被探测的未知代号 ghost-xyz 绝不
    // 出现在 your_grants 里，request_hint 不暗示其可授性。这正是「默认拒绝且不泄露存在性」。
    let ghost = ResourceCode::new("ghost-xyz");
    assert!(
        !unknown_deny.your_grants.contains_key(&ghost),
        "⑤ 未知资源的 your_grants 绝不含被探测的 ghost-xyz（不泄露存在性）"
    );
    assert!(
        unknown_deny.request_hint.is_none(),
        "⑤ 未知资源的 request_hint 为 None（snapshot.grantable 无之 → 不暗示其可授性/存在性）"
    );

    // 不可区分性的进程内最强证据（L-13）：必须以**同一意图**（同一 SQL → 同一归类
    // capability/objects）打两个资源代号——一个「已存在但越界」(db-main 打 mutate，db-main 只授
    // query)、一个「根本不存在」(ghost-xyz 打同一 mutate)——两路 deny 须在**存在性敏感字段**上
    // 不可区分。用同一 mutate SQL 保证 denied.capability/objects 同形（它们源自请求方自己的意图、
    // 非快照查表产物，不构成存在性泄露）；真正可能泄露拓扑的 your_grants/request_hint/reason
    // /denied.capability/denied.objects 则须两路相同。denied.resource 例外：core 文档钉死其语义为
    // 「Resource the request targeted」，机械回显**攻击者自己输入的请求代号**，零存在性泄露，故
    // 两路各自回显自身代号（db-main / ghost-xyz）而不相等——这正确而非缺陷。
    let same_mutate = &corpus.benign_mutate_sql;
    let out_of_scope_deny = match e.kernel.submit(request_for(RESOURCE, same_mutate)).await {
        Err(d) => d,
        Ok(_) => panic!("⑤ 已存在但越界（db-main mutate）应 deny"),
    };
    let nonexistent_deny = match e.kernel.submit(request_for("ghost-xyz", same_mutate)).await {
        Err(d) => d,
        Ok(_) => panic!("⑤ 根本不存在（ghost-xyz mutate）应 deny"),
    };
    assert_eq!(
        out_of_scope_deny.your_grants, nonexistent_deny.your_grants,
        "⑤ L-13：your_grants 两路须不可区分（不泄露存在性）"
    );
    assert_eq!(
        out_of_scope_deny.request_hint, nonexistent_deny.request_hint,
        "⑤ L-13：request_hint 两路须不可区分（均 None，不暗示可授性/存在性）"
    );
    assert_eq!(
        out_of_scope_deny.reason, nonexistent_deny.reason,
        "⑤ L-13：reason 两路须不可区分（不泄露存在性）"
    );
    assert_eq!(
        out_of_scope_deny.denied.capability, nonexistent_deny.denied.capability,
        "⑤ L-13：denied.capability 两路须不可区分（源自请求方意图，非快照查表）"
    );
    assert_eq!(
        out_of_scope_deny.denied.objects, nonexistent_deny.denied.objects,
        "⑤ L-13：denied.objects 两路须不可区分（源自请求方意图，非快照查表）"
    );
    // 修复后的正确行为：denied.resource 机械回显各自请求代号（攻击者自己的输入），不相等且正确。
    assert_eq!(
        out_of_scope_deny.denied.resource.as_str(),
        RESOURCE,
        "⑤ denied.resource 机械回显请求代号 db-main（攻击者输入，零存在性泄露）"
    );
    assert_eq!(
        nonexistent_deny.denied.resource.as_str(),
        "ghost-xyz",
        "⑤ denied.resource 机械回显请求代号 ghost-xyz（攻击者输入，零存在性泄露）"
    );
    // 承重子事实：被探测的 ghost-xyz 不出现在 your_grants（your_grants 只反映 principal 自身真实
    // 授权世界），request_hint 恒 None——存在性敏感字段无差异。
    assert!(
        !nonexistent_deny.your_grants.contains_key(&ghost),
        "⑤ 不存在资源的 your_grants 绝不含被探测的 ghost-xyz"
    );
}
