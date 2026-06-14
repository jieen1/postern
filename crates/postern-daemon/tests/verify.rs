//! 控制面红队自检 `POST /v1/verify` 的真断言（TDD：先红后绿）。
//!
//! 钉死红队自检（详细设计 6.7；技术设计 13.4 / 11.4「自我观测」；场景 07 §3/§4.1 九类红队项）：
//! daemon 以一个**临时低权 Principal** 自我发起一组**应被拒绝**的数据面请求，每条走完整管线
//! [0]→[10]，逐条确认结果符合预期（八项 deny + 第 9 项脱敏探测放行但响应无敏感回显），且均出现
//! 在审计中。任一项不符 → 该项 FAIL、`all_pass=false`，FAIL 项 `gap_note` 指出缺口防线。
//!
//! 驱动方式（与 e2e_acceptance.rs 同构，用真实组件装配真 Kernel）：
//! - 真 core 求值器 [`Evaluator`]（真 RBAC + 真三值决策 + 真结构化 deny）；
//! - 真 secrets 保险箱（crypto 封 vault → unlock → 真 ScrubSet）+ 真出口脱敏器 [`DaemonSanitizer`]；
//! - 真 [`PostgresAdapter::classify`]（伪装写 / 多语句 / SET 识破），仅 execute Fake（回含真实
//!   地址/凭据样本的固定字节，验脱敏真擦干净）；
//! - 真 [`Kernel::submit`] 跑完整 [0]→[10]。verify 经 [`run_verify`] 对该 Kernel 自发探针。
//!
//! 认证用 **local_process**（零凭证族、**无 argon2**）：临时低权 principal 经 SO_PEERCRED 观测
//! uid 裁定（secret_hash 文本承载 uid 规则），故本测试**无需** argon2 内存上限包裹。
//!
//! 先红后绿（禁 should_panic 反向桩 / 空壳 / 弱断言）：
//! - all-pass：seed 一份「正确」策略（低权 principal 仅持 db-main/query，无越权授权格）→ verify
//!   全项 PASS、all_pass=true（真断言每条 item.pass + gap_note 为 None）。
//! - 某项-FAIL：构造「防线被破」setup（给低权 principal 越权授权格，使越权 mutate 被放行）→
//!   对应项 FAIL、gap_note 指出缺口（真断言该项 pass=false 且 gap_note 含「漏放」语义）。
//!
//! 雷区纪律：本 `.rs` **零 SQL 标记**（探针 SQL 全在 `verify_corpus/probes.json` 数据文件，经
//! include_str! 读取）；需要 `ConnOrigin` 以 `use postern_core::request::ConnOrigin as Origin`
//! 别名构造（测试在 shells 外）；**绝不**构造 `ResolvedTarget`/`ResourceCredential`（走 secrets
//! 真实构造路径）；异步用 `#[tokio::test]`。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use zeroize::Zeroizing;

use postern_core::domain::{
    Capability, ConstraintSpec, CredentialMeta, CredentialTier, CredentialView, GrantAction,
    GrantCell, PolicySnapshot, PrincipalId, ResourceCode, Role, TierDecl, Timestamp,
};
use postern_core::error::{ExecError, TransportError};
use postern_core::eval::evaluator::Evaluator;
use postern_core::id::SnowflakeId;
use postern_core::plugin::sanitize::Sanitizer;
use postern_core::plugin::{Adapter, Authenticator, Channel, ConditionPredicate, RawResponse};
// 测试在 shells 外：以别名构造请求来源，绝不写字面 ConnOrigin:: 变体（雷区）。
use postern_core::request::ConnOrigin as Origin;
use postern_core::request::{ClassifiedIntent, Intent};

use postern_adapters::postgres::{PostgresAdapter, PROTOCOL as PG_PROTOCOL};

use postern_secrets::scrubset::ScrubSet;
use postern_secrets::vault::crypto;
use postern_secrets::vault::format::{VaultFile, FORMAT_VERSION, NONCE_LEN};
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;
use postern_secrets::vault::{self, UnlockedVault};

