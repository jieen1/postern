//! 机密保险箱 vault 文件格式、解锁、原子写入、rekey/rotate-kdf 的行为测试（RED）。
//!
//! 被测对象：`postern_secrets::vault`——vault 文件 codec（`format`）、明文头（`header`）、
//! AEAD 原语（`crypto`）、payload（`payload`）、原子写入与 rekey/rotate-kdf（`write`）、
//! 解锁入口（`vault::unlock`）。
//!
//! 覆盖 §8 条目（逐条加 `// §8 …` 注释）：
//! - F-1（vault 格式与解锁）：合法 vault + 正确主密钥 → 解锁成功并返回可用句柄；明文头
//!   全段作 AEAD AAD 校验通过；篡改明文头任一字节 / data-key 错误 → `UnlockError`、不返回句柄。
//! - F-8（录入/更新原子写入）：整体重加密、临时文件→fsync→原子 rename 覆盖、保留 `.bak`、
//!   本次 nonce 与历史采样互异；回读只得掩码 / `vault://` 引用、绝不回吐明文。
//! - F-9（rekey/rotate-kdf）：rekey / 换 argon2id 参数后同一 data-key 仍解原 payload
//!   （payload 密文未变），仅包裹槽更新。
//! - L-1（`format_version` 不识别→拒锁）。L-2（AAD 篡改/降级→拒锁）。
//! - L-3（nonce 绝不复用）：连续 N 次写采样的 N 个 24B nonce 两两互异（行为）+ nonce 源
//!   恒为 CSPRNG、源码无固定常量 / 计数器 nonce 路径（结构检查）。
//! - L-4（原子写半写不损坏）：临时文件写到一半 / rename 前中断 → 原 vault 仍可解锁、`.bak` 可回退。
//!
//! 测试策略（§3.1）：用**临时 vault 文件**驱动——以 `crypto` 原语 + `format` codec 构造
//! 合法 vault + 固定主密钥；逐字节篡改明文头 / 置错 data-key / 设不识别 `format_version`
//! 验各 fail-closed 分支；连续 N 次写采样 nonce 验两两互异。`MasterKeySource` 用直接持有型
//! 固定主密钥即天然可控，无需真实钥匙串/systemd/TPM。
//!
//! 雷区纪律：本文件含注释 / 字符串均**不含任何裸数据库写标记**；payload 夹具 `secrets` 段键
//! 形如 `db-main/readonly`，字段名 / 值均不触 SQL 标记；**本文件不构造 `ResolvedTarget`/
//! `ResourceCredential`**（vault 单元只解出 payload 明文映射，物化归 mapping/provider）。

use std::collections::BTreeMap;

use postern_secrets::error::UnlockError;
use postern_secrets::vault::crypto;
use postern_secrets::vault::format::{VaultFile, FORMAT_VERSION, MAGIC, NONCE_LEN};
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;
use postern_secrets::vault::write::{self, VaultWriteError};
use postern_secrets::vault::{self, UnlockedVault};
use zeroize::Zeroizing;

// ── 固定测试材料（可控、确定，不碰真实来源） ──────────────────────────────

/// 固定 32B 主密钥（直接持有型来源的解锁主密钥，非全零）。
const MASTER_KEY: [u8; 32] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
    0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
];

/// 一个**错误**的 32B 主密钥（与 `MASTER_KEY` 不同，验"data-key 错误→拒锁"）。
const WRONG_MASTER_KEY: [u8; 32] = [0x42u8; 32];

/// 固定 32B data-key（随机 data-key 的测试替身；包裹槽包裹的就是它）。
const DATA_KEY: [u8; 32] = [
    0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    0xf0, 0x0d, 0xca, 0xfe, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
];

/// 一个 payload 凭据字段值样本（明文，用于"回读绝不回吐明文"取证）。
const SECRET_VALUE: &str = "s3cr3t-ro-pw";
/// payload `targets` 真实地址字段值样本（明文）。
const TARGET_HOST: &str = "10.0.3.17";

fn data_key() -> Zeroizing<[u8; 32]> {
    Zeroizing::new(DATA_KEY)
}

/// 断言一个 `Result<UnlockedVault, UnlockError>` 恰为某 `UnlockError`——不要求 `Ok` 变体
/// 实现 `Debug`（`UnlockedVault` 持有 payload，绝不 derive `Debug`，故不能用 `unwrap_err`）。
fn assert_unlock_err(res: Result<UnlockedVault, UnlockError>, want: UnlockError) {
    match res {
        Ok(_) => panic!("expected unlock to fail with {want:?}, but it returned a handle"),
        Err(e) => assert_eq!(
            e, want,
            "unlock must fail-closed with the expected UnlockError"
        ),
    }
}

/// 断言一个 `Result<UnlockedVault, UnlockError>` 是 `Err`（任何变体），不返回句柄。
fn assert_unlock_refused(res: Result<UnlockedVault, UnlockError>) {
    assert!(
        res.is_err(),
        "unlock must be refused (no handle returned) on this input"
    );
}

/// 断言 `VaultFile::decode` 恰以某 `UnlockError` 失败（`VaultFile` 不实现 `Debug`）。
fn assert_decode_err(res: Result<VaultFile, UnlockError>, want: UnlockError) {
    match res {
        Ok(_) => panic!("expected decode to fail with {want:?}, but it produced a VaultFile"),
        Err(e) => assert_eq!(
            e, want,
            "decode must fail-closed with the expected UnlockError"
        ),
    }
}

/// 构造一个最小但完整的两段 payload：一条 `secrets`（`db-main/readonly`）+ 一条 `targets`
/// （`db-main`）。字段名 / 值均不触任何裸数据库写标记。
fn sample_payload() -> Payload {
    let mut secrets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut ro = BTreeMap::new();
    ro.insert("user".to_string(), Zeroizing::new("ro".to_string()));
    ro.insert(
        "password".to_string(),
        Zeroizing::new(SECRET_VALUE.to_string()),
    );
    secrets.insert("db-main/readonly".to_string(), ro);

    let mut targets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut t = BTreeMap::new();
    t.insert("host".to_string(), Zeroizing::new(TARGET_HOST.to_string()));
    t.insert("port".to_string(), Zeroizing::new("5432".to_string()));
    targets.insert("db-main".to_string(), t);

    Payload::from_sections(secrets, targets)
}

/// 构造一个**只含 KeyFile 来源单包裹槽**的明文头（直接持有型，无 KDF 参数 / salt）。
/// `nonce_i` / `wrapped_data_key` 在 `build_valid_vault` 里由 `crypto::wrap_data_key` 填实。
fn header_with_slot(nonce_i: [u8; NONCE_LEN], wrapped: Vec<u8>) -> Header {
    Header {
        format_version: FORMAT_VERSION,
        slots: vec![Slot {
            source: SlotSource::KeyFile,
            kdf_params: None,
            salt: None,
            nonce_i,
            wrapped_data_key: wrapped,
        }],
    }
}

