//! Provider 单元 `postern_secrets::provider` 行为测试（RED）。
//!
//! 钉死 §8 F-3（凭据解析 (res,tier)）、F-5（机密类型唯一构造）、F-7（会话来源三档）、
//! L-5（配置缺失→deny）、L-7（账号密码会话过期无人值守重登）、L-8（live-session 命中
//! 复用不重登）、L-9（续会话单飞）、L-10（会话硬过期→fail-closed + 不泄账号）、
//! L-11（明文不出边界）。每条只钉一个行为，断言精确到具体值 / 变体 / 错误字段，禁弱断言。
//!
//! 接口（签名权威：模块文档 §5.3 与 `core::plugin::channel`）：
//! `CredentialProvider::credential_for(res, tier) -> Result<ResourceCredential, CredentialError>`。
//! 静态来源实现 `StaticVaultProvider`；会话来源 `LiveSessionProvider`（详细设计 6.13）。
//!
//! 决定性夹具：
//! - 静态来源用**直接持有型 32B 主密钥**（KeyFile 来源）经 `vault::unlock` 构造句柄，
//!   避开 passphrase argon2id KDF（本路径纯查表，夹具不跑 KDF）。
//! - 会话来源用 **Fake 时钟** + **Fake 认证端点**驱动续期路径，使 L-7/L-8/L-9/L-10 确定可复现。
//! - async：本 crate 不依赖 runtime（§3.1），测试内置一个零依赖 `block_on` 单线程执行器
//!   驱动 future（续会话经 Fake 端点同步完成，无真实 IO）。
//!
//! 雷区纪律：所有真实地址/凭据样本只在 `Zeroizing` 内入 payload；文本不出现任何裸数据库
//! 写标记；不字面构造 `ConnOrigin`。机密类型 `Debug=REDACTED` 的 `Ok` 侧不 `unwrap`，
//! 失败侧单独 `match`。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Waker};

use postern_core::domain::{CredentialTier, ResourceCode, ResourceCredential};
use postern_core::error::CredentialError;
use postern_core::plugin::CredentialProvider;

use postern_secrets::provider::session::{
    Clock, LiveSessionProvider, RenewedSession, SessionAuthority, SessionForm,
};
use postern_secrets::provider::static_vault::StaticVaultProvider;
use postern_secrets::vault::crypto;
use postern_secrets::vault::format::{VaultFile, FORMAT_VERSION, NONCE_LEN};
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;
use postern_secrets::vault::{self, UnlockedVault};
use zeroize::Zeroizing;

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

/// 静态来源 `secrets` 段：一条数据库凭据的字段明文（账号 + 口令样本）。值是机密，
/// 经 `Zeroizing` 入 payload。字段名 `user`/`password` 非敏感；值才是凭据明文。
const DB_USER_VALUE: &str = "ro";
const DB_PASSWORD_VALUE: &str = "s3cr3t-ro-pw";
/// 该凭据物化后预期的 `material` 明文（F-3 成功路径精确钉死值）：字段按 `BTreeMap` 序
/// （`password` < `user`）拼成 `field=value`、以 `;` 连接。这是静态来源唯一确定的物化形态。
const DB_MATERIAL: &str = "password=s3cr3t-ro-pw;user=ro";

/// `secrets` 段第二条凭据（不同 tier，验 (res,tier) 各自查表互不串扰）。
const DB_ADMIN_USER_VALUE: &str = "admin";
const DB_ADMIN_PASSWORD_VALUE: &str = "adm1n-pw";
const DB_ADMIN_MATERIAL: &str = "password=adm1n-pw;user=admin";

/// L-11：错误/成功路径返回串里绝不能出现的敏感子串（账号/口令/真实地址明文样本）。
/// 本数组本身是测试夹具，受全波次雷区约束——不含任何裸数据库写标记。
const FORBIDDEN_SUBSTRINGS: &[&str] = &[
    DB_PASSWORD_VALUE,
    DB_ADMIN_PASSWORD_VALUE,
    "ro",
    "admin",
    SESSION_VALUE_FRESH,
    SESSION_VALUE_RENEWED,
    VAULT_PASSWORD,
];

// ── 会话来源样本 ──────────────────────────────────────────────────────────