use postern_daemon::control::verify::{
    run_verify, ProbeContext, VerifyAudit, VerifyCorpus, VerifyReport,
};
use postern_daemon::identity::local_process::{LocalProcessAuthenticator, KIND as LOCAL_KIND};
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

/// vault `targets` 段真实地址样本——**绝不可出现在任何出口响应里**（脱敏须擦掉）。
const REAL_HOST: &str = "10.77.88.99";
const REAL_PORT: &str = "5432";
/// vault `secrets` 段真实凭据样本——**绝不可出现在任何出口响应里**。
const REAL_DB_USER: &str = "ro_acct";
const REAL_DB_PASSWORD: &str = "s3cr3t-ro-pw-zzz";

/// 临时低权 principal 经 SO_PEERCRED 观测到的可信 uid（local_process 规则承载它）。
const LOW_PRIV_UID: u32 = 1000;
const LOW_PRIV_GID: u32 = 1000;
/// 一个**不被采信**的来源 uid（local_process 规则不匹配它 → auth 阶拒）。
const UNTRUSTED_UID: u32 = 9999;

// ════════════════════════════════════════════════════════════════════════════
//  探针语料（数据文件读取——本 .rs 零 SQL 标记，B 方案）
// ════════════════════════════════════════════════════════════════════════════

const CORPUS_JSON: &str = include_str!("verify_corpus/probes.json");

#[derive(Deserialize)]
struct CorpusFile {
    scope_out_mutate: String,
    disguised_write: String,
    session_tamper: String,
    multi_statement: String,
    benign_query: String,
}

fn corpus() -> VerifyCorpus {
    let f: CorpusFile =
        serde_json::from_str(CORPUS_JSON).expect("verify_corpus/probes.json 可解析");
    VerifyCorpus {
        scope_out_mutate: f.scope_out_mutate,
        disguised_write: f.disguised_write,
        session_tamper: f.session_tamper,
        multi_statement: f.multi_statement,
        benign_query: f.benign_query,
    }
}

/// 禁现子串集（真实地址 / 凭据明文）——verify 据其 grep 凭据零接触 / 脱敏探测两项。
fn forbidden() -> Vec<String> {
    vec![
        REAL_HOST.to_string(),
        REAL_DB_USER.to_string(),
        REAL_DB_PASSWORD.to_string(),
    ]
}

// ════════════════════════════════════════════════════════════════════════════
//  真实 secrets 装配：crypto 封 vault → unlock → 句柄（targets/secrets 含真实样本）
// ════════════════════════════════════════════════════════════════════════════

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

/// 端到端封一份**含真实地址 + 真实凭据**的 vault 字节，经真 `vault::unlock` 还原句柄。
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
//  最小建连缝：交还不透明 Channel（Transport/execute 是 Fake，无真实连接）
// ════════════════════════════════════════════════════════════════════════════

struct StubAcquire;

impl ConnAcquire for StubAcquire {
    fn acquire<'a>(
        &'a self,
        _resource: &'a ResourceCode,
        _tier: &'a CredentialTier,
    ) -> Pin<Box<dyn Future<Output = Result<Channel, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(Channel {
                handle: Box::new(()),
            })
        })
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
//  Fake-execute 适配器：真 classify（委派 PostgresAdapter），仅 execute 假
// ════════════════════════════════════════════════════════════════════════════

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
        self.inner.classify(intent)
    }
    fn check_constraint(
        &self,
        spec: &ConstraintSpec,
        ci: &ClassifiedIntent,
    ) -> Result<bool, postern_core::error::ConstraintError> {
        self.inner.check_constraint(spec, ci)
    }
    async fn execute(&self, _ch: &mut Channel, _intent: &Intent) -> Result<RawResponse, ExecError> {
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
//  快照 seed
// ════════════════════════════════════════════════════════════════════════════

fn principal() -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(42))
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_700_000_000_000)
}