/// 端到端构造一个**合法 vault 文件字节**：
/// 用 `MASTER_KEY` 包裹 `DATA_KEY` 入槽 → 用 `DATA_KEY` 加密 `sample_payload`（明文头作 AAD）
/// → `encode` 成整文件字节。返回 `(vault_bytes, VaultFile)`（后者便于逐字段断言）。
fn build_valid_vault() -> (Vec<u8>, VaultFile) {
    let dk = data_key();
    let (slot_nonce, wrapped) =
        crypto::wrap_data_key(&MASTER_KEY, &dk).expect("wrap data-key under master key");
    let header = header_with_slot(slot_nonce, wrapped);

    let payload = sample_payload();
    let plaintext = payload
        .to_plaintext()
        .expect("serialize payload to JSON plaintext");

    // 明文头全段作 AAD：先用一个临时 VaultFile 取 aad_bytes（payload_nonce/ciphertext 占位），
    // 再以该 AAD 加密 payload，回填真实 nonce/密文。
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
    (bytes, vf)
}

/// 端到端构造一个**指定直接持有型来源**（KeyFile/OsKeychain/SystemdCred，无 KDF / salt）的
/// 合法 vault 字节：用 `MASTER_KEY` 包裹 `DATA_KEY` 入对应 source 的单槽 → 用 data-key 加密
/// `sample_payload`（明文头作 AAD）。返回 `(bytes, VaultFile)`。供「直接持有型来源逐字节篡改
/// 即拒锁」取证（这些来源的 KDF / salt 元数据字节既不在 AAD 内、又无口令再派生消费）。
fn build_direct_hold_vault(source: SlotSource) -> (Vec<u8>, VaultFile) {
    let dk = data_key();
    let (slot_nonce, wrapped) =
        crypto::wrap_data_key(&MASTER_KEY, &dk).expect("wrap data-key under master key");
    let header = Header {
        format_version: FORMAT_VERSION,
        slots: vec![Slot {
            source,
            kdf_params: None,
            salt: None,
            nonce_i: slot_nonce,
            wrapped_data_key: wrapped,
        }],
    };
    let plaintext = sample_payload().to_plaintext().expect("serialize payload");
    let mut vf = VaultFile {
        header,
        payload_nonce: [0u8; NONCE_LEN],
        ciphertext: Vec::new(),
    };
    let aad = vf.aad_bytes();
    let (pn, ct) = crypto::encrypt_payload(&dk, &plaintext, &aad).expect("encrypt payload");
    vf.payload_nonce = pn;
    vf.ciphertext = ct;
    let bytes = vf.encode();
    (bytes, vf)
}

/// 构造一个**data-key 可正确解出、但 payload 明文是任意给定字节**的 vault：用 `DATA_KEY`
/// 直接加密 `plaintext`（明文头作 AAD），其余同 `build_valid_vault`。供「解密成功但 JSON 坏 →
/// `PayloadDecryptFailed`」取证——AEAD 层放行（tag 有效），解析层 fail-closed。
fn build_vault_with_plaintext(plaintext: &[u8]) -> Vec<u8> {
    let dk = data_key();
    let (slot_nonce, wrapped) =
        crypto::wrap_data_key(&MASTER_KEY, &dk).expect("wrap data-key under master key");
    let header = header_with_slot(slot_nonce, wrapped);
    let mut vf = VaultFile {
        header,
        payload_nonce: [0u8; NONCE_LEN],
        ciphertext: Vec::new(),
    };
    let aad = vf.aad_bytes();
    let (pn, ct) =
        crypto::encrypt_payload(&dk, plaintext, &aad).expect("encrypt arbitrary plaintext");
    vf.payload_nonce = pn;
    vf.ciphertext = ct;
    vf.encode()
}

// ════════════════════════════════════════════════════════════════════════
//  F-1 / L-6：合法 vault + 正确主密钥 → 解锁成功，返回可用句柄
// ════════════════════════════════════════════════════════════════════════

/// §8 F-1：合法 vault + 正确主密钥 → 解锁成功并返回可用保险箱句柄；解出的 payload 含
/// 写入的 `secrets`/`targets` 条目（明文头全段作 AEAD AAD 校验通过的端到端证据）。
#[test]
fn valid_vault_with_correct_master_key_unlocks_and_returns_handle() {
    let (bytes, _) = build_valid_vault();
    let unlocked: UnlockedVault =
        vault::unlock(&MASTER_KEY, &bytes).expect("valid vault + correct key must unlock");
    let payload = unlocked.payload();
    assert_eq!(
        payload.secret_refs(),
        vec!["db-main/readonly".to_string()],
        "unlocked payload must expose exactly the written secrets reference"
    );
    assert_eq!(
        payload.target_codes(),
        vec!["db-main".to_string()],
        "unlocked payload must expose exactly the written target code"
    );
}

