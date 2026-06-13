//! 映射单元 `postern_secrets::mapping` 行为测试（RED）。
//!
//! 钉死 §8 F-4（地址解析）、F-5（机密类型唯一构造）、L-5（配置缺失→fail-closed）、
//! L-11（明文不出边界）。每条只钉一个行为，断言精确到具体值 / 变体 / 错误字段。
//!
//! 接口（签名权威：模块文档 §5.4 与详细设计 4.3）：
//! `resolve(code: &ResourceCode) -> Result<ResolvedTarget, ResolveError>`，挂在已
//! 解锁保险箱句柄上（`impl UnlockedVault`）。物化规则（详细设计 4.3 `targets` 形态）：
//! `{host, port}` → `host:port`；`{instance_id, region}` → `instance_id@region`。
//!
//! 夹具纪律：用**直接持有型 32B 主密钥**（KeyFile 来源）经 `vault::unlock` 构造句柄，
//! 避开 passphrase argon2id KDF 路径——本单元纯内存查表，夹具不跑 KDF。所有真实地址
//! 样本只在 `Zeroizing` 内入 payload；文本不出现任何裸数据库写标记。

use std::collections::BTreeMap;

use postern_core::domain::{ResolvedTarget, ResourceCode};
use postern_secrets::error::ResolveError;
use postern_secrets::vault::crypto;
use postern_secrets::vault::format::FORMAT_VERSION;
use postern_secrets::vault::format::{VaultFile, NONCE_LEN};
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;
use postern_secrets::vault::{self, UnlockedVault};
use zeroize::Zeroizing;

// ── 固定测试材料（可控、确定，不碰真实来源） ──────────────────────────────

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

/// `targets` 段真实地址样本（明文，host:port 形态的端点物化取证）。
const HOST_VALUE: &str = "10.0.3.17";
const PORT_VALUE: &str = "5432";
/// `host:port` 物化后的预期端点明文（F-4 成功路径精确钉死值）。
const HOST_PORT_ENDPOINT: &str = "10.0.3.17:5432";

/// `targets` 段云实例样本（`{instance_id, region}` 形态）。
const INSTANCE_ID_VALUE: &str = "i-0abc1234def";
const REGION_VALUE: &str = "us-west-2";
/// `instance_id@region` 物化后的预期端点明文。
const INSTANCE_REGION_ENDPOINT: &str = "i-0abc1234def@us-west-2";

// ── 夹具构造 ──────────────────────────────────────────────────────────────

/// 把一段 `(代号 → 字段映射)` 写成 payload 的 `targets` 段形态。
/// 叶子明文值入 `Zeroizing<String>`（机密材料纪律）。
fn target_section(
    code: &str,
    fields: &[(&str, &str)],
) -> BTreeMap<String, BTreeMap<String, Zeroizing<String>>> {
    let mut section: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut entry: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
    for (k, v) in fields {
        entry.insert((*k).to_string(), Zeroizing::new((*v).to_string()));
    }
    section.insert(code.to_string(), entry);
    section
}