/// local_process 凭据 meta：secret_hash 文本承载 uid 规则（零真实 secret，无 argon2）。
fn local_cred_meta() -> CredentialMeta {
    CredentialMeta {
        principal: principal(),
        kind: LOCAL_KIND.into(),
        secret_hash: format!("uid={LOW_PRIV_UID},gid={LOW_PRIV_GID}"),
        expires_at: None,
        revoked_at: None,
    }
}

/// 一个挂 always 条件、不挂细则的授权格。
fn allow_cell(capability: Capability) -> GrantCell {
    GrantCell {
        resource: ResourceCode::new(RESOURCE),
        capability,
        role: Role::new("observer"),
        action: GrantAction::Allow,
        constraints: Vec::new(),
        conditions: vec![postern_core::domain::ConditionSpec {
            kind: COND_KIND.into(),
            spec: "{}".into(),
        }],
    }
}

/// db-main 的 tier 声明：readonly 承载 query；若 `carry_writes` 则额外承载 mutate/destroy
/// （「防线被破」setup 用——tier 声明 ⊃ 账号真实权限，使越权写有承载 tier 可放行）。
fn tier_decls(carry_writes: bool) -> Vec<TierDecl> {
    let mut carries = vec![Capability::Query];
    if carry_writes {
        carries.push(Capability::Mutate);
        carries.push(Capability::Destroy);
    }
    vec![TierDecl {
        tier: CredentialTier::new(TIER),
        carries,
    }]
}

/// seed 一份内存策略快照。`extra_grants` 注入额外授权格（「防线被破」setup 用）；`carry_writes`
/// 让 tier 额外承载写动词。正确策略：principal 仅持 (db-main, Query) Allow 格、tier 只承载 query。
fn seed_snapshot(extra_grants: &[Capability], carry_writes: bool) -> PolicySnapshot {
    let resource = ResourceCode::new(RESOURCE);
    let mut per_principal = BTreeMap::new();
    per_principal.insert(
        (resource.clone(), Capability::Query),
        allow_cell(Capability::Query),
    );
    for cap in extra_grants {
        per_principal.insert((resource.clone(), *cap), allow_cell(*cap));
    }
    let mut grants = BTreeMap::new();
    grants.insert(principal(), per_principal);

    let mut tiers = BTreeMap::new();
    tiers.insert(resource.clone(), tier_decls(carry_writes));

    PolicySnapshot {
        policy_rev: 7,
        grants,
        tiers,
        credentials: CredentialView {
            credentials: vec![local_cred_meta()],
        },
        deny_notes: BTreeMap::new(),
        grantable: BTreeMap::new(),
        modes: BTreeMap::new(),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  端到端装配：真 Evaluator(local_process) + 真 Sanitizer + 真 PostgresAdapter + 真 Kernel
// ════════════════════════════════════════════════════════════════════════════

/// 装配产物：真 Kernel + 与之共享 Arc 的 verify 审计汇。
struct Harness {
    kernel: Kernel,
    audit: Arc<VerifyAudit>,
}

fn assemble(snapshot: PolicySnapshot) -> Harness {
    let vault = Arc::new(unlock_real_vault());

    let scrubset = Arc::new(ScrubSet::from_payload(vault.payload()));
    let sanitizer: Arc<dyn Sanitizer> = Arc::new(DaemonSanitizer::new(scrubset));

    // 真 core 求值器：真 local_process 认证器（无 argon2）+ 真 always 谓词。
    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(LOCAL_KIND, Box::new(LocalProcessAuthenticator));
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert(COND_KIND, Box::new(AlwaysPredicate));
    let evaluator = Arc::new(Evaluator::new(auths, preds));

    let adapter = FakeExecuteAdapter {
        inner: PostgresAdapter,
        canned: canned_backend_bytes(),
    };
    assert_eq!(adapter.protocol(), PG_PROTOCOL, "适配器协议键须为 postgres");
    let adapters = Arc::new(AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>
    ]));

    let audit = Arc::new(VerifyAudit::new());

    let kernel = Kernel::new(
        evaluator,
        adapters,
        Arc::new(StubAcquire) as Arc<dyn ConnAcquire>,
        audit.clone() as Arc<dyn postern_core::plugin::AuditSink>,
        sanitizer,
        Arc::new(snapshot),
        now(),
    );

    Harness { kernel, audit }
}