/// 一条"已登录"的活跃会话令牌明文（L-8 命中复用取证）。
const SESSION_VALUE_FRESH: &str = "sess-token-fresh-aaa";
/// 续会话成功后取得的新会话令牌明文（L-7 续期回填取证）。
const SESSION_VALUE_RENEWED: &str = "sess-token-renewed-bbb";
/// 续期提前量（毫秒）：`now ≥ expiry − skew` 即续。
const SKEW_MILLIS: u64 = 1_000;
/// vault 账号密码档的口令样本（①档重登所用持久凭据；绝不出现在任何返回串，L-11）。
const VAULT_PASSWORD: &str = "vault-acct-pw-zzz";

// ════════════════════════════════════════════════════════════════════════
//  静态来源夹具（端到端封 vault → unlock 句柄）
// ════════════════════════════════════════════════════════════════════════

/// 把一组 `(引用键 → 字段映射)` 写成 payload 的 `secrets` 段形态。
/// 叶子明文值入 `Zeroizing<String>`（机密材料纪律）。
fn secret_section(
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

/// 端到端把一个两段 payload 封进合法 vault 字节，经 `vault::unlock` 还原成
/// `UnlockedVault` 句柄。用 **KeyFile 来源**（直接持有 32B 主密钥，无 KDF），避开 argon2id。
fn unlocked_with_secrets(
    secrets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>>,
) -> UnlockedVault {
    // targets 段给一条最小条目，满足 payload 两段齐全；其字段不触任何裸数据库写标记。
    let targets = {
        let mut t: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
        let mut entry: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
        entry.insert("host".to_string(), Zeroizing::new("10.0.3.17".to_string()));
        entry.insert("port".to_string(), Zeroizing::new("5432".to_string()));
        t.insert("db-main".to_string(), entry);
        t
    };

    let payload = Payload::from_sections(secrets, targets);
    let plaintext = payload
        .to_plaintext()
        .expect("serialize payload to JSON plaintext");

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

    let bytes = vf.encode();
    vault::unlock(&MASTER_KEY, &bytes).expect("KeyFile-source vault must unlock with master key")
}

/// 一个解锁句柄：`secrets` 含两条凭据——`vault://db-main/readonly`（ro）与
/// `vault://db-main/admin`（admin），验 (res,tier) 各自查表。
fn unlocked_two_creds() -> UnlockedVault {
    unlocked_with_secrets(secret_section(&[
        (
            "vault://db-main/readonly",
            &[("user", DB_USER_VALUE), ("password", DB_PASSWORD_VALUE)],
        ),
        (
            "vault://db-main/admin",
            &[
                ("user", DB_ADMIN_USER_VALUE),
                ("password", DB_ADMIN_PASSWORD_VALUE),
            ],
        ),
    ]))
}

// ════════════════════════════════════════════════════════════════════════
//  零依赖单线程 block_on 执行器（本 crate 不依赖 runtime，§3.1）
// ════════════════════════════════════════════════════════════════════════

/// 单线程驱动一个 future 到完成（busy-poll，无定时器需求）。`Box::pin` 安全堆 pin，
/// 不用 `unsafe`。no-op waker 取 `std::task::Waker::noop()`（标准库提供的安全空 waker，
/// 不触 `unsafe`，B-6）；续会话经 Fake 端点同步完成，无真实 IO，反复 `poll` 直至 `Ready`。
fn block_on<F: Future>(fut: F) -> F::Output {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut fut = Box::pin(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            // Fake 端点同步完成；本测试编排不产生真实挂起，立即再 poll。
            Poll::Pending => continue,
        }
    }
}

/// 让出一次执行权的 future：首 `poll` 唤醒自身并 `Pending`，次 `poll` `Ready`。
/// 用于让 leader 的续会话在途时把执行权交还驱动器（建模并发在途重叠），从而让另一并发
/// future 得以被交错驱动越过缓存缺失检查——这是证伪"无单飞"实现的必要交错。
struct YieldOnceFut {
    yielded: bool,
}

