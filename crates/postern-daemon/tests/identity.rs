//! 身份与凭证域 8.4 —— `Authenticator` 实现的真断言（TDD：先红后绿）。
//!
//! 钉死 daemon 三族认证器（`local_process` / `api_key` / `token`）对 core
//! `Authenticator` trait 的实现行为：
//! - **api_key/token**：正确 key → 对应 principal；错 key → `Err`（不放行）；过期/吊销
//!   凭据 → `Err`@now；可信域不符 → `Err`；多候选确定性选取。
//! - **local_process**：正确 uid → principal；错 uid → `Err`；非 Unix peer 来源 → `Err`。
//! - **按 kind 分派**：`authenticator_registry()` 以 kind 为键正确装配三族。
//!
//! argon2 纪律（WSL 内存铁律）：测试夹具自造 `secret_hash` 时用**极小** argon2id 参数
//! （m_cost=8 KiB / t_cost=1 / p_cost=1），verify 据 stored PHC 串自身参数运行，秒级完成。
//! 本测试须以内存上限包裹运行（systemd-run --scope -p MemoryMax=4G）。
//!
//! 雷区纪律：测试在 shells 外，需要请求来源以 `use postern_core::request::ConnOrigin as
//! Origin` 别名构造（绝不写字面 ConnOrigin:: 变体）；零 SQL 标记；不构造机密类型。

use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

use postern_core::domain::{
    CredentialMeta, CredentialView, PresentedCredential, PrincipalId, Timestamp,
};
use postern_core::error::AuthError;
use postern_core::id::SnowflakeId;
use postern_core::plugin::Authenticator;
// 雷区 2：以别名构造请求来源，绝不写字面 ConnOrigin:: 变体（测试在 shells 外）。
use postern_core::request::ConnOrigin as Origin;

use postern_daemon::identity::local_process::LocalProcessAuthenticator;
use postern_daemon::identity::{self, api_key, local_process, token};

// ════════════════════════════════════════════════════════════════════════════
//  夹具：小参数 argon2id PHC 串、principal、时间、来源
// ════════════════════════════════════════════════════════════════════════════

/// 极小 argon2id 参数（WSL 内存铁律）：m_cost=8 KiB、t_cost=1、p_cost=1。verify 据
/// 此 stored 串运行，内存峰值 ~8 KiB、秒级完成。
fn tiny_argon2() -> Argon2<'static> {
    let params = Params::new(8, 1, 1, None).expect("tiny argon2 params valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// 以小参数 argon2id 把 secret 字节哈希成 PHC 串（夹具自造 stored hash 用）。
fn hash_secret(secret: &[u8]) -> String {
    // 固定 salt 文本→确定性夹具（测试可复现）；真实部署 salt 随机，本处仅夹具。
    let salt = SaltString::from_b64("ZGV0ZXJtaW5pc3RpY3NhbHQ").expect("salt b64 valid");
    tiny_argon2()
        .hash_password(secret, &salt)
        .expect("hash secret ok")
        .to_string()
}

fn principal(n: u64) -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(n))
}

fn now() -> Timestamp {
    Timestamp::from_unix_ms(1_700_000_000_000)
}

fn earlier() -> Timestamp {
    Timestamp::from_unix_ms(1_699_000_000_000)
}

fn later() -> Timestamp {
    Timestamp::from_unix_ms(1_701_000_000_000)
}

fn tcp_origin() -> Origin {
    Origin::Tcp {
        remote: "203.0.113.7:54321".parse().expect("addr parses"),
    }
}

fn unix_origin(uid: u32, gid: u32) -> Origin {
    Origin::UnixPeer { uid, gid }
}

/// 空 presented（local_process 零凭证用）。
fn empty_presented(kind: &str) -> PresentedCredential {
    PresentedCredential::new(kind, Vec::new())
}

fn view(credentials: Vec<CredentialMeta>) -> CredentialView {
    CredentialView { credentials }
}

// ════════════════════════════════════════════════════════════════════════════
//  api_key 认证器
// ════════════════════════════════════════════════════════════════════════════

const API_SECRET: &[u8] = b"correct-horse-battery-staple-api-key";
const WRONG_SECRET: &[u8] = b"definitely-not-the-right-key";