/// verify 临时低权材料：local_process 零凭证（空出示 secret）、可信 / 不可信观测来源。
fn probe_ctx() -> ProbeContext {
    ProbeContext {
        auth_kind: LOCAL_KIND.to_string(),
        presented_secret: Vec::new(),
        resource: ResourceCode::new(RESOURCE),
        trusted_origin: Origin::UnixPeer {
            uid: LOW_PRIV_UID,
            gid: LOW_PRIV_GID,
        },
        untrusted_origin: Origin::UnixPeer {
            uid: UNTRUSTED_UID,
            gid: LOW_PRIV_GID,
        },
    }
}

/// 跑一次 verify（正确策略 seed），回报告。
async fn run_with(snapshot: PolicySnapshot) -> VerifyReport {
    let h = assemble(snapshot);
    let ctx = probe_ctx();
    let corpus = corpus();
    run_verify(&h.kernel, &h.audit, &ctx, &corpus, &forbidden()).await
}

/// 取报告里某项（按名）。
fn item<'r>(
    report: &'r VerifyReport,
    name: &str,
) -> &'r postern_daemon::control::verify::VerifyItem {
    report
        .items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("报告应含探针项 '{name}'，实得 {:?}", report.items))
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 A：正确策略 → verify 全项 PASS、all_pass=true（九类红队项逐条被拒/无回显）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn correct_policy_makes_all_redteam_probes_pass() {
    // 正确策略：低权 principal 仅持 db-main/query 格，tier 只承载 query（无越权授权、无越权 tier）。
    let report = run_with(seed_snapshot(&[], false)).await;

    // 九类探针全部 PASS（逐条真断言：任一项 FAIL 即整体红，gap_note 暴露是哪条防线）。
    for it in &report.items {
        assert!(
            it.pass,
            "正确策略下探针 '{}' 本应 PASS，却 FAIL：{:?}",
            it.name, it.gap_note
        );
        assert_eq!(it.gap_note, None, "PASS 项 '{}' 不应带 gap_note", it.name);
    }
    assert!(report.all_pass, "九类红队项全 PASS ⇒ all_pass=true");

    // 探针面**恰覆盖**九类（设计承诺：缺一类 / 多一类即红）。
    let names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "scope_out_mutate",
            "disguised_write",
            "session_tamper",
            "multi_statement",
            "default_deny_unknown_resource",
            "credential_zero_touch",
            "origin_not_trusted",
            "untrusted_origin_auth_stage",
            "redaction_probe",
        ],
        "verify 须恰覆盖九类红队探针（顺序确定）"
    );
    assert_eq!(report.items.len(), 9, "九类红队项恰九条");
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 B（防线被破→FAIL）：给低权 principal 越权授权格 + 越权 tier ⇒ 越权写被放行 ⇒
//  scope_out_mutate / disguised_write 两项 FAIL，all_pass=false，gap_note 指出缺口
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn broken_defense_grants_overreach_makes_probe_fail() {
    // 防线被破：给低权 principal 额外授 (db-main, Mutate) + (db-main, Destroy) 格，且 tier 声明
    // 越权承载 mutate/destroy（tier 声明 ⊃ 账号真实权限）——本应被拒的越权写/伪装写现被放行。
    let report = run_with(seed_snapshot(
        &[Capability::Mutate, Capability::Destroy],
        true,
    ))
    .await;

    // 越权写探针 FAIL：本应在 rbac 阶被拒，实测放行（防线漏放）。
    let scope = item(&report, "scope_out_mutate");
    assert!(
        !scope.pass,
        "防线被破（越权授格）下 scope_out_mutate 本应 FAIL（越权写竟被放行）"
    );
    let scope_note = scope
        .gap_note
        .as_ref()
        .expect("FAIL 项须带 gap_note 指出缺口");
    assert!(
        scope_note.contains("rbac") && scope_note.contains("漏放"),
        "scope_out_mutate 的 gap_note 须指出『本应 rbac 阶拒、实测漏放』，实得：{scope_note}"
    );

    // 伪装写探针 FAIL：归 Destroy 后本应 rbac 拒，越权 Destroy 授格使其被放行。
    let disguise = item(&report, "disguised_write");
    assert!(
        !disguise.pass,
        "防线被破下 disguised_write 本应 FAIL（伪装写归 Destroy 却因越权 Destroy 授格被放行）"
    );
    assert!(
        disguise
            .gap_note
            .as_ref()
            .map(|n| n.contains("rbac") && n.contains("漏放"))
            .unwrap_or(false),
        "disguised_write 的 gap_note 须指出 rbac 阶漏放，实得：{:?}",
        disguise.gap_note
    );

    // 整体失败：任一项 FAIL ⇒ all_pass=false（verify 绝不假装通过，详设 6.7 / 场景 E6）。
    assert!(
        !report.all_pass,
        "防线被破 ⇒ all_pass=false（verify 如实暴露缺口，不假装通过）"
    );

    // 未被破的防线仍 PASS（缺口是局部的、verify 精确归因到被破项，不误伤其他项）。
    assert!(
        item(&report, "session_tamper").pass,
        "归类层防线未破 ⇒ session_tamper 仍 PASS"
    );
    assert!(
        item(&report, "multi_statement").pass,
        "归类层防线未破 ⇒ multi_statement 仍 PASS"
    );
    assert!(
        item(&report, "origin_not_trusted").pass,
        "来源观测门未破 ⇒ origin_not_trusted 仍 PASS"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 C（防线被破→FAIL）：脱敏出口被破（ScrubSet 不擦真实样本）⇒ redaction_probe FAIL，
//  且 credential_zero_touch 亦 FAIL（deny 出口回显真实机密）
// ════════════════════════════════════════════════════════════════════════════

/// 不擦任何东西的「破脱敏器」：把后端字节原样放行（模拟脱敏出口防线被破）。
struct LeakySanitizer;

impl Sanitizer for LeakySanitizer {
    fn scrub(
        &self,
        payload: RawResponse,
        _declared: &[postern_core::plugin::sanitize::MaskRule],
    ) -> postern_core::plugin::sanitize::SanitizedResponse {
        postern_core::plugin::sanitize::SanitizedResponse {
            payload: payload.payload,
        }
    }
    fn scrub_stream(
        &self,
        _declared: &[postern_core::plugin::sanitize::MaskRule],
    ) -> Box<dyn postern_core::plugin::sanitize::StreamScrubber> {
        unreachable!("verify 探针不走流式脱敏")
    }
}

#[tokio::test]
async fn broken_egress_scrub_makes_redaction_probe_fail() {
    // 正确策略，但把出口脱敏器换成「破脱敏器」（原样回显后端字节，含真实地址/凭据）。
    let vault = Arc::new(unlock_real_vault());
    let sanitizer: Arc<dyn Sanitizer> = Arc::new(LeakySanitizer);

    let mut auths: BTreeMap<&'static str, Box<dyn Authenticator>> = BTreeMap::new();
    auths.insert(LOCAL_KIND, Box::new(LocalProcessAuthenticator));
    let mut preds: BTreeMap<&'static str, Box<dyn ConditionPredicate>> = BTreeMap::new();
    preds.insert(COND_KIND, Box::new(AlwaysPredicate));
    let evaluator = Arc::new(Evaluator::new(auths, preds));

    let adapter = FakeExecuteAdapter {
        inner: PostgresAdapter,
        canned: canned_backend_bytes(),
    };
    let adapters = Arc::new(AdapterRegistry::new(vec![
        Box::new(adapter) as Box<dyn Adapter>
    ]));
    let audit = Arc::new(VerifyAudit::new());
    let kernel = Kernel::new(
        evaluator,
        adapters,
        Arc::new(StubAcquire) as Arc<dyn ConnAcquire>,
        audit.clone() as Arc<dyn postern_core::plugin::AuditSink>,
        sanitizer,
        Arc::new(seed_snapshot(&[], false)),
        now(),
    );
    // vault 句柄在装配里只用于 ScrubSet 派生；本破脱敏器不用 ScrubSet，显式丢弃以示对照。
    drop(vault);

    let report = run_verify(&kernel, &audit, &probe_ctx(), &corpus(), &forbidden()).await;

    // 脱敏探测项 FAIL：放行响应回显了真实机密（脱敏出口防线被破）。
    let redact = item(&report, "redaction_probe");
    assert!(
        !redact.pass,
        "脱敏出口被破下 redaction_probe 本应 FAIL（放行响应回显真实机密）"
    );
    assert!(
        redact
            .gap_note
            .as_ref()
            .map(|n| n.contains("回显")
                && (n.contains(REAL_HOST)
                    || n.contains(REAL_DB_USER)
                    || n.contains(REAL_DB_PASSWORD)))
            .unwrap_or(false),
        "redaction_probe 的 gap_note 须指出回显了真实机密子串，实得：{:?}",
        redact.gap_note
    );

    assert!(!report.all_pass, "脱敏出口被破 ⇒ all_pass=false");
}

// ════════════════════════════════════════════════════════════════════════════
//  断言 D：路由落地——mount_verify 使 POST /v1/verify 真实可达（非 501 占位），回 JSON 报告
// ════════════════════════════════════════════════════════════════════════════

/// 最小 VerifyRunner：回一份固定报告（路由落地测只验「路由可达 + 回 JSON 报告」，探针逻辑由
/// 断言 A/B/C 真组件覆盖）。
struct CannedRunner {
    report: VerifyReport,
}

impl postern_daemon::control::verify::VerifyRunner for CannedRunner {
    fn run(&self) -> Pin<Box<dyn Future<Output = VerifyReport> + Send + '_>> {
        let report = self.report.clone();
        Box::pin(async move { report })
    }
}

#[tokio::test]
async fn mount_verify_makes_route_reachable_and_returns_report() {
    use tower::ServiceExt; // oneshot

    let report = VerifyReport {
        items: vec![postern_daemon::control::verify::VerifyItem {
            name: "scope_out_mutate".to_string(),
            pass: true,
            gap_note: None,
        }],
        all_pass: true,
    };
    let runner: Arc<dyn postern_daemon::control::verify::VerifyRunner> =
        Arc::new(CannedRunner { report });

    // 把 verify 路由接到一个空基 router（本测只验路由落地，不需全套控制面 state）。
    let app = postern_daemon::control::router::mount_verify(axum::Router::new(), runner);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/verify")
        .body(axum::body::Body::empty())
        .expect("request builds");
    let resp = app.oneshot(req).await.expect("router serves");

    // 路由真实可达：非 404（确被挂上）、非 501（非占位 stub，是真实 verify handler）。
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::NOT_FOUND,
        "mount_verify 后 /v1/verify 须可达（非 404）"
    );
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "mount_verify 后 /v1/verify 须由真实 handler 处理（非 501 占位）"
    );
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "verify 路由回 200 + 报告"
    );

    // 回体是 VerifyReport 的 JSON（CLI / SPA 据此渲染逐条 PASS/FAIL + all_pass）。
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .expect("read body");
    let parsed: VerifyReport = serde_json::from_slice(&body).expect("回体须为 VerifyReport JSON");
    assert!(parsed.all_pass, "回体 all_pass 须忠实反映 runner 报告");
    assert_eq!(parsed.items.len(), 1, "回体 items 须忠实反映 runner 报告");
    assert_eq!(parsed.items[0].name, "scope_out_mutate");
}