/// 端到端把一个两段 payload 封装进合法 vault 字节，再经 `vault::unlock` 还原成
/// `UnlockedVault` 句柄——句柄是 `resolve` 的唯一可用入口。用 **KeyFile 来源**
/// （直接持有 32B 主密钥，无 KDF 参数），避开 argon2id。
fn unlocked_with_targets(
    targets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>>,
) -> UnlockedVault {
    // secrets 段给一条最小条目，满足 payload 两段齐全；其字段不触任何裸数据库写标记。
    let mut secrets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut ro: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
    ro.insert("user".to_string(), Zeroizing::new("ro".to_string()));
    secrets.insert("db-main/readonly".to_string(), ro);

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

/// 一个解锁句柄：`targets` 含 `db-main` ({host, port}) 与 `app-prod`
/// ({instance_id, region}) 两条代号。
fn unlocked_two_codes() -> UnlockedVault {
    let mut targets = target_section("db-main", &[("host", HOST_VALUE), ("port", PORT_VALUE)]);
    targets.extend(target_section(
        "app-prod",
        &[("instance_id", INSTANCE_ID_VALUE), ("region", REGION_VALUE)],
    ));
    unlocked_with_targets(targets)
}

/// 从一个 `Result<ResolvedTarget, ResolveError>` 取出 `Ok`，断言其失败信息——
/// `ResolvedTarget` 不实现 `Debug` 的 `Ok` 侧不能用 `unwrap`（`Debug=REDACTED` 由
/// core 提供，但语义上不暴露内容），故对失败侧单独 `match`。
fn expect_resolved(res: Result<ResolvedTarget, ResolveError>) -> ResolvedTarget {
    match res {
        Ok(t) => t,
        Err(e) => panic!("expected resolve to succeed, but it failed with {e:?}"),
    }
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-4：地址解析——存在的 code → ResolvedTarget（物化端点精确）
// ════════════════════════════════════════════════════════════════════════

/// §8 F-4：存在的 `code`（`{host, port}` 形态）→ 返回 `ResolvedTarget`，其 `endpoint`
/// 恰为 `host:port` 物化串。精确钉死物化值，禁弱断言。
#[test]
fn resolve_known_host_port_code_materializes_host_colon_port_endpoint() {
    let vault = unlocked_two_codes();
    let target = expect_resolved(vault.resolve(&ResourceCode::new("db-main")));
    assert_eq!(
        target.endpoint, HOST_PORT_ENDPOINT,
        "resolve must materialize {{host, port}} into exactly host:port"
    );
}

/// §8 F-4：存在的 `code`（`{instance_id, region}` 形态）→ `ResolvedTarget`，其
/// `endpoint` 恰为 `instance_id@region` 物化串。
#[test]
fn resolve_known_instance_region_code_materializes_instance_at_region_endpoint() {
    let vault = unlocked_two_codes();
    let target = expect_resolved(vault.resolve(&ResourceCode::new("app-prod")));
    assert_eq!(
        target.endpoint, INSTANCE_REGION_ENDPOINT,
        "resolve must materialize {{instance_id, region}} into exactly instance_id@region"
    );
}

/// §8 F-4：同一句柄上多代号各自解析互不串扰——两个代号物化出各自端点，
/// 解析是纯查表、无跨条目污染。
#[test]
fn resolve_distinct_codes_yield_distinct_endpoints_on_same_handle() {
    let vault = unlocked_two_codes();
    let db = expect_resolved(vault.resolve(&ResourceCode::new("db-main")));
    let app = expect_resolved(vault.resolve(&ResourceCode::new("app-prod")));
    assert_eq!(
        db.endpoint, HOST_PORT_ENDPOINT,
        "db-main endpoint must be host:port"
    );
    assert_eq!(
        app.endpoint, INSTANCE_REGION_ENDPOINT,
        "app-prod endpoint must be instance_id@region"
    );
    assert_ne!(
        db.endpoint, app.endpoint,
        "distinct codes must materialize distinct endpoints (no cross-entry bleed)"
    );
}

/// §8 F-4：解析是**纯查表、无状态**——对同一 code 连续两次解析得**相同**端点，
/// 不消耗 / 不改写句柄持有的 `targets`。
#[test]
fn resolve_is_pure_lookup_repeatable_for_same_code() {
    let vault = unlocked_two_codes();
    let first = expect_resolved(vault.resolve(&ResourceCode::new("db-main")));
    let second = expect_resolved(vault.resolve(&ResourceCode::new("db-main")));
    assert_eq!(
        first.endpoint, second.endpoint,
        "pure lookup must yield the same endpoint on repeated resolve of the same code"
    );
    assert_eq!(
        first.endpoint, HOST_PORT_ENDPOINT,
        "and that endpoint stays host:port"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-4 / L-5：未知 code → ResolveError::UnknownCode、无产物（fail-closed）
// ════════════════════════════════════════════════════════════════════════

/// §8 F-4 / L-5：未知 `code`（句柄 `targets` 段无此键）→ `Err(UnknownCode)`、无产物。
/// 签名层即无"缺省地址"返回路径——失败侧只有 `Err`、不存在缺省 `ResolvedTarget`。
#[test]
fn resolve_unknown_code_is_unknown_code_error_with_no_product() {
    let vault = unlocked_two_codes();
    let res = vault.resolve(&ResourceCode::new("does-not-exist"));
    match res {
        Ok(_) => panic!("unknown code must fail-closed, not return a default ResolvedTarget"),
        Err(e) => assert_eq!(
            e,
            ResolveError::UnknownCode,
            "unknown code must map to ResolveError::UnknownCode"
        ),
    }
}

/// §8 L-5：空 `targets` 段的句柄上解析任意 code → `Err(UnknownCode)`、无缺省产物
/// （配置缺失即拒，fail-closed）。
#[test]
fn resolve_on_empty_targets_section_fails_closed() {
    let vault = unlocked_with_targets(BTreeMap::new());
    let res = vault.resolve(&ResourceCode::new("db-main"));
    match res {
        Ok(_) => panic!("empty targets must yield no resolved address (fail-closed)"),
        Err(e) => assert_eq!(
            e,
            ResolveError::UnknownCode,
            "missing mapping must be UnknownCode"
        ),
    }
}

/// §8 F-4 / L-5：代号存在但字段不成形（既缺 `{host, port}` 又缺
/// `{instance_id, region}`）→ `Err(UnknownCode)`、绝不物化半截端点。
#[test]
fn resolve_code_with_unshaped_fields_fails_closed() {
    let targets = target_section("db-main", &[("note", "not-an-address")]);
    let vault = unlocked_with_targets(targets);
    let res = vault.resolve(&ResourceCode::new("db-main"));
    match res {
        Ok(_) => panic!("a code whose fields form no known address shape must not resolve"),
        Err(e) => assert_eq!(
            e,
            ResolveError::UnknownCode,
            "unshaped target fields must fail-closed as UnknownCode, never a partial endpoint"
        ),
    }
}

/// §8 F-4 / L-5：`{host, port}` 形态但缺 `port` 单字段 → `Err(UnknownCode)`，
/// 绝不用单 `host` 物化出半截端点（边界越界即拒）。
#[test]
fn resolve_host_without_port_fails_closed() {
    let targets = target_section("db-main", &[("host", HOST_VALUE)]);
    let vault = unlocked_with_targets(targets);
    let res = vault.resolve(&ResourceCode::new("db-main"));
    match res {
        Ok(_) => panic!("host without port must not materialize a partial endpoint"),
        Err(e) => assert_eq!(
            e,
            ResolveError::UnknownCode,
            "incomplete host:port shape must fail-closed as UnknownCode"
        ),
    }
}

/// §8 F-4 / L-5：`{host, port}` 形态但缺 `host` 单字段（只有 `port`）→ `Err(UnknownCode)`，
/// 绝不用单 `port` 物化出半截端点。与 `host_without_port` 对称，钉死 host:port
/// 分支两侧的部分缺字段拒绝，杜绝任一字段单独触发物化。
#[test]
fn resolve_port_without_host_fails_closed() {
    let targets = target_section("db-main", &[("port", PORT_VALUE)]);
    let vault = unlocked_with_targets(targets);
    let res = vault.resolve(&ResourceCode::new("db-main"));
    match res {
        Ok(_) => panic!("port without host must not materialize a partial endpoint"),
        Err(e) => assert_eq!(
            e,
            ResolveError::UnknownCode,
            "incomplete host:port shape (port only) must fail-closed as UnknownCode"
        ),
    }
}

/// §8 F-4 / L-5：`{instance_id, region}` 形态但缺 `region` 单字段（只有 `instance_id`）
/// → `Err(UnknownCode)`，绝不用单 `instance_id` 物化出 `i-xxx@` 半截端点。
/// 钉死 instance 分支同样依赖两字段齐全——与 host:port 分支半截拒绝同等严格。
#[test]
fn resolve_instance_id_without_region_fails_closed() {
    let targets = target_section("app-prod", &[("instance_id", INSTANCE_ID_VALUE)]);
    let vault = unlocked_with_targets(targets);
    let res = vault.resolve(&ResourceCode::new("app-prod"));
    match res {
        Ok(_) => panic!("instance_id without region must not materialize a partial endpoint"),
        Err(e) => assert_eq!(
            e,
            ResolveError::UnknownCode,
            "incomplete instance_id@region shape (instance_id only) must fail-closed as UnknownCode"
        ),
    }
}

/// §8 F-4 / L-5：`{instance_id, region}` 形态但缺 `instance_id` 单字段（只有 `region`）
/// → `Err(UnknownCode)`，绝不用单 `region` 直出 `@us-west-2` / `us-west-2` 半截端点。
/// 与 `instance_id_without_region` 对称，钉死 instance 分支两侧的部分缺字段拒绝。
#[test]
fn resolve_region_without_instance_id_fails_closed() {
    let targets = target_section("app-prod", &[("region", REGION_VALUE)]);
    let vault = unlocked_with_targets(targets);
    let res = vault.resolve(&ResourceCode::new("app-prod"));
    match res {
        Ok(_) => panic!("region without instance_id must not materialize a partial endpoint"),
        Err(e) => assert_eq!(
            e,
            ResolveError::UnknownCode,
            "incomplete instance_id@region shape (region only) must fail-closed as UnknownCode"
        ),
    }
}

// ════════════════════════════════════════════════════════════════════════
//  §8 F-5：机密类型唯一构造——本 crate 内 resolve 能产出 ResolvedTarget 实例
// ════════════════════════════════════════════════════════════════════════

/// §8 F-5：本 crate 内存在 `ResolvedTarget` 的构造路径——`resolve` 对已存在代号
/// 产出一个实例（其 `Debug` 恒为 `REDACTED`，由 core 提供，本 crate 不另加 impl）。
/// 此测试存在即证"构造点在本 crate"：`resolve` 返回了一个真实 `ResolvedTarget`。
#[test]
fn resolve_constructs_a_resolved_target_instance_in_this_crate() {
    let vault = unlocked_two_codes();
    let target = expect_resolved(vault.resolve(&ResourceCode::new("db-main")));
    // Debug 恒 REDACTED（机密类型纪律 §7-1）：构造出的实例 Debug 不泄明文。
    assert_eq!(
        format!("{target:?}"),
        "REDACTED",
        "ResolvedTarget Debug must be the constant REDACTED, never the endpoint plaintext"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  §8 L-11：明文不出边界——错误码不含 code / 真实地址明文；Debug 不泄端点
// ════════════════════════════════════════════════════════════════════════

/// §8 L-11：未知 code 的错误**文案**为常量英文、绝不内插请求的 code 明文。
/// （错误码跨 crate 边界前已脱敏，不回吐原始 code。）
#[test]
fn resolve_error_message_does_not_interpolate_requested_code() {
    let vault = unlocked_two_codes();
    let probe = "super-secret-code-name";
    let err = match vault.resolve(&ResourceCode::new(probe)) {
        Ok(_) => panic!("unknown code must error"),
        Err(e) => e,
    };
    let text = format!("{err}");
    assert!(
        !text.contains(probe),
        "error display must not interpolate the requested code (L-11 plaintext must not leak)"
    );
    assert_eq!(
        text, "no target for requested resource code",
        "UnknownCode display must be the constant English error string"
    );
}

/// §8 L-11：成功路径返回的 `ResolvedTarget` 的 `Debug` 恒为 `REDACTED`——真实地址
/// 明文（如 `10.0.3.17`）绝不经 `Debug` 外泄。
#[test]
fn resolved_target_debug_never_leaks_real_address() {
    let vault = unlocked_two_codes();
    let target = expect_resolved(vault.resolve(&ResourceCode::new("db-main")));
    let dbg = format!("{target:?}");
    assert_eq!(dbg, "REDACTED", "ResolvedTarget Debug must be REDACTED");
    assert!(
        !dbg.contains(HOST_VALUE),
        "ResolvedTarget Debug must never contain the real address plaintext"
    );
}

/// §8 L-11：解析失败侧绝不泄露句柄持有的任何真实地址明文——错误文案与任何已存
/// 代号的地址样本无关（错误是常量码，与 payload 内容无耦合）。
#[test]
fn resolve_failure_does_not_leak_stored_addresses() {
    let vault = unlocked_two_codes();
    let text = match vault.resolve(&ResourceCode::new("unknown")) {
        Ok(_) => panic!("must error"),
        Err(e) => format!("{e}"),
    };
    assert!(
        !text.contains(HOST_VALUE) && !text.contains(INSTANCE_ID_VALUE),
        "a resolve error must not contain any real address held in the vault (L-11)"
    );
}