impl Future for YieldOnceFut {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// **交错**驱动两个 future 到完成（建模同一 `(res,tier)` 的两个并发请求）。
/// 关键：每轮**先 poll a 再 poll b**，使两者在各自首个挂起点同时在途——
/// 与串行 `a.await; b.await`（a 跑完整路径后 b 才开始）截然不同。配合先挂起一次再完成的
/// 续会话 future，leader 在途时 follower 必被驱动越过缓存缺失检查：单飞实现下 follower 不
/// 再调 renew，无单飞实现下 follower 各自调 renew——本驱动据此让 L-9 断言可证伪"无单飞"。
fn drive_two<FA, FB>(fut_a: FA, fut_b: FB) -> (FA::Output, FB::Output)
where
    FA: Future,
    FB: Future,
{
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut fut_a = Box::pin(fut_a);
    let mut fut_b = Box::pin(fut_b);
    let mut out_a: Option<FA::Output> = None;
    let mut out_b: Option<FB::Output> = None;
    // 有界自旋（防止任一实现死循环时挂死测试）；两 future 各至多数次让出即完成。
    for _ in 0..10_000 {
        if out_a.is_none() {
            if let Poll::Ready(v) = fut_a.as_mut().poll(&mut cx) {
                out_a = Some(v);
            }
        }
        if out_b.is_none() {
            if let Poll::Ready(v) = fut_b.as_mut().poll(&mut cx) {
                out_b = Some(v);
            }
        }
        if out_a.is_some() && out_b.is_some() {
            // 两者皆 `Ready`，`take` 取出（避免 `is_some` 后 `unwrap`）。
            match (out_a.take(), out_b.take()) {
                (Some(a), Some(b)) => return (a, b),
                _ => unreachable!("both outputs are Some in this branch"),
            }
        }
    }
    panic!("drive_two did not complete both futures within the poll budget");
}

/// 把一个 `Result<ResourceCredential, CredentialError>` 的 `Ok` 取出。`ResourceCredential`
/// `Debug=REDACTED`，不对其 `unwrap`；失败侧单独 `match` 给出脱敏信息。
fn expect_cred(res: Result<ResourceCredential, CredentialError>) -> ResourceCredential {
    match res {
        Ok(c) => c,
        Err(e) => panic!("expected credential_for to succeed, but it failed with {e:?}"),
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake 时钟（注入时间源，可推进）
// ════════════════════════════════════════════════════════════════════════

/// 可推进的 Fake 墙钟：续期判定只读它，使 L-7/L-8 的"临近过期 / 未临近过期"确定可控。
struct FakeClock {
    now: AtomicU64,
}

impl FakeClock {
    fn at(millis: u64) -> Self {
        Self {
            now: AtomicU64::new(millis),
        }
    }
}

impl Clock for FakeClock {
    fn now_millis(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fake 认证端点（注入续会话执行者，可编排成功/失败/需交互 + 记登录次数）
// ════════════════════════════════════════════════════════════════════════

/// 端点编排脚本：决定下一次 `renew` 的结果。
enum AuthScript {
    /// 续会话成功，返回固定新会话令牌 + 给定新过期墙钟。
    Succeed { new_expiry: u64 },
    /// 续会话成功，但 future **先挂起一次再完成**（`Poll::Pending`→`Ready`）。
    /// 用于 L-9 单飞观察：使 leader 的续会话在途时另一并发等待者得以被驱动越过缓存缺失
    /// 检查——单飞实现下 follower 不应再调 renew（计数恒 1），无单飞实现下 follower 会各自
    /// 调 renew（计数 2），由此该测试可证伪"无单飞"实现。
    SucceedAfterYield { new_expiry: u64 },
    /// 续会话失败（账号密码/refresh 失效）→ `RefreshFailed`。
    Fail,
    /// 系统强制每次 2FA、无长效会话 → `InteractiveAuthRequired`（仅 ③ 档）。
    RequireInteractive,
}

/// Fake 认证端点：记录 `renew` 被调次数（L-9 单飞观察），按脚本返回结果。
/// `Send + Sync`，但本测试单线程驱动；登录计数用 `AtomicU64`。
struct FakeAuthority {
    /// 累计 `renew` 调用次数——L-9 钉死"至多一次在途登录/刷新"。
    renew_calls: AtomicU64,
    /// 下一次 `renew` 的编排结果。
    script: AuthScript,
}

impl FakeAuthority {
    fn succeeding(new_expiry: u64) -> Self {
        Self {
            renew_calls: AtomicU64::new(0),
            script: AuthScript::Succeed { new_expiry },
        }
    }
    fn succeeding_after_yield(new_expiry: u64) -> Self {
        Self {
            renew_calls: AtomicU64::new(0),
            script: AuthScript::SucceedAfterYield { new_expiry },
        }
    }
    fn failing() -> Self {
        Self {
            renew_calls: AtomicU64::new(0),
            script: AuthScript::Fail,
        }
    }
    fn requiring_interactive() -> Self {
        Self {
            renew_calls: AtomicU64::new(0),
            script: AuthScript::RequireInteractive,
        }
    }
    fn calls(&self) -> u64 {
        self.renew_calls.load(Ordering::SeqCst)
    }
}

impl SessionAuthority for FakeAuthority {
    fn renew<'a>(
        &'a self,
        _res: &'a ResourceCode,
        _tier: &'a CredentialTier,
        _form: SessionForm,
    ) -> Pin<Box<dyn Future<Output = Result<RenewedSession, CredentialError>> + Send + 'a>> {
        Box::pin(async move {
            // 计数在 future **首 poll** 即自增——即"本任务确实发起了一次续会话/登录"。
            // 单飞观察据此：leader 调一次→1；follower 不应再调（计数仍 1）。
            self.renew_calls.fetch_add(1, Ordering::SeqCst);
            match &self.script {
                AuthScript::Succeed { new_expiry } => Ok(RenewedSession {
                    session_value: Zeroizing::new(SESSION_VALUE_RENEWED.to_string()),
                    expiry_millis: *new_expiry,
                }),
                AuthScript::SucceedAfterYield { new_expiry } => {
                    // 先挂起一次再完成：leader 续会话"在途"期间把执行权交还驱动器，
                    // 使并发等待者得以被驱动越过缓存缺失检查（建模真实在途重叠）。
                    YieldOnceFut { yielded: false }.await;
                    Ok(RenewedSession {
                        session_value: Zeroizing::new(SESSION_VALUE_RENEWED.to_string()),
                        expiry_millis: *new_expiry,
                    })
                }
                AuthScript::Fail => Err(CredentialError::RefreshFailed),
                AuthScript::RequireInteractive => Err(CredentialError::InteractiveAuthRequired),
            }
        })
    }
}

/// 构造一个账号密码档（①）会话来源，预置一条活跃会话（已登录稳态）。
fn password_provider_seeded(
    authority: FakeAuthority,
    clock: FakeClock,
    seed_expiry: u64,
) -> LiveSessionProvider<FakeAuthority, FakeClock> {
    let p = LiveSessionProvider::new(SessionForm::PasswordSession, SKEW_MILLIS, authority, clock);
    p.seed_session(
        ResourceCode::new("svc-x"),
        CredentialTier::new("app"),
        Zeroizing::new(SESSION_VALUE_FRESH.to_string()),
        seed_expiry,
    );
    p
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-3 / F-5 / L-5 / L-11：静态来源 credential_for（按 (res,tier)）
// ════════════════════════════════════════════════════════════════════════

/// §8 F-3：存在的 `(res, tier)` → 返回 `ResourceCredential`，其 `material` 恰为该引用键下
/// 字段映射的确定物化串。精确钉死物化值，禁弱断言。
#[test]
fn static_existing_res_tier_materializes_exact_credential_material() {
    let vault = unlocked_two_creds();
    let provider = StaticVaultProvider::new(&vault);
    let cred = expect_cred(block_on(provider.credential_for(
        &ResourceCode::new("db-main"),
        &CredentialTier::new("readonly"),
    )));
    assert_eq!(
        cred.material, DB_MATERIAL,
        "static source must materialize the secrets-section fields into the exact canonical material"
    );
}

/// §8 F-3：不同 `(res, tier)` 各自查 `vault://<code>/<tier>` 键，互不串扰——同句柄上
/// `admin` tier 物化出 admin 凭据、`readonly` 物化出 ro 凭据，两者不同。
#[test]
fn static_distinct_tiers_materialize_distinct_credentials() {
    let vault = unlocked_two_creds();
    let provider = StaticVaultProvider::new(&vault);
    let ro = expect_cred(block_on(provider.credential_for(
        &ResourceCode::new("db-main"),
        &CredentialTier::new("readonly"),
    )));
    let admin = expect_cred(block_on(
        provider.credential_for(&ResourceCode::new("db-main"), &CredentialTier::new("admin")),
    ));
    assert_eq!(
        ro.material, DB_MATERIAL,
        "readonly tier must materialize the ro credential"
    );
    assert_eq!(
        admin.material, DB_ADMIN_MATERIAL,
        "admin tier must materialize the admin credential"
    );
    assert_ne!(
        ro.material, admin.material,
        "distinct tiers must materialize distinct credentials (no cross-tier bleed)"
    );
}

/// §8 F-5：机密类型唯一构造——`credential_for` 在本 crate 能产出 `ResourceCredential`
/// 实例（构造路径存在）。这里以"成功取得一个实例并读出其 `material`"取证构造点可达。
#[test]
fn static_credential_for_is_a_construction_path_for_resource_credential() {
    let vault = unlocked_two_creds();
    let provider = StaticVaultProvider::new(&vault);
    let cred = expect_cred(block_on(provider.credential_for(
        &ResourceCode::new("db-main"),
        &CredentialTier::new("readonly"),
    )));
    // 能读出本 crate 写入的 material 字面量，即证明本 crate 是其构造点（core 内无构造路径）。
    assert!(
        !cred.material.is_empty(),
        "credential_for must produce a constructed ResourceCredential with material set"
    );
}

/// §8 F-3 / L-5：不存在的 `tier`（句柄无 `vault://db-main/nope` 键）→
/// `Err(CredentialError::NotFound)`、不返回任何凭据。fail-closed，非缺省凭据。
#[test]
fn static_unknown_tier_is_not_found_with_no_credential() {
    let vault = unlocked_two_creds();
    let provider = StaticVaultProvider::new(&vault);
    match block_on(
        provider.credential_for(&ResourceCode::new("db-main"), &CredentialTier::new("nope")),
    ) {
        Err(CredentialError::NotFound) => {}
        Err(other) => panic!("unknown tier must be NotFound, got {other:?}"),
        Ok(_) => panic!("unknown tier must NOT return any credential (fail-closed, no default)"),
    }
}

/// §8 F-3 / L-5：不存在的 `res`（句柄无 `vault://other/readonly` 键）→
/// `Err(CredentialError::NotFound)`、无产物。
#[test]
fn static_unknown_resource_is_not_found_with_no_credential() {
    let vault = unlocked_two_creds();
    let provider = StaticVaultProvider::new(&vault);
    match block_on(provider.credential_for(
        &ResourceCode::new("other"),
        &CredentialTier::new("readonly"),
    )) {
        Err(CredentialError::NotFound) => {}
        Err(other) => panic!("unknown resource must be NotFound, got {other:?}"),
        Ok(_) => panic!("unknown resource must NOT return any credential (fail-closed)"),
    }
}

/// §8 L-5：`secrets` 段为空 → 任一 `(res, tier)` 均 `NotFound`，签名层无"缺省凭据"路径。
#[test]
fn static_empty_secrets_section_fails_closed_not_found() {
    let vault = unlocked_with_secrets(BTreeMap::new());
    let provider = StaticVaultProvider::new(&vault);
    match block_on(provider.credential_for(
        &ResourceCode::new("db-main"),
        &CredentialTier::new("readonly"),
    )) {
        Err(CredentialError::NotFound) => {}
        Err(other) => panic!("empty secrets section must yield NotFound, got {other:?}"),
        Ok(_) => panic!("empty secrets section must never yield a default credential"),
    }
}

/// §8 L-11：`NotFound` 错误的脱敏文案绝不内插 res / tier / 账号明文——其 `Display`
/// 恒为 core 常量英文码，且不含任何敏感子串。
#[test]
fn static_not_found_error_carries_no_plaintext() {
    let vault = unlocked_two_creds();
    let provider = StaticVaultProvider::new(&vault);
    let err = match block_on(provider.credential_for(
        &ResourceCode::new("secret-res"),
        &CredentialTier::new("secret-tier"),
    )) {
        Err(e) => e,
        Ok(_) => panic!("unknown (res,tier) must fail"),
    };
    let text = err.to_string();
    assert_eq!(
        text, "no credential for requested resource and tier",
        "NotFound Display must be the core constant code, no interpolation"
    );
    assert!(
        !text.contains("secret-res") && !text.contains("secret-tier"),
        "error text must not interpolate the requested res/tier"
    );
}

/// §8 L-11：成功路径的 `ResourceCredential` `Debug` 恒为 `REDACTED`——其格式化输出
/// 不含任何凭据明文（账号/口令样本），机密类型在类型层即不可被日志/trace 直接记录。
#[test]
fn static_resource_credential_debug_is_redacted_no_plaintext() {
    let vault = unlocked_two_creds();
    let provider = StaticVaultProvider::new(&vault);
    let cred = expect_cred(block_on(provider.credential_for(
        &ResourceCode::new("db-main"),
        &CredentialTier::new("readonly"),
    )));
    let dbg = format!("{cred:?}");
    assert_eq!(
        dbg, "REDACTED",
        "ResourceCredential Debug must be exactly REDACTED"
    );
    for needle in FORBIDDEN_SUBSTRINGS {
        assert!(
            !dbg.contains(needle),
            "ResourceCredential Debug must not leak plaintext substring {needle:?}"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-7 / L-8：会话来源——命中且未临近过期 → 复用缓存、不重登
// ════════════════════════════════════════════════════════════════════════

/// §8 L-8：缓存命中且 `now < expiry − skew` → 直接复用缓存会话，本次建连**不触发任何
/// 登录请求**。以 Fake 端点登录计数恒为 0 取证"不重登"。
#[test]
fn session_cache_hit_not_near_expiry_reuses_without_relogin() {
    // expiry=10_000、skew=1_000 → 续期阈值 9_000；now=5_000 远未临近过期。
    let authority = FakeAuthority::succeeding(20_000);
    let provider = password_provider_seeded(authority, FakeClock::at(5_000), 10_000);
    let cred = expect_cred(block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ));
    // 复用缓存会话 → material 来自已存活跃会话令牌，且续会话端点一次都没被调。
    assert_eq!(
        cred.material, SESSION_VALUE_FRESH,
        "cache hit must reuse the existing live session value verbatim"
    );
    assert_eq!(
        provider_authority_calls(&provider),
        0,
        "cache hit (not near expiry) must NOT trigger any login/refresh (L-8)"
    );
}

/// §8 F-7 ①档 / L-7：缓存临近过期（`now ≥ expiry − skew`）→ 用 vault 账号密码**无人值守
/// 重登**续会话成功并回填，全程无人交互。续会话端点恰被调 1 次，物化值为新会话令牌。
#[test]
fn session_near_expiry_password_tier_relogins_unattended_and_refills() {
    // expiry=10_000、skew=1_000 → 阈值 9_000；now=9_500 已临近过期 → 触发重登。
    let authority = FakeAuthority::succeeding(20_000);
    let provider = password_provider_seeded(authority, FakeClock::at(9_500), 10_000);
    let cred = expect_cred(block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ));
    assert_eq!(
        cred.material, SESSION_VALUE_RENEWED,
        "near-expiry must re-login and materialize the renewed session value (L-7)"
    );
    assert_eq!(
        provider_authority_calls(&provider),
        1,
        "near-expiry password tier must re-login exactly once, unattended (L-7)"
    );
}

/// §8 L-7：续会话成功后**回填缓存**——重登一次后，对同键再次解析落在新过期内、复用
/// 回填的新会话，端点**不再被调**（累计仍为 1）。证明续期产物已写回缓存。
#[test]
fn session_renewal_refills_cache_so_next_resolve_reuses() {
    let authority = FakeAuthority::succeeding(20_000);
    let provider = password_provider_seeded(authority, FakeClock::at(9_500), 10_000);
    // 第一次：临近过期 → 重登（端点 +1），回填 expiry=20_000。
    let _ = expect_cred(block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ));
    // now=9_500 仍 < 20_000 − 1_000=19_000 → 第二次命中复用回填会话，端点不再调。
    let again = expect_cred(block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ));
    assert_eq!(
        again.material, SESSION_VALUE_RENEWED,
        "second resolve must reuse the refilled renewed session"
    );
    assert_eq!(
        provider_authority_calls(&provider),
        1,
        "renewal must refill cache so a subsequent resolve does NOT re-login again"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-7 ②档：API token（长效）——命中即直接取用，不触发 skew 续期
// ════════════════════════════════════════════════════════════════════════

/// 构造一个 API token 档（②）会话来源，预置一条 token 条目。
fn apitoken_provider_seeded(
    authority: FakeAuthority,
    clock: FakeClock,
    seed_expiry: u64,
) -> LiveSessionProvider<FakeAuthority, FakeClock> {
    let p = LiveSessionProvider::new(SessionForm::ApiToken, SKEW_MILLIS, authority, clock);
    p.seed_session(
        ResourceCode::new("svc-x"),
        CredentialTier::new("app"),
        Zeroizing::new(SESSION_VALUE_FRESH.to_string()),
        seed_expiry,
    );
    p
}

/// §8 F-7 ②档（`SessionForm::ApiToken`）：长效 token 命中 → **直接取用**，即便墙钟已越过
/// ①/③ 的 `expiry − skew` 续期阈值，②档也**不触发续会话**（②无运行期续期机制——这是与
/// ①/③ skew 续期判定相异、专属第二档的解析路径，钉死 F-7「三档各自存在解析路径」之②档）。
/// 取证：物化值为预置 token、续会话端点登录计数恒 0。
#[test]
fn session_apitoken_tier_reuses_long_lived_token_without_renewal() {
    // expiry=10_000、skew=1_000 → ①/③ 阈值 9_000；now=9_500 已越阈值。①档此刻会续会话，
    // 但②档（ApiToken）长效、直接取用，不调端点。端点编排为失败以反证"根本没去续"。
    let authority = FakeAuthority::failing();
    let provider = apitoken_provider_seeded(authority, FakeClock::at(9_500), 10_000);
    let cred = expect_cred(block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ));
    assert_eq!(
        cred.material, SESSION_VALUE_FRESH,
        "ApiToken tier must take the long-lived token directly (verbatim), even past the skew threshold"
    );
    assert_eq!(
        provider_authority_calls(&provider),
        0,
        "ApiToken tier must NOT trigger any renewal even past expiry−skew (②档 long-lived, no skew-renew, F-7)"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-9：续会话单飞（同一 (res,tier) 至多一次在途登录）
// ════════════════════════════════════════════════════════════════════════

/// §8 L-9：同一 `(res, tier)` 在缓存缺失/临近过期下**真正并发**触发续会话 → **至多一次**
/// 在途登录/刷新，其余请求复用同一在途结果（无登录风暴）。
///
/// 证伪力（关键，针对此前同义反复的反例）：续会话 future 被编排为**先挂起一次再完成**
/// （`SucceedAfterYield`），且两 future 经 `drive_two` **交错**驱动（每轮先 poll a 再 poll b），
/// 而非串行 `a.await; b.await`。于是 leader（fut_a）在途续会话期间，follower（fut_b）必被
/// 驱动越过缓存缺失检查：
/// - **有单飞**：follower 见键在途即不再调 renew、让出复用 leader 回填产物 → 登录计数 **1**；
/// - **无单飞**：follower 自行调 renew → 登录计数 **2**。
///
/// 因此 `provider_authority_calls == 1` 对"零单飞原语"的实现**会失败**（计数为 2），
/// 此测试据此能证伪登录风暴缺陷，而非依赖串行回填缓存命中而恒绿。
#[test]
fn session_concurrent_renewal_is_single_flight_one_login() {
    // 无预置会话（缺失）→ 两个并发请求都需续期；单飞保证只登录一次。
    // 续会话 future 先挂起一次再完成，确保两请求在 leader 在途时同时在场。
    let authority = FakeAuthority::succeeding_after_yield(20_000);
    let provider = LiveSessionProvider::new(
        SessionForm::PasswordSession,
        SKEW_MILLIS,
        authority,
        FakeClock::at(5_000),
    );

    let res = ResourceCode::new("svc-x");
    let tier = CredentialTier::new("app");

    // 交错驱动两个续会话 future（先 poll a 再 poll b，建模真实并发在途重叠）。
    let fut_a = provider.credential_for(&res, &tier);
    let fut_b = provider.credential_for(&res, &tier);
    let (ra, rb) = drive_two(fut_a, fut_b);

    let ca = expect_cred(ra);
    let cb = expect_cred(rb);
    assert_eq!(
        ca.material, SESSION_VALUE_RENEWED,
        "first concurrent waiter must get the in-flight renewal product"
    );
    assert_eq!(
        cb.material, SESSION_VALUE_RENEWED,
        "second concurrent waiter must REUSE the same in-flight renewal product (not a fresh login)"
    );
    assert_eq!(
        provider_authority_calls(&provider),
        1,
        "concurrent renewal of one (res,tier) must trigger at most ONE login/refresh (single-flight, L-9): \
         a no-single-flight impl makes the in-flight follower issue its own renew → count==2 → this fails"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-10 / F-7 ③档：续会话不可建立 → fail-closed，不泄账号
// ════════════════════════════════════════════════════════════════════════

/// §8 L-10：续会话失败（账号密码失效）→ 该请求 `Err(CredentialError::RefreshFailed)`、
/// 不返回任何凭据；fail-closed，不在数据面静默重试。
#[test]
fn session_renewal_failure_is_refresh_failed_deny() {
    let authority = FakeAuthority::failing();
    let provider = password_provider_seeded(authority, FakeClock::at(9_500), 10_000);
    match block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ) {
        Err(CredentialError::RefreshFailed) => {}
        Err(other) => panic!("renewal failure must be RefreshFailed, got {other:?}"),
        Ok(_) => panic!("renewal failure must NOT return any credential (fail-closed deny)"),
    }
}

/// §8 F-7 ③档 / L-10：强制 2FA 且无长效会话 → `Err(CredentialError::InteractiveAuthRequired)`；
/// **运行期建连路径不触发 2FA**——续会话以该错误 deny，绝不在数据面发起交互。
#[test]
fn session_oauth_tier_forced_2fa_is_interactive_required_deny_not_2fa_trigger() {
    // ③档（OAuthRefresh）：端点编排为"需交互"，模拟系统强制每次 2FA、无长效会话。
    let authority = FakeAuthority::requiring_interactive();
    let provider = LiveSessionProvider::new(
        SessionForm::OAuthRefresh,
        SKEW_MILLIS,
        authority,
        FakeClock::at(9_500),
    );
    provider.seed_session(
        ResourceCode::new("svc-oauth"),
        CredentialTier::new("app"),
        Zeroizing::new(SESSION_VALUE_FRESH.to_string()),
        10_000,
    );
    match block_on(
        provider.credential_for(&ResourceCode::new("svc-oauth"), &CredentialTier::new("app")),
    ) {
        Err(CredentialError::InteractiveAuthRequired) => {}
        Err(other) => {
            panic!("forced-2fa-no-session must be InteractiveAuthRequired, got {other:?}")
        }
        Ok(_) => {
            panic!("forced-2fa path must deny at runtime, never trigger 2FA on the data plane")
        }
    }
}

/// §8 L-10 / L-11：续会话失败的错误文案绝不回吐账号明文——`RefreshFailed` 的 `Display`
/// 恒为 core 常量英文码，不含 vault 账号口令或任何敏感子串。
#[test]
fn session_refresh_failed_error_carries_no_account_plaintext() {
    let authority = FakeAuthority::failing();
    let provider = password_provider_seeded(authority, FakeClock::at(9_500), 10_000);
    let err = match block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ) {
        Err(e) => e,
        Ok(_) => panic!("renewal failure must fail"),
    };
    let text = err.to_string();
    assert_eq!(
        text, "credential refresh failed",
        "RefreshFailed Display must be the core constant code"
    );
    for needle in FORBIDDEN_SUBSTRINGS {
        assert!(
            !text.contains(needle),
            "refresh-failed error must not leak plaintext substring {needle:?}"
        );
    }
}

/// §8 L-8 → L-10 边界：缓存命中**未**临近过期时，即便端点被编排为失败也不被触发——
/// 复用缓存不依赖续会话，端点登录计数恒 0（命中复用与续期失败两路严格隔离）。
#[test]
fn session_cache_hit_does_not_consult_failing_authority() {
    let authority = FakeAuthority::failing();
    let provider = password_provider_seeded(authority, FakeClock::at(5_000), 10_000);
    let cred = expect_cred(block_on(
        provider.credential_for(&ResourceCode::new("svc-x"), &CredentialTier::new("app")),
    ));
    assert_eq!(
        cred.material, SESSION_VALUE_FRESH,
        "cache hit must reuse without consulting the (failing) authority"
    );
    assert_eq!(
        provider_authority_calls(&provider),
        0,
        "cache hit must not call the authority at all (so its failure is irrelevant)"
    );
}

// ── 测试内省：读出 provider 持有的 Fake 端点登录计数 ──────────────────────
//
// `LiveSessionProvider::authority()` 只读借出注入的端点；测试据此读其登录计数器，
// 用于 L-8/L-9 钉死"续会话被触发的次数"，无需在被测面引入测试专用入口。
fn provider_authority_calls(provider: &LiveSessionProvider<FakeAuthority, FakeClock>) -> u64 {
    provider.authority().calls()
}