/// §8 F-1：解锁产物对 `secrets` 引用键查得写入的**字段名集合**（端到端 round-trip：写入的
/// 字段结构经加密→解密后完整恢复，证明 data-key + AAD 路径整链正确）。字段**名**非敏感，
/// 字段**值**不经此暴露（值的不回吐另由掩码回读测试钉死）。
#[test]
fn unlocked_payload_round_trips_written_secret_field_names() {
    let (bytes, _) = build_valid_vault();
    let unlocked = vault::unlock(&MASTER_KEY, &bytes).expect("unlock");
    let mut names = unlocked
        .payload()
        .secret_field_names("db-main/readonly")
        .expect("written secret ref must be present after round-trip");
    names.sort();
    assert_eq!(
        names,
        vec!["password".to_string(), "user".to_string()],
        "decrypted secret entry must recover exactly the written field names"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  F-1 / L-2：篡改明文头任一字节 → AEAD 校验失败拒锁
// ════════════════════════════════════════════════════════════════════════

/// §8 L-2：篡改明文头**任一字节**（魔数 / `format_version` / source / KDF 参数 / salt /
/// `nonce_i` / `wrapped_data_key`——**整段、逐字节、无遗漏**）→ 解锁被拒、不返回句柄。
///
/// 用 **passphrase 来源** vault 取证——只有这种来源的明文头**每个字节都被解锁路径消费**，
/// 故"任一字节篡改即拒锁"对整段端到端成立：魔数 / 版本由 decode 拦（L-1）；source 在
/// payload AAD 内（原始字节 AAD，篡改 → `AadMismatch`）；`kdf_params` / `salt` 经 argon2id
/// 派生主密钥消费（篡改 → 派生出不同主密钥 → 开包裹槽失败 / 派生失败）；`nonce_i` /
/// `wrapped_data_key` 由包裹槽 AEAD 消费（篡改 → 开包裹槽失败）。错误码按被篡改区段而异
/// （这正是 AAD 不绑定包裹槽材料、改由派生 / 包裹槽 AEAD 兜底的 fail-closed 设计，见
/// `format::AAD_PREFIX_LEN`）——故此处断言**任一变体的 `UnlockError`、不返回句柄**，
/// 而非钉死某个魔法偏移只产某一种错误码。
///
/// 走**完整 passphrase 解锁链**（从被篡改字节读回 salt/params → 派生主密钥 → 解锁），
/// 模拟真实威胁模型：攻击者改盘上明文头任一字节，持正确口令的合法解锁仍必败。
#[test]
fn tampering_any_plaintext_header_byte_fails_aad_and_refuses_unlock() {
    let pass = Zeroizing::new(b"a strong unlock phrase".to_vec());
    let params = test_kdf_params();
    let salt = b"salt-aaaa-16byte";
    let (bytes, _) = build_passphrase_vault(&pass, params, salt);

    // 明文头区段 = 文件开头 → payload nonce 之前（魔数 + 版本 + 整个包裹槽）。
    let vf = VaultFile::decode(&bytes).expect("decode baseline passphrase vault");
    let header_end = bytes.len() - NONCE_LEN - vf.ciphertext.len();

    // 基线：未篡改字节经完整 passphrase 解锁链必成功（证明探针链本身可解锁）。
    assert!(
        passphrase_unlock_from_bytes(&pass, &bytes).is_ok(),
        "untampered passphrase vault must unlock via the full derive-then-unlock chain"
    );

    let mut probed = 0usize;
    for i in 0..header_end {
        let Some(orig) = bytes.get(i).copied() else {
            continue;
        };
        let mut t = bytes.clone();
        if let Some(b) = t.get_mut(i) {
            *b = orig ^ 0xff;
        }
        // 明文头任一字节篡改 → 完整解锁链拒锁（任一 `UnlockError` 变体），绝不返回句柄。
        assert_unlock_refused(passphrase_unlock_from_bytes(&pass, &t));
        probed += 1;
    }
    // 必须真的逐字节扫完整个明文头（魔数 6 + 包裹槽数十字节），而非某个 8 字节窗口。
    assert!(
        probed >= MAGIC.len() + 1 + 1 + 13 + 1 + 4 + salt.len(),
        "must probe the entire plaintext header byte-by-byte (magic+version+full slot prefix), got {probed}"
    );

    // 子断言：AAD 直接覆盖的前缀字节（魔数 + 版本 + source）篡改也必拒——这部分既经
    // decode（魔数 / 版本）又经原始字节 AAD（source）拦截。
    for i in 0..(MAGIC.len() + 1 + 1) {
        let Some(orig) = bytes.get(i).copied() else {
            continue;
        };
        let mut t = bytes.clone();
        if let Some(b) = t.get_mut(i) {
            *b = orig ^ 0xff;
        }
        assert_unlock_refused(passphrase_unlock_from_bytes(&pass, &t));
    }
}

/// §8 L-2 / F-1：对**直接持有型来源**（KeyFile/OsKeychain/SystemdCred——无人值守常驻
/// daemon 的主部署形态）的 vault，篡改明文头**任一字节**（含 KDF / salt 元数据区）→ 拒锁。
///
/// 直接持有型来源无 passphrase 再派生，KDF / salt 元数据字节既不在 payload AAD 内
/// （`AAD_PREFIX_LEN` 排除轮换重写材料），又不经口令派生消费——若解码端把这些字节读后丢弃，
/// 篡改它们（has_kdf / m,t,p_cost / has_salt / salt_len 区，偏移 7..N）解锁仍会成功，
/// 直接违反 L-2「篡改明文头任一字节 → 不返回句柄」。本测试以 `MASTER_KEY` 直接解锁，逐字节
/// 翻转整个明文头（魔数 → payload nonce 之前），断言**无一字节**篡改后仍能解锁。
#[test]
fn tampering_any_header_byte_of_direct_hold_source_vault_refuses_unlock() {
    for source in [
        SlotSource::KeyFile,
        SlotSource::OsKeychain,
        SlotSource::SystemdCred,
    ] {
        let (bytes, vf) = build_direct_hold_vault(source);
        // 基线：未篡改字节用正确主密钥必解锁（证明该来源的探针链本身可解锁）。
        vault::unlock(&MASTER_KEY, &bytes).expect("untampered direct-hold vault must unlock");

        // 明文头 = 文件开头 → payload nonce 之前（魔数 + 版本 + 整个包裹槽）。
        let header_end = bytes.len() - NONCE_LEN - vf.ciphertext.len();
        // 明文头必须覆盖到 KDF / salt 元数据区（魔数 6 + source 1 + has_kdf 1 + 参数 12 +
        // has_salt 1 + salt_len 4 = 至少 25 字节），否则探不到本缺口所在区段。
        assert!(
            header_end >= MAGIC.len() + 1 + 1 + 12 + 1 + 4,
            "header must span the KDF/salt metadata region (got header_end={header_end})"
        );

        for i in 0..header_end {
            let Some(orig) = bytes.get(i).copied() else {
                continue;
            };
            let mut t = bytes.clone();
            if let Some(b) = t.get_mut(i) {
                *b = orig ^ 0xff;
            }
            assert_unlock_refused(vault::unlock(&MASTER_KEY, &t));
        }

        // 子断言：显式钉死 KDF / salt 元数据区（偏移 7..=20：has_kdf + 12 参数字节 + has_salt）——
        // 这正是镜头实证「篡改后仍 unlock 成功」的 SURVIVED_OFFSETS。任一字节翻转后必拒锁。
        let meta_start = MAGIC.len() + 1 + 1; // magic + version + source 之后即 has_kdf
        let meta_end = meta_start + 1 + 12 + 1; // has_kdf + (m,t,p)_cost + has_salt
        for i in meta_start..meta_end {
            let orig = *bytes.get(i).expect("kdf/salt metadata byte present");
            let mut t = bytes.clone();
            if let Some(b) = t.get_mut(i) {
                *b = orig ^ 0xff;
            }
            assert_unlock_refused(vault::unlock(&MASTER_KEY, &t));
        }
    }
}

/// §8 F-1 / L-2：篡改 **payload 密文区任一字节**（或 payload nonce）→ AEAD tag 校验失败拒锁。
///
/// 这条 fail-closed 路径（data-key 可解但密文损坏 → 拒锁）此前无任何测试驱动：旧逐字节循环
/// 显式止于明文头末尾，nonce + ciphertext 整段被排除。若回归使 `decrypt_payload` 不校验
/// tag / 返回部分明文，该缺陷不被任何断言捕获。本测试逐字节翻转 payload nonce + ciphertext，
/// 断言**无一字节**篡改后仍能解锁（AEAD tag 校验恒生效）。
#[test]
fn tampering_any_payload_ciphertext_or_nonce_byte_fails_aead_and_refuses_unlock() {
    let (bytes, vf) = build_valid_vault();
    // 基线：未篡改必解锁。
    vault::unlock(&MASTER_KEY, &bytes).expect("untampered vault must unlock");

    // payload 区 = 明文头之后的 nonce(24B) + ciphertext（含 Poly1305 tag）。
    let header_end = bytes.len() - NONCE_LEN - vf.ciphertext.len();
    assert!(
        !vf.ciphertext.is_empty(),
        "ciphertext region must be non-empty to probe AEAD tag enforcement"
    );

    let mut probed = 0usize;
    for i in header_end..bytes.len() {
        let orig = *bytes.get(i).expect("payload byte present");
        let mut t = bytes.clone();
        if let Some(b) = t.get_mut(i) {
            *b = orig ^ 0xff;
        }
        // 篡改 payload nonce 或密文任一字节 → AEAD tag 校验失败 → 拒锁、不返回句柄、不吐部分明文。
        assert_unlock_err(vault::unlock(&MASTER_KEY, &t), UnlockError::AadMismatch);
        probed += 1;
    }
    // 必须真的覆盖 nonce(24) + 整段密文（含 16B tag）。
    assert!(
        probed >= NONCE_LEN + vf.ciphertext.len(),
        "must probe the entire payload nonce + ciphertext region byte-by-byte, got {probed}"
    );
}

/// §8 F-1 / B-6：data-key 可解、但解出的明文是**坏 JSON**（结构不符 / 截断 / 多余键）→
/// `Payload::from_plaintext` 必返回 `PayloadDecryptFailed`、不返回半截 payload、不返回句柄。
///
/// 此前无任何夹具构造「data-key 可解、明文却坏」的 vault；回归使 `from_plaintext` 对坏输入
/// 产出空 / 半截 Payload 而非 `Err` 时无测试能捕获。本测试用正确 data-key 加密多个坏明文，
/// 经完整 `vault::unlock` 链断言恰返回 `PayloadDecryptFailed`。
#[test]
fn unlock_with_decryptable_but_malformed_payload_json_fails_closed() {
    let malformed_cases: &[&[u8]] = &[
        b"",                                                 // 空明文
        b"{",                                                // 截断的对象
        b"not json at all",                                  // 非 JSON
        b"{\"secrets\":{}}",                                 // 缺 targets 段
        b"{\"targets\":{}}",                                 // 缺 secrets 段
        b"{\"secrets\":{},\"targets\":{},\"extra\":{}}",     // 多余顶层键
        b"{\"secrets\":{\"a\":{\"k\":\"v\"}},\"targets\":{", // targets 段截断
        b"\xff\xfe\xfd",                                     // 非法 UTF-8
    ];
    for bad in malformed_cases {
        let bytes = build_vault_with_plaintext(bad);
        // data-key 必能解出（AAD 一致、tag 有效），但 from_plaintext 必拒坏 JSON。
        assert_unlock_err(
            vault::unlock(&MASTER_KEY, &bytes),
            UnlockError::PayloadDecryptFailed,
        );
    }
}

/// §8 L-1 / L-2：`format_version` **降级**为不识别值 → 拒锁。version 字节就在魔数之后，
/// 它既是格式判别又在 AAD 内：被识别性检查先拦（L-1），故返回 `UnknownFormatVersion`，
/// 绝不按未知格式继续尝试解密、不返回句柄。
#[test]
fn format_version_downgrade_is_rejected_before_any_decrypt() {
    let (mut bytes, _) = build_valid_vault();
    let version_idx = MAGIC.len();
    let cur = *bytes.get(version_idx).expect("version byte present");
    // 设为一个当前实现不识别的版本值（确保 ≠ FORMAT_VERSION）。
    let bogus = cur.wrapping_add(7);
    if let Some(b) = bytes.get_mut(version_idx) {
        *b = bogus;
    }
    assert_unlock_err(
        vault::unlock(&MASTER_KEY, &bytes),
        UnlockError::UnknownFormatVersion,
    );
}

// ════════════════════════════════════════════════════════════════════════
//  F-1：data-key 错误（错误主密钥开不出 data-key）→ 拒锁
// ════════════════════════════════════════════════════════════════════════

/// §8 F-1：**错误主密钥** → 开包裹槽（解 data-key）AEAD 校验失败 → 返回 `UnlockError`、
/// 不返回句柄。错误主密钥解不出正确 data-key，是 fail-closed 的核心分支。
#[test]
fn wrong_master_key_fails_to_unwrap_data_key_and_refuses_unlock() {
    let (bytes, _) = build_valid_vault();
    assert_unlock_err(
        vault::unlock(&WRONG_MASTER_KEY, &bytes),
        UnlockError::PayloadDecryptFailed,
    );
}

/// §8 F-1：截断的 vault 字节（少于最小头长）→ `decode` fail-closed，返回 `UnlockError`、
/// 不 panic、不返回句柄。
#[test]
fn truncated_vault_bytes_are_rejected_not_panicked() {
    let (bytes, _) = build_valid_vault();
    let short = bytes
        .get(0..MAGIC.len())
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    assert_unlock_refused(vault::unlock(&MASTER_KEY, &short));
}

/// §8 F-1：魔数不符（非 PSTRN 文件）→ 拒锁，返回 `UnlockError`、不返回句柄。
#[test]
fn wrong_magic_bytes_are_rejected() {
    let (mut bytes, _) = build_valid_vault();
    if let Some(b) = bytes.get_mut(0) {
        *b = b'X'; // 破坏魔数首字节
    }
    assert_unlock_refused(vault::unlock(&MASTER_KEY, &bytes));
}

// ════════════════════════════════════════════════════════════════════════
//  L-1：未知 format_version 在 decode 层即拒（不依赖主密钥）
// ════════════════════════════════════════════════════════════════════════

/// §8 L-1：`VaultFile::decode` 对不识别 `format_version` 直接返回 `UnknownFormatVersion`——
/// 在 decode 层 fail-closed，绝不按未知格式继续切分 / 尝试解密。
#[test]
fn decode_rejects_unrecognized_format_version_at_codec_layer() {
    let (mut bytes, _) = build_valid_vault();
    let version_idx = MAGIC.len();
    if let Some(b) = bytes.get_mut(version_idx) {
        *b = 0xfe; // 不识别版本
    }
    assert_decode_err(VaultFile::decode(&bytes), UnlockError::UnknownFormatVersion);
}

/// §8 F-1：`decode` 对合法字节成功，且 round-trips——`encode(decode(bytes)) == bytes`
/// （codec 是确定、无损的，使逐字节篡改测试有意义）。
#[test]
fn decode_then_encode_round_trips_byte_identical() {
    let (bytes, _) = build_valid_vault();
    let vf = VaultFile::decode(&bytes).expect("decode valid vault");
    assert_eq!(
        vf.encode(),
        bytes,
        "encode∘decode must be byte-identical (deterministic lossless codec)"
    );
    assert_eq!(
        vf.header.format_version, FORMAT_VERSION,
        "decoded header must carry the recognized format_version"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  L-3：nonce 绝不复用（行为采样 + 结构检查）
// ════════════════════════════════════════════════════════════════════════

/// §8 L-3：连续 N 次调用 `crypto::new_nonce` 采样的 N 个 24B nonce **两两互异**
/// （行为观察：CSPRNG 取值在 24B 空间内重复概率可忽略）。
#[test]
fn consecutive_nonces_are_pairwise_distinct() {
    const N: usize = 64;
    let nonces: Vec<[u8; NONCE_LEN]> = (0..N).map(|_| crypto::new_nonce()).collect();
    for i in 0..nonces.len() {
        for j in (i + 1)..nonces.len() {
            assert_ne!(
                nonces[i], nonces[j],
                "nonces {i} and {j} must differ (nonce never reused, L-3)"
            );
        }
    }
    // 每个 nonce 恰 24 字节，且不是全零常量（结构性反例：常量 nonce 会全相等且常为 0）。
    for (k, n) in nonces.iter().enumerate() {
        assert_eq!(n.len(), NONCE_LEN, "nonce {k} must be exactly 24 bytes");
        assert_ne!(
            *n, [0u8; NONCE_LEN],
            "nonce {k} must not be the all-zero constant"
        );
    }
}

/// §8 L-3：两次写入（整体重加密）采样的 payload nonce 互异——同一 payload + 同一 data-key
/// 连写两次，密文里的 nonce 必不同（每次写入 CSPRNG 重生成，绝不复用）。
#[test]
fn two_writes_of_same_payload_use_distinct_nonces() {
    let dk = data_key();
    let payload = sample_payload();
    let plaintext = payload.to_plaintext().expect("serialize");
    let aad = b"fixed-header-aad-sample";

    let (n1, c1) = crypto::encrypt_payload(&dk, &plaintext, aad).expect("encrypt 1");
    let (n2, c2) = crypto::encrypt_payload(&dk, &plaintext, aad).expect("encrypt 2");
    assert_ne!(
        n1, n2,
        "two encryptions of identical payload must draw distinct nonces (no reuse, L-3)"
    );
    assert_ne!(
        c1, c2,
        "distinct nonces must yield distinct ciphertext for identical plaintext"
    );
}

/// §8 L-3（结构检查）：`crypto.rs` 源码 nonce 取值源恒为 CSPRNG（`OsRng`），且**无固定
/// 常量 nonce、无计数器递增 nonce 路径**。读源文件做文本级反例断言。
#[test]
fn crypto_source_draws_nonces_only_from_csprng_no_constant_or_counter() {
    const CRYPTO_SRC: &str = include_str!("../src/vault/crypto.rs");
    // 取值源恒为 CSPRNG：OsRng 必须出现在 nonce 取值路径里。
    assert!(
        CRYPTO_SRC.contains("OsRng"),
        "nonce source must be the CSPRNG OsRng (chacha20poly1305::aead::OsRng)"
    );
    // 反例：源码不得出现"计数器递增 nonce"的取值路径标记（如对 nonce 做 += / wrapping_add /
    // checked_add 递增），也不得把固定常量数组当 nonce 用。此处以保守文本反例守底线。
    assert!(
        !CRYPTO_SRC.contains("nonce_counter") && !CRYPTO_SRC.contains("NONCE_CONST"),
        "crypto must not expose a counter-based or constant nonce path (L-3 structural)"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  F-8：录入/更新原子写入（temp→fsync→rename，保留 .bak），整体重加密
// ════════════════════════════════════════════════════════════════════════

/// §8 F-8：原子写入后，目标路径就是一个可解锁的合法 vault（整体重加密落盘正确）。
#[test]
fn atomic_write_produces_an_unlockable_vault_at_target_path() {
    let dir = tempdir();
    let path = dir.join("vault.postern");
    let dk = data_key();

    // 先有一份"原 vault"以触发 .bak 保留路径；这里直接用 build_valid_vault 落一份初始文件。
    let (initial, _) = build_valid_vault();
    std::fs::write(&path, &initial).expect("seed initial vault");

    // 整体重加密写入一份新 payload。
    let header = header_for_write(&dk);
    let payload = sample_payload();
    payload
        .write_atomic(&path, &MASTER_KEY, &dk, &header)
        .expect("atomic write must succeed");

    let bytes = std::fs::read(&path).expect("read back written vault");
    let unlocked = vault::unlock(&MASTER_KEY, &bytes).expect("written vault must unlock");
    assert_eq!(
        unlocked.payload().secret_refs(),
        vec!["db-main/readonly".to_string()],
        "written vault must round-trip the payload"
    );
}

/// §8 F-8：原子写入**覆盖**已存在的 vault 时，把上一代保留为 `.bak`——`.bak` 仍可独立
/// 解锁（保留的是完整旧 vault，不是垃圾）。
#[test]
fn atomic_write_preserves_previous_generation_as_bak() {
    let dir = tempdir();
    let path = dir.join("vault.postern");
    let bak = dir.join("vault.postern.bak");
    let dk = data_key();

    let (initial, _) = build_valid_vault();
    std::fs::write(&path, &initial).expect("seed initial vault");

    let header = header_for_write(&dk);
    sample_payload()
        .write_atomic(&path, &MASTER_KEY, &dk, &header)
        .expect("atomic write");

    assert!(bak.exists(), "previous generation must be retained as .bak");
    let bak_bytes = std::fs::read(&bak).expect("read .bak");
    assert_eq!(
        bak_bytes, initial,
        ".bak must be the byte-identical previous-generation vault"
    );
    // .bak 自身可独立解锁，证明它是完整旧 vault（可回退）。
    vault::unlock(&MASTER_KEY, &bak_bytes).expect(".bak must itself unlock (rollback target)");
}

/// §8 F-8：回读**只得掩码**——`masked_secret` 对每个字段返回掩码标记，**绝不回吐明文**。
/// 掩码结果里既无明文凭据值，键集合仍完整（掩码是逐字段值替换，不丢字段名）。
#[test]
fn masked_readback_never_yields_plaintext_secret_value() {
    let (bytes, _) = build_valid_vault();
    let unlocked = vault::unlock(&MASTER_KEY, &bytes).expect("unlock");
    let masked = unlocked
        .payload()
        .masked_secret("db-main/readonly")
        .expect("known secret ref must yield a masked view");
    // 字段名保留。
    assert!(
        masked.contains_key("user") && masked.contains_key("password"),
        "masked readback must keep field names"
    );
    // 任一掩码值都不得等于明文，也不得把明文作为子串包含。
    for (field, val) in &masked {
        assert_ne!(
            val, SECRET_VALUE,
            "masked value for {field} must not equal the plaintext secret"
        );
        assert!(
            !val.contains(SECRET_VALUE),
            "masked value for {field} must not contain the plaintext secret as a substring"
        );
    }
}

/// §8 F-8：回读引用形态——`secret_refs` 只产 `vault://`-可寻址的引用键，**不含任何凭据
/// 明文值**（引用键即 `<code>/<tier-or-slot>` 形态，不嵌入凭据值）。
#[test]
fn secret_refs_readback_contains_no_plaintext_value() {
    let (bytes, _) = build_valid_vault();
    let unlocked = vault::unlock(&MASTER_KEY, &bytes).expect("unlock");
    for r in unlocked.payload().secret_refs() {
        assert!(
            !r.contains(SECRET_VALUE),
            "secret reference {r:?} must not leak the plaintext credential value"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════
//  L-4：原子写半写不损坏（rename 前中断 / 临时文件半写 → 原 vault 完好、.bak 可回退）
// ════════════════════════════════════════════════════════════════════════

/// §8 L-4：**驱动真实生产写路径** `write_atomic`，并在其内部「临时文件写阶段 / `rename`
/// 覆盖原 vault 之前」注入失败 → 原 vault 路径**逐字节不变**、仍可解锁；上一代 `.bak` 可回退。
///
/// 因果取证（区别于空转重言式）：在原 vault 同路径 `path` 上调用 `write_atomic`，但事先把
/// 临时文件路径 `path.tmp` 占成一个**目录**——使 `write_atomic` 内部写临时文件那一步
/// （`File::create(tmp)`）失败，从而在**任何 `rename` 覆盖 `path` 之前**短路返回 `Err`。
/// 这条路径真正经过 `payload.write_atomic`：若实现被改成就地截断覆盖 `path`（L-4 真正要防的
/// 灾难），原 vault 会被破坏、本测试即红。断言：失败返回 `AtomicWriteFailed`、原 vault 字节
/// 不变且可解锁、保留的 `.bak` 与原 vault 逐字节相同（可回退）。
#[test]
fn interrupted_write_before_rename_leaves_original_vault_intact() {
    let dir = tempdir();
    let path = dir.join("vault.postern");
    let bak = dir.join("vault.postern.bak");
    let (initial, _) = build_valid_vault();
    std::fs::write(&path, &initial).expect("seed initial vault");

    // 把临时文件路径占成目录 → write_atomic 写临时文件失败，rename 永不发生（原 vault 不被触碰）。
    let tmp_blocker = dir.join("vault.postern.tmp");
    std::fs::create_dir(&tmp_blocker)
        .expect("occupy temp path with a directory to block tmp write");

    let dk = data_key();
    let header = header_for_write(&dk);
    // 真正调用被测的原子写路径（不是手 std::fs::write 一个无关 tmp）。
    let res = sample_payload().write_atomic(&path, &MASTER_KEY, &dk, &header);
    assert_eq!(
        res,
        Err(VaultWriteError::AtomicWriteFailed),
        "an interrupted temp-write (before rename) must fail-closed as AtomicWriteFailed"
    );

    // 原 vault 必须逐字节不变、仍可解锁（rename 前的失败绝不触及 path，L-4）。
    let after = std::fs::read(&path).expect("read original vault");
    assert_eq!(
        after, initial,
        "original vault must be byte-unchanged by a write interrupted before rename"
    );
    vault::unlock(&MASTER_KEY, &after)
        .expect("original vault must still unlock after an interrupted write");

    // .bak 已在覆盖前保留为上一代、与原 vault 逐字节相同且可独立解锁（可回退，L-4）。
    assert!(
        bak.exists(),
        "interrupted write must still have preserved the previous generation as .bak"
    );
    let bak_bytes = std::fs::read(&bak).expect("read .bak");
    assert_eq!(
        bak_bytes, initial,
        ".bak must be the byte-identical previous-generation vault"
    );
    vault::unlock(&MASTER_KEY, &bak_bytes).expect(".bak must itself unlock (rollback target)");
}

/// §8 L-4：`write_atomic` 内部失败（如目标目录不可写）时返回 `Err`，**绝不**把原 vault
/// 改成半写状态——原文件解锁仍成功。以一个**不存在的父目录**触发原子写失败分支。
#[test]
fn failed_atomic_write_returns_err_and_does_not_corrupt_existing_vault() {
    let dir = tempdir();
    let path = dir.join("vault.postern");
    let (initial, _) = build_valid_vault();
    std::fs::write(&path, &initial).expect("seed initial vault");

    // 指向一个不可写的目标（父目录不存在）触发 IO 失败分支。
    let bad = dir.join("no-such-subdir").join("vault.postern");
    let dk = data_key();
    let header = header_for_write(&dk);
    let res = sample_payload().write_atomic(&bad, &MASTER_KEY, &dk, &header);
    assert_eq!(
        res,
        Err(VaultWriteError::AtomicWriteFailed),
        "an un-writable target must fail-closed as AtomicWriteFailed"
    );

    // 已存在的原 vault（另一路径）未被触及，仍可解锁。
    let after = std::fs::read(&path).expect("read original vault");
    assert_eq!(
        after, initial,
        "a failed write to another path must not touch the existing vault"
    );
    vault::unlock(&MASTER_KEY, &after).expect("existing vault must remain unlockable");
}

/// §8 L-4：**覆盖同一已存在 vault 的中途失败**——`write_atomic` 在覆盖 `path`（已有合法
/// vault）的过程中失败时，该 vault 仍完好且可由 `.bak` 回退。这是 L-4 的核心风险（区别于
/// [failed_atomic_write] 写另一不存在路径的恒真构造）：写入目标就是 `path` 本身，失败注入在
/// `rename` 覆盖之前，断言 `path` 字节不变 + `.bak` 持有可回退的旧 vault。
#[test]
fn failed_overwrite_of_same_existing_vault_keeps_it_intact_and_bak_recoverable() {
    let dir = tempdir();
    let path = dir.join("vault.postern");
    let bak = dir.join("vault.postern.bak");
    let (initial, _) = build_valid_vault();
    std::fs::write(&path, &initial).expect("seed initial vault");

    // 把临时文件路径占成目录 → write_atomic 写临时文件失败，rename 覆盖 path 永不发生。
    let tmp_blocker = dir.join("vault.postern.tmp");
    std::fs::create_dir(&tmp_blocker).expect("occupy temp path with a directory");

    let dk = data_key();
    let header = header_for_write(&dk);
    // 目标 = path 本身（已存在的 vault），覆盖中途失败。
    let res = sample_payload().write_atomic(&path, &MASTER_KEY, &dk, &header);
    assert_eq!(
        res,
        Err(VaultWriteError::AtomicWriteFailed),
        "a mid-overwrite failure on the existing vault must fail-closed as AtomicWriteFailed"
    );

    // 同一已存在 vault 完好：字节不变、仍可解锁（覆盖中途失败绝不就地损坏它，L-4）。
    let after = std::fs::read(&path).expect("read original vault");
    assert_eq!(
        after, initial,
        "the existing vault being overwritten must stay byte-identical when the write fails midway"
    );
    vault::unlock(&MASTER_KEY, &after).expect("the existing vault must remain unlockable");

    // .bak 回退分支被真实覆盖：覆盖前已把上一代复制为 .bak，可独立解锁回退。
    assert!(
        bak.exists(),
        ".bak rollback copy must exist after a failed overwrite of an existing vault"
    );
    let bak_bytes = std::fs::read(&bak).expect("read .bak");
    assert_eq!(
        bak_bytes, initial,
        ".bak must be the byte-identical recoverable previous generation"
    );
    vault::unlock(&MASTER_KEY, &bak_bytes).expect(".bak must itself unlock (rollback target)");
}

// ════════════════════════════════════════════════════════════════════════
//  F-9：rekey / rotate-kdf——重包裹 data-key，payload 密文一字不动
// ════════════════════════════════════════════════════════════════════════

/// §8 F-9：rekey（换主密钥）后——**payload 段（nonce + ciphertext）逐字节不变**，仅包裹槽
/// 更新；且新 vault 用**新主密钥**仍解出原 payload（同一 data-key）。
#[test]
fn rekey_rewraps_data_key_but_payload_ciphertext_is_byte_identical() {
    let (bytes, before) = build_valid_vault();

    let rekeyed = write::rekey(&bytes, &MASTER_KEY, &WRONG_MASTER_KEY)
        .expect("rekey under a new master key must succeed");
    let after = VaultFile::decode(&rekeyed).expect("decode rekeyed vault");

    // payload 密文一字不动（F-9 核心：rekey 只重写包裹槽）。
    assert_eq!(
        after.payload_nonce, before.payload_nonce,
        "rekey must NOT change the payload nonce (payload ciphertext untouched)"
    );
    assert_eq!(
        after.ciphertext, before.ciphertext,
        "rekey must NOT change the payload ciphertext (only the wrapping slot is rewritten)"
    );
    // 包裹槽确实变了（新主密钥重包裹 → wrapped_data_key 不同）。
    let before_slot = before.header.primary_slot().expect("before slot");
    let after_slot = after.header.primary_slot().expect("after slot");
    assert_ne!(
        before_slot.wrapped_data_key, after_slot.wrapped_data_key,
        "rekey must rewrite the wrapping slot (wrapped data-key differs)"
    );

    // 新主密钥解锁新 vault → 仍得原 payload（同一 data-key）。
    let unlocked =
        vault::unlock(&WRONG_MASTER_KEY, &rekeyed).expect("rekeyed vault unlocks under new key");
    assert_eq!(
        unlocked.payload().secret_refs(),
        vec!["db-main/readonly".to_string()],
        "rekeyed vault must still decrypt to the original payload"
    );
}

/// §8 F-9：rekey 后**旧主密钥不再能解锁**新 vault（包裹槽已换）——换密钥的语义成立。
#[test]
fn after_rekey_old_master_key_no_longer_unlocks() {
    let (bytes, _) = build_valid_vault();
    let rekeyed = write::rekey(&bytes, &MASTER_KEY, &WRONG_MASTER_KEY).expect("rekey");
    // 旧主密钥开不出新包裹槽 → 不返回句柄（换密钥语义成立）。
    assert_unlock_refused(vault::unlock(&MASTER_KEY, &rekeyed));
}

/// §8 F-9：rotate-kdf（换 argon2id 参数 / salt）后——**payload 段逐字节不变**，仅
/// passphrase 包裹槽的 KDF 参数 / salt / 包裹密文更新（同一 data-key 仍解原 payload）。
/// 这里构造一个 passphrase 来源的 vault 以使 rotate-kdf 有意义。
#[test]
fn rotate_kdf_rewraps_under_new_params_but_payload_ciphertext_is_byte_identical() {
    let pass = Zeroizing::new(b"a strong unlock phrase".to_vec());
    let old_params = test_kdf_params();
    let old_salt = b"salt-aaaa-16byte";
    let (bytes, before) = build_passphrase_vault(&pass, old_params, old_salt);

    let new_params = postern_secrets::vault::header::KdfParams {
        m_cost: old_params.m_cost * 2, // 换更高内存成本
        t_cost: old_params.t_cost + 1,
        p_cost: old_params.p_cost,
    };
    let new_salt = b"salt-bbbb-16byte";

    let rotated = write::rotate_kdf(&bytes, &pass, new_params, new_salt)
        .expect("rotate-kdf must succeed for a passphrase-sourced vault");
    let after = VaultFile::decode(&rotated).expect("decode rotated vault");

    // payload 一字不动。
    assert_eq!(
        after.payload_nonce, before.payload_nonce,
        "rotate-kdf must not touch payload nonce"
    );
    assert_eq!(
        after.ciphertext, before.ciphertext,
        "rotate-kdf must not touch payload ciphertext"
    );

    // 包裹槽 KDF 参数 / salt 已更新。
    let after_slot = after.header.primary_slot().expect("after slot");
    assert_eq!(
        after_slot.kdf_params,
        Some(new_params),
        "rotate-kdf must refresh the slot KDF params"
    );
    assert_eq!(
        after_slot.salt.as_deref(),
        Some(new_salt.as_slice()),
        "rotate-kdf must refresh the slot salt"
    );

    // F-9 通过判定的核心（载荷回路）：用**新参数 + 新 salt** 从盘上字节重派生主密钥 → 解锁
    // 旋转后的 vault → 仍解出**原 payload**（同一 data-key）。若 rotate-kdf 写入的
    // wrapped_data_key 在新参数下解不出（例如重包裹用错派生密钥），此处必红——这是「换
    // argon2id 参数后同一 data-key 仍可解原 payload」一半此前完全未被钉死的断言。
    let unlocked = passphrase_unlock_from_bytes(&pass, &rotated)
        .expect("rotated vault must unlock via the new params/salt derive-then-unlock chain");
    assert_eq!(
        unlocked.payload().secret_refs(),
        vec!["db-main/readonly".to_string()],
        "rotate-kdf must still decrypt to the original payload under the new KDF params"
    );

    // 旧参数 / 旧 salt 不再能解锁旋转后的 vault（换 KDF 语义成立、包裹槽确已更换）。
    assert_unlock_refused(passphrase_unlock_from_bytes_with(
        &pass, &rotated, old_params, old_salt,
    ));
}

// ════════════════════════════════════════════════════════════════════════
//  L-2 / fail-closed：被篡改保险箱的病态 m_cost → 调用 argon2 之前即拒锁
//  （防 unlock 期 OOM 拒绝服务；argon2 0.5.x Params::new 不设 m_cost 上限）
// ════════════════════════════════════════════════════════════════════════

/// §8 L-2 / fail-closed（安全 + 稳定性）：一个被篡改保险箱文件把 passphrase 槽的 `m_cost`
/// 写成接近 `u32::MAX`（≈4 TiB 内存）。合法者持正确口令尝试解锁时，参数范围校验必须在
/// **触碰 argon2 之前**拒绝，返回 `KdfParamsOutOfRange`——绝不据此申请大内存（防 OOM DoS）。
///
/// 取证方式：直接把越界 `m_cost` 喂给 passphrase 来源 `obtain`（即 unlock 路径喂 argon2 的
/// 那一步），断言它**快速**返回确切错误而非进入 argon2 的内存分配。若校验缺失，这一步会
/// 触发 argon2 据 ~4 TiB m_cost 申请内存 → 被内存上限包裹 cgroup OOM-kill（返回非零）；
/// 加了校验后则即时返回 `KdfParamsOutOfRange`，不分配任何 KDF 内存。
#[test]
fn oversized_m_cost_is_rejected_before_argon2_runs() {
    use postern_secrets::unlock::passphrase::{Argon2Params, Passphrase};
    use postern_secrets::unlock::source::MasterKeySource;

    let pathological = Argon2Params {
        m_cost: u32::MAX, // ≈4 TiB —— 被篡改文件的病态值
        t_cost: 1,
        p_cost: 1,
    };
    let src = Passphrase::new(
        Zeroizing::new(b"a strong unlock phrase".to_vec()),
        b"salt-aaaa-16byte".to_vec(),
        pathological,
    );
    match src.obtain() {
        Ok(_) => panic!("oversized m_cost must be rejected, not used to derive a key"),
        Err(e) => assert_eq!(
            e,
            UnlockError::KdfParamsOutOfRange,
            "unlock must fail-closed with KdfParamsOutOfRange before argon2 allocates memory"
        ),
    }
}

/// §8 L-2 / fail-closed：经**完整盘上字节解锁链**驱动——构造一个合法 passphrase vault，
/// 再把明文头里 KDF 区的 `m_cost` 字节篡改成 `1 << 30`（≈1 TiB 内存）。合法者持正确口令、
/// 从盘上字节读回（被篡改的）参数尝试解锁时，必须在跑 argon2 之前拒锁、不分配大内存。
///
/// 模拟真实威胁：攻击者改盘上 vault 文件的 m_cost 字段，网关 unlock 时不应 OOM 崩溃。
#[test]
fn tampered_on_disk_m_cost_field_fails_closed_before_argon2() {
    let pass = Zeroizing::new(b"a strong unlock phrase".to_vec());
    let params = test_kdf_params(); // m_cost=8（小、安全）
    let salt = b"salt-aaaa-16byte";
    let (bytes, _) = build_passphrase_vault(&pass, params, salt);

    // 基线：未篡改时完整解锁链成功（小 m_cost，快速）。
    assert!(
        passphrase_unlock_from_bytes(&pass, &bytes).is_ok(),
        "untampered small-m_cost passphrase vault must unlock"
    );

    // 定位明文头 KDF 区的 m_cost 4 字节并改成病态大值。布局（见 format::encode_slot）：
    // magic(5) + version(1) + source(1) + has_kdf(1) + m_cost(4 LE) + ...
    let m_cost_off = MAGIC.len() + 1 + 1 + 1;
    let mut tampered = bytes.clone();
    let pathological: u32 = 1 << 30; // ≈1 TiB 内存
    for (k, b) in pathological.to_le_bytes().iter().enumerate() {
        if let Some(slot) = tampered.get_mut(m_cost_off + k) {
            *slot = *b;
        }
    }

    // 完整解锁链从盘上字节读回被篡改的 m_cost → 必在跑 argon2 之前拒锁（不返回句柄、不 OOM）。
    assert_unlock_err(
        passphrase_unlock_from_bytes(&pass, &tampered),
        UnlockError::KdfParamsOutOfRange,
    );
}

/// §8 F-2：production 量级的合法参数（19~64 MiB）仍被接受——上限不能误伤正常配置。
/// 这里用 64 MiB（65_536 KiB）的边界值之内取一个明确合法值验证校验不过严。
/// （注：本测试会真实跑一次 64 MiB argon2，内存上限包裹下安全；为保持 vault 套件快速，
/// 仅以一次小迭代验证「合法上界被接受」。）
#[test]
fn production_grade_params_are_accepted() {
    use postern_secrets::unlock::passphrase::{Argon2Params, Passphrase};
    use postern_secrets::unlock::source::MasterKeySource;

    let prod = Argon2Params {
        m_cost: 65_536, // 64 MiB —— production 默认上界，必须被接受
        t_cost: 3,
        p_cost: 1,
    };
    let src = Passphrase::new(
        Zeroizing::new(b"a strong unlock phrase".to_vec()),
        b"salt-aaaa-16byte".to_vec(),
        prod,
    );
    let key = src
        .obtain()
        .expect("production-grade params must be accepted (not over-rejected)");
    assert_eq!(key.len(), 32, "accepted params must still derive a 32B key");
}

// ── passphrase-来源 vault 构造（仅 rotate-kdf 测试用） ─────────────────────

fn test_kdf_params() -> postern_secrets::vault::header::KdfParams {
    postern_secrets::vault::header::KdfParams {
        m_cost: 8,
        t_cost: 1,
        p_cost: 1,
    }
}

/// 完整 passphrase 解锁链（威胁模型取证用）：从**盘上字节**读回 salt/params → argon2id
/// 派生主密钥 → `vault::unlock`。模拟"攻击者改盘上明文头字节、合法者持正确口令仍解锁"
/// 的端到端路径——明文头任一字节被篡改都应在此链某一步 fail-closed。
///
/// 任一前置步骤（decode 失败 / salt 或 params 缺失 / KDF 派生失败）即视为拒锁，返回
/// `Err`——这与"无法解锁"语义一致（L-2 只要求"不返回句柄"）。
fn passphrase_unlock_from_bytes(
    passphrase: &Zeroizing<Vec<u8>>,
    bytes: &[u8],
) -> Result<UnlockedVault, UnlockError> {
    use postern_secrets::unlock::passphrase::{Argon2Params, Passphrase};
    use postern_secrets::unlock::source::MasterKeySource;

    // 1) decode：魔数 / 版本 / 结构截断在此 fail-closed（L-1）。
    let vf = VaultFile::decode(bytes)?;
    let slot = vf
        .header
        .primary_slot()
        .ok_or(UnlockError::PayloadDecryptFailed)?;

    // 2) 从（可能被篡改的）盘上字节读回 KDF 参数与 salt；缺失即无法 passphrase 解锁 → 拒。
    let params = slot.kdf_params.ok_or(UnlockError::PayloadDecryptFailed)?;
    let salt = slot.salt.clone().ok_or(UnlockError::PayloadDecryptFailed)?;

    // 3) 用（可能被篡改的）salt/params 派生主密钥——篡改这些字节 → 派生出不同主密钥。
    let src = Passphrase::new(
        passphrase.clone(),
        salt,
        Argon2Params {
            m_cost: params.m_cost,
            t_cost: params.t_cost,
            p_cost: params.p_cost,
        },
    );
    let master = src.obtain()?;

    // 4) 用派生主密钥解锁——错误主密钥 / source(AAD) / 包裹槽材料篡改在此 fail-closed。
    vault::unlock(&master, bytes)
}

/// 用**显式给定的 params/salt**（而非盘上读回的）派生主密钥再解锁 `bytes`。供 rotate-kdf 后
/// 「旧参数 / 旧 salt 不再能解锁旋转后 vault」取证——旋转后包裹槽是按新参数重派生的主密钥
/// 包裹的，故用旧参数派生出的主密钥开不出新包裹槽 → 拒锁。
fn passphrase_unlock_from_bytes_with(
    passphrase: &Zeroizing<Vec<u8>>,
    bytes: &[u8],
    params: postern_secrets::vault::header::KdfParams,
    salt: &[u8],
) -> Result<UnlockedVault, UnlockError> {
    use postern_secrets::unlock::passphrase::{Argon2Params, Passphrase};
    use postern_secrets::unlock::source::MasterKeySource;

    let src = Passphrase::new(
        passphrase.clone(),
        salt.to_vec(),
        Argon2Params {
            m_cost: params.m_cost,
            t_cost: params.t_cost,
            p_cost: params.p_cost,
        },
    );
    let master = src.obtain()?;
    vault::unlock(&master, bytes)
}

/// 构造一个 passphrase 来源的合法 vault：用 argon2id(pass,salt,params) 派生主密钥包裹
/// `DATA_KEY`，再用 data-key 加密 sample payload。返回 `(bytes, VaultFile)`。
fn build_passphrase_vault(
    passphrase: &Zeroizing<Vec<u8>>,
    params: postern_secrets::vault::header::KdfParams,
    salt: &[u8],
) -> (Vec<u8>, VaultFile) {
    use postern_secrets::unlock::passphrase::{Argon2Params, Passphrase};
    use postern_secrets::unlock::source::MasterKeySource;

    let src = Passphrase::new(
        passphrase.clone(),
        salt.to_vec(),
        Argon2Params {
            m_cost: params.m_cost,
            t_cost: params.t_cost,
            p_cost: params.p_cost,
        },
    );
    let master = src.obtain().expect("derive passphrase master key");

    let dk = data_key();
    let (slot_nonce, wrapped) =
        crypto::wrap_data_key(&master, &dk).expect("wrap data-key under derived master key");
    let header = Header {
        format_version: FORMAT_VERSION,
        slots: vec![Slot {
            source: SlotSource::Passphrase,
            kdf_params: Some(params),
            salt: Some(salt.to_vec()),
            nonce_i: slot_nonce,
            wrapped_data_key: wrapped,
        }],
    };

    let plaintext = sample_payload().to_plaintext().expect("serialize payload");
    let mut vf = VaultFile {
        header,
        payload_nonce: [0u8; NONCE_LEN],
        ciphertext: Vec::new(),
    };
    let aad = vf.aad_bytes();
    let (pn, ct) = crypto::encrypt_payload(&dk, &plaintext, &aad).expect("encrypt payload");
    vf.payload_nonce = pn;
    vf.ciphertext = ct;
    let bytes = vf.encode();
    (bytes, vf)
}

// ── 写入用明文头（write_atomic 入参；KeyFile 单槽，包裹由 write_atomic 内部填实） ──

fn header_for_write(_dk: &Zeroizing<[u8; 32]>) -> Header {
    // write_atomic 内部会用 master_key 重包裹 data_key 并填 slot；这里给出来源判别与空槽位。
    Header {
        format_version: FORMAT_VERSION,
        slots: vec![Slot {
            source: SlotSource::KeyFile,
            kdf_params: None,
            salt: None,
            nonce_i: [0u8; NONCE_LEN],
            wrapped_data_key: Vec::new(),
        }],
    }
}

// ── 临时目录（不引第三方 tempfile，用进程 pid + 计数器在 std::env::temp_dir 下建目录） ──

fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("postern-vault-test-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&p).expect("create temp dir");
    p
}