fn api_key_meta(p: u64) -> CredentialMeta {
    CredentialMeta {
        principal: principal(p),
        kind: api_key::KIND.to_string(),
        secret_hash: hash_secret(API_SECRET),
        expires_at: None,
        revoked_at: None,
    }
}

#[test]
fn api_key_correct_secret_resolves_to_principal() {
    let auth = api_key::authenticator();
    let creds = view(vec![api_key_meta(7)]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    let got = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect("correct api_key authenticates");
    assert_eq!(got, principal(7), "correct key → its principal");
}

#[test]
fn api_key_wrong_secret_is_denied() {
    let auth = api_key::authenticator();
    let creds = view(vec![api_key_meta(7)]);
    let presented = PresentedCredential::new(api_key::KIND, WRONG_SECRET.to_vec());

    let err = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect_err("wrong api_key must not authenticate");
    assert_eq!(
        err,
        AuthError::InvalidCredential,
        "wrong key → InvalidCredential, never an Ok principal"
    );
}

#[test]
fn api_key_expired_credential_is_denied_at_now() {
    let auth = api_key::authenticator();
    let mut meta = api_key_meta(7);
    // expires_at 早于 now → 过期（按 now 墙钟二次校验，即刻失效）。
    meta.expires_at = Some(earlier());
    let creds = view(vec![meta]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    let err = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect_err("expired credential must not authenticate");
    assert_eq!(
        err,
        AuthError::ExpiredCredential,
        "expires_at < now → ExpiredCredential"
    );
}

#[test]
fn api_key_not_yet_expired_credential_authenticates() {
    let auth = api_key::authenticator();
    let mut meta = api_key_meta(7);
    // expires_at 晚于 now → 仍在生命期内，正确 key 应过。
    meta.expires_at = Some(later());
    let creds = view(vec![meta]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    let got = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect("not-yet-expired credential authenticates");
    assert_eq!(got, principal(7));
}

#[test]
fn api_key_revoked_credential_is_denied_at_now() {
    let auth = api_key::authenticator();
    let mut meta = api_key_meta(7);
    // revoked_at <= now → 已吊销。
    meta.revoked_at = Some(earlier());
    let creds = view(vec![meta]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    let err = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect_err("revoked credential must not authenticate");
    assert_eq!(
        err,
        AuthError::RevokedCredential,
        "revoked_at <= now → RevokedCredential"
    );
}

#[test]
fn api_key_trust_domain_mismatch_is_denied() {
    let auth = api_key::authenticator();
    let creds = view(vec![api_key_meta(7)]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    // api_key 可信域为 Network（仅 TCP 相符）；Unix peer 观测来源 → 可信域不符。
    let err = auth
        .authenticate(&presented, &unix_origin(1000, 1000), &creds, now())
        .expect_err("api_key from non-network origin must be denied");
    assert_eq!(
        err,
        AuthError::TrustDomainMismatch,
        "unix-peer origin for network api_key → TrustDomainMismatch"
    );
}

#[test]
fn api_key_no_matching_kind_is_denied() {
    let auth = api_key::authenticator();
    // 视图里只有 token 凭据，无 api_key kind → 无候选。
    let token_meta = CredentialMeta {
        principal: principal(9),
        kind: token::KIND.to_string(),
        secret_hash: hash_secret(API_SECRET),
        expires_at: None,
        revoked_at: None,
    };
    let creds = view(vec![token_meta]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    let err = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect_err("no api_key candidate → deny");
    assert_eq!(err, AuthError::InvalidCredential);
}

#[test]
fn api_key_multi_candidate_selects_matching_secret_deterministically() {
    let auth = api_key::authenticator();
    // 三个 api_key 候选；只有中间那个 secret_hash 对应 API_SECRET。其余两个是别的 secret。
    let other_a = CredentialMeta {
        principal: principal(1),
        kind: api_key::KIND.to_string(),
        secret_hash: hash_secret(b"other-secret-a"),
        expires_at: None,
        revoked_at: None,
    };
    let mut wanted = api_key_meta(42);
    let other_b = CredentialMeta {
        principal: principal(3),
        kind: api_key::KIND.to_string(),
        secret_hash: hash_secret(b"other-secret-b"),
        expires_at: None,
        revoked_at: None,
    };
    // 固定不同 secret 的候选，确保只有 wanted 的 verify 通过。
    wanted.secret_hash = hash_secret(API_SECRET);
    let creds = view(vec![other_a, wanted, other_b]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    let got = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect("the one matching candidate authenticates");
    assert_eq!(
        got,
        principal(42),
        "deterministically selects the candidate whose hash verifies"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  token 认证器（同 api_key 形态，按 token kind）
// ════════════════════════════════════════════════════════════════════════════

const TOKEN_SECRET: &[u8] = b"a-bearer-token-value-9f3c";

fn token_meta(p: u64) -> CredentialMeta {
    CredentialMeta {
        principal: principal(p),
        kind: token::KIND.to_string(),
        secret_hash: hash_secret(TOKEN_SECRET),
        expires_at: None,
        revoked_at: None,
    }
}

#[test]
fn token_correct_secret_resolves_to_principal() {
    let auth = token::authenticator();
    let creds = view(vec![token_meta(11)]);
    let presented = PresentedCredential::new(token::KIND, TOKEN_SECRET.to_vec());

    let got = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect("correct token authenticates");
    assert_eq!(got, principal(11));
}

#[test]
fn token_wrong_secret_is_denied() {
    let auth = token::authenticator();
    let creds = view(vec![token_meta(11)]);
    let presented = PresentedCredential::new(token::KIND, WRONG_SECRET.to_vec());

    let err = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect_err("wrong token must not authenticate");
    assert_eq!(err, AuthError::InvalidCredential);
}

#[test]
fn token_revoked_credential_is_denied_at_now() {
    let auth = token::authenticator();
    let mut meta = token_meta(11);
    meta.revoked_at = Some(earlier());
    let creds = view(vec![meta]);
    let presented = PresentedCredential::new(token::KIND, TOKEN_SECRET.to_vec());

    let err = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect_err("revoked token must not authenticate");
    assert_eq!(err, AuthError::RevokedCredential);
}

// ════════════════════════════════════════════════════════════════════════════
//  local_process 认证器（零凭证，据观测 uid/gid）
// ════════════════════════════════════════════════════════════════════════════

/// 构造一条 local_process 凭据：match 规则编码在 secret_hash 文本里。
fn local_meta(p: u64, rule: &str) -> CredentialMeta {
    CredentialMeta {
        principal: principal(p),
        kind: local_process::KIND.to_string(),
        secret_hash: rule.to_string(),
        expires_at: None,
        revoked_at: None,
    }
}

#[test]
fn local_process_matching_uid_resolves_to_principal() {
    let auth = LocalProcessAuthenticator;
    let creds = view(vec![local_meta(5, "uid=1000,gid=1000")]);

    let got = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &unix_origin(1000, 1000),
            &creds,
            now(),
        )
        .expect("matching uid/gid authenticates");
    assert_eq!(got, principal(5), "matching local peer → its principal");
}

#[test]
fn local_process_wrong_uid_is_denied() {
    let auth = LocalProcessAuthenticator;
    let creds = view(vec![local_meta(5, "uid=1000,gid=1000")]);

    // 观测 uid 不等规则 uid → 无匹配 → deny。
    let err = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &unix_origin(1001, 1000),
            &creds,
            now(),
        )
        .expect_err("wrong uid must not authenticate");
    assert_eq!(err, AuthError::InvalidCredential);
}

#[test]
fn local_process_wrong_gid_is_denied() {
    let auth = LocalProcessAuthenticator;
    let creds = view(vec![local_meta(5, "uid=1000,gid=1000")]);

    let err = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &unix_origin(1000, 9999),
            &creds,
            now(),
        )
        .expect_err("wrong gid must not authenticate");
    assert_eq!(err, AuthError::InvalidCredential);
}

#[test]
fn local_process_uid_only_rule_ignores_gid() {
    let auth = LocalProcessAuthenticator;
    // 规则只约束 uid → 任意 gid 均可。
    let creds = view(vec![local_meta(5, "uid=1000")]);

    let got = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &unix_origin(1000, 4242),
            &creds,
            now(),
        )
        .expect("uid-only rule ignores gid");
    assert_eq!(got, principal(5));
}

#[test]
fn local_process_tcp_origin_is_undeterminable() {
    let auth = LocalProcessAuthenticator;
    let creds = view(vec![local_meta(5, "uid=1000")]);

    // 非 Unix peer 来源无法取 SO_PEERCRED 信任域门 → UndeterminableOrigin。
    let err = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &tcp_origin(),
            &creds,
            now(),
        )
        .expect_err("tcp origin cannot do local_process");
    assert_eq!(err, AuthError::UndeterminableOrigin);
}

#[test]
fn local_process_expired_credential_is_denied_at_now() {
    let auth = LocalProcessAuthenticator;
    let mut meta = local_meta(5, "uid=1000");
    meta.expires_at = Some(earlier());
    let creds = view(vec![meta]);

    let err = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &unix_origin(1000, 1000),
            &creds,
            now(),
        )
        .expect_err("expired local_process credential denied");
    assert_eq!(err, AuthError::ExpiredCredential);
}

#[test]
fn local_process_multi_candidate_selects_first_match_deterministically() {
    let auth = LocalProcessAuthenticator;
    // 两条都匹配 uid=1000 的候选；按固定顺序取首个 → principal 30。
    let creds = view(vec![
        local_meta(30, "uid=1000"),
        local_meta(31, "uid=1000"),
    ]);

    let got = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &unix_origin(1000, 1000),
            &creds,
            now(),
        )
        .expect("first matching candidate selected");
    assert_eq!(
        got,
        principal(30),
        "deterministic: first matching candidate in view order"
    );
}

#[test]
fn local_process_malformed_rule_does_not_grant() {
    let auth = LocalProcessAuthenticator;
    // 规则非法（缺 uid）→ 该候选不匹配 → 无候选 → deny（绝不无约束放行）。
    let creds = view(vec![local_meta(5, "gid=1000")]);

    let err = auth
        .authenticate(
            &empty_presented(local_process::KIND),
            &unix_origin(1000, 1000),
            &creds,
            now(),
        )
        .expect_err("malformed rule must not grant");
    assert_eq!(err, AuthError::InvalidCredential);
}

// ════════════════════════════════════════════════════════════════════════════
//  按 kind 分派：authenticator_registry() 装配三族
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn registry_registers_all_three_kinds_by_kind_key() {
    let registry = identity::authenticator_registry();
    assert_eq!(registry.len(), 3, "exactly three authenticator kinds");

    // 每个键对应的认证器其 kind() 与键一致（kind 即注册键）。
    for kind in [local_process::KIND, api_key::KIND, token::KIND] {
        let auth = registry
            .get(kind)
            .unwrap_or_else(|| panic!("registry missing kind {kind}"));
        assert_eq!(auth.kind(), kind, "registry key matches authenticator kind");
    }
}

#[test]
fn registry_dispatches_api_key_to_a_working_authenticator() {
    let registry = identity::authenticator_registry();
    let creds = view(vec![api_key_meta(77)]);
    let presented = PresentedCredential::new(api_key::KIND, API_SECRET.to_vec());

    // 按 presented kind 在注册表选认证器并认证 → 正确 principal（端到端分派）。
    let auth = registry.get(presented.kind()).expect("api_key registered");
    let got = auth
        .authenticate(&presented, &tcp_origin(), &creds, now())
        .expect("dispatched api_key authenticates");
    assert_eq!(got, principal(77));
}

#[test]
fn registry_dispatches_local_process_to_a_working_authenticator() {
    let registry = identity::authenticator_registry();
    let creds = view(vec![local_meta(88, "uid=1000")]);
    let presented = empty_presented(local_process::KIND);

    let auth = registry
        .get(presented.kind())
        .expect("local_process registered");
    let got = auth
        .authenticate(&presented, &unix_origin(1000, 1000), &creds, now())
        .expect("dispatched local_process authenticates");
    assert_eq!(got, principal(88));
}
