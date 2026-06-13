//! 解锁来源族 `MasterKeySource` 四来源的行为测试（RED）。
//!
//! 被测对象：`postern_secrets::unlock`——`MasterKeySource` trait（本域定义，§5.2 /
//! 详细设计 4.3）及四实现 `Passphrase`/`KeyFile`/`OsKeychain`/`SystemdCred`。
//!
//! 覆盖 §8 条目：
//! - F-2（MasterKeySource 四来源）：`obtain(&self) -> Result<Zeroizing<[u8;32]>, UnlockError>`
//!   签名一致且存在；四实现各自存在；仅 `passphrase` 经 argon2id KDF 派生（行为：
//!   同口令+同 salt/参数 → 同一 32B），其余直接持有 32B（行为：给定 32B → 原样返回）。
//! - B-8（解锁强度诚实表述）：四来源强度表述落到来源实现的文档注释——仅 passphrase
//!   标注 argon2id KDF 派生 + 与无人值守互斥；key_file/os_keychain/systemd_cred 标注
//!   "直接持有 32B 主密钥、强度等于文件/钥匙串/凭据保护"；key_file 标注弱模式威胁后果。
//!   人工 checklist 逐项 yes 即过，本测试把每项落成文本级结构检查（读源文件断言标记存在）。
//!
//! 测试策略（§3.1）：`MasterKeySource` 用 Fake/可控来源驱动——直接持有型来源以固定 32B
//! 构造即天然可控，无需真实钥匙串/systemd/TPM；passphrase 以固定口令+salt+参数驱动。
//! 雷区：本文件含注释与字符串均不含任何裸数据库写标记；不构造机密类型。

use postern_secrets::error::UnlockError;
use postern_secrets::unlock::key_file::KeyFile;
use postern_secrets::unlock::os_keychain::OsKeychain;
use postern_secrets::unlock::passphrase::{Argon2Params, Passphrase};
use postern_secrets::unlock::source::MasterKeySource;
use postern_secrets::unlock::systemd_cred::SystemdCred;
use zeroize::Zeroizing;

// ── 固定测试材料（可控、确定，不碰真实来源） ──────────────────────────────

/// 固定 32B 主密钥样本之一（直接持有型来源的输入；非全零，验"原样返回"非"清零返回"）。
const MASTER_KEY_A: [u8; 32] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
    0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
];

/// 固定 32B 主密钥样本之二（与样本一不同，验不同输入产出不同）。
const MASTER_KEY_B: [u8; 32] = [
    0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf,
    0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd, 0xbe, 0xbf,
];

/// argon2id 测试参数（低成本，使测试快；强度由参数与口令熵决定，此处只验确定性，不验强度）。
fn test_params() -> Argon2Params {
    Argon2Params {
        m_cost: 8, // KiB，argon2 最小内存阈值之上
        t_cost: 1,
        p_cost: 1,
    }
}

fn passphrase_source(secret: &[u8], salt: &[u8]) -> Passphrase {
    Passphrase::new(
        Zeroizing::new(secret.to_vec()),
        salt.to_vec(),
        test_params(),
    )
}

// ── §8 F-2：trait 签名一致 + 存在（设计承诺级） ────────────────────────────

/// §8 F-2：`MasterKeySource::obtain` 的签名恰为 `fn(&self) -> Result<Zeroizing<[u8;32]>, UnlockError>`。
/// 用一个接受 `&dyn MasterKeySource` 的函数固定签名——若签名漂移（async、改返回类型、
/// 改密钥长度）则本测试不编译。
#[test]
fn master_key_source_obtain_signature_is_sync_32byte_result() {
    fn call_obtain(src: &dyn MasterKeySource) -> Result<Zeroizing<[u8; 32]>, UnlockError> {
        src.obtain()
    }
    let src = KeyFile::new(Zeroizing::new(MASTER_KEY_A));
    let key = call_obtain(&src).expect("key_file obtain must succeed");
    assert_eq!(key.len(), 32, "master key must be exactly 32 bytes");
}

/// §8 F-2：四实现都实现 `MasterKeySource`——可统一塞进 `&dyn MasterKeySource` 切片驱动。
/// 这是"四来源各自存在且收敛于同一 trait"的编译期 + 行为表达。
#[test]
fn all_four_sources_implement_master_key_source_trait() {
    let pass = passphrase_source(b"correct horse battery staple", b"unlock-salt-16by");
    let key_file = KeyFile::new(Zeroizing::new(MASTER_KEY_A));
    let keychain = OsKeychain::new(Zeroizing::new(MASTER_KEY_A));
    let systemd = SystemdCred::new(Zeroizing::new(MASTER_KEY_A));

    let sources: [&dyn MasterKeySource; 4] = [&pass, &key_file, &keychain, &systemd];
    for src in sources {
        let key = src.obtain().expect("each source must obtain a 32B key");
        assert_eq!(key.len(), 32);
    }
}

// ── §8 F-2：直接持有型来源——给定 32B → 原样返回（无 KDF、无变换） ──────────

/// §8 F-2：key_file 直接持有——`obtain` 原样返回构造时给定的 32B，逐字节相等。
#[test]
fn key_file_obtain_returns_given_master_key_verbatim() {
    let src = KeyFile::new(Zeroizing::new(MASTER_KEY_A));
    let key = src.obtain().expect("key_file obtain must succeed");
    assert_eq!(
        *key, MASTER_KEY_A,
        "key_file must return the held 32B unchanged"
    );
}

/// §8 F-2：os_keychain 直接持有——`obtain` 原样返回构造时给定的 32B。
#[test]
fn os_keychain_obtain_returns_given_master_key_verbatim() {
    let src = OsKeychain::new(Zeroizing::new(MASTER_KEY_A));
    let key = src.obtain().expect("os_keychain obtain must succeed");
    assert_eq!(
        *key, MASTER_KEY_A,
        "os_keychain must return the held 32B unchanged"
    );
}

/// §8 F-2：systemd_cred 直接持有——`obtain` 原样返回构造时给定的 32B。
#[test]
fn systemd_cred_obtain_returns_given_master_key_verbatim() {
    let src = SystemdCred::new(Zeroizing::new(MASTER_KEY_A));
    let key = src.obtain().expect("systemd_cred obtain must succeed");
    assert_eq!(
        *key, MASTER_KEY_A,
        "systemd_cred must return the held 32B unchanged"
    );
}

/// §8 F-2：直接持有型不引入派生——不同输入 32B → 不同输出 32B（B≠A 时输出亦 B≠A）。
/// 钉死"非凭空清零 / 非常量返回"：换了主密钥，返回值随之变。
#[test]
fn key_file_obtain_reflects_different_master_keys() {
    let a = KeyFile::new(Zeroizing::new(MASTER_KEY_A))
        .obtain()
        .expect("obtain A");
    let b = KeyFile::new(Zeroizing::new(MASTER_KEY_B))
        .obtain()
        .expect("obtain B");
    assert_eq!(*a, MASTER_KEY_A);
    assert_eq!(*b, MASTER_KEY_B);
    assert_ne!(
        *a, *b,
        "different held keys must yield different obtained keys"
    );
}

/// §8 F-2：直接持有型不是 argon2id——同一 32B 经直接持有来源返回的就是该 32B 本身，
/// 不等于把它当口令喂给 argon2id 的派生结果（验"无 KDF 加固"在行为上成立）。
#[test]
fn direct_hold_sources_do_not_run_kdf() {
    let direct = KeyFile::new(Zeroizing::new(MASTER_KEY_A))
        .obtain()
        .expect("direct obtain");
    // 把同样 32B 当口令 + 固定 salt 喂给 passphrase 来源，得到的是 KDF 派生值。
    let derived = passphrase_source(&MASTER_KEY_A, b"unlock-salt-16by")
        .obtain()
        .expect("kdf obtain");
    assert_eq!(*direct, MASTER_KEY_A, "direct-hold returns input untouched");
    assert_ne!(
        *direct, *derived,
        "direct-hold output must differ from argon2id-derived output (no KDF on direct sources)"
    );
}

// ── §8 F-2：passphrase——唯一经 argon2id KDF 派生（确定性） ─────────────────

/// §8 F-2：passphrase 经 argon2id 派生——同口令 + 同 salt + 同参数 → 同一 32B（确定性）。
#[test]
fn passphrase_same_inputs_derive_identical_master_key() {
    let k1 = passphrase_source(b"a strong unlock phrase", b"unlock-salt-16by")
        .obtain()
        .expect("derive 1");
    let k2 = passphrase_source(b"a strong unlock phrase", b"unlock-salt-16by")
        .obtain()
        .expect("derive 2");
    assert_eq!(
        *k1, *k2,
        "argon2id derivation must be deterministic for identical inputs"
    );
}

/// §8 F-2：passphrase 派生对口令敏感——不同口令（同 salt/参数）→ 不同 32B。
#[test]
fn passphrase_different_passphrase_derives_different_master_key() {
    let k1 = passphrase_source(b"passphrase one", b"unlock-salt-16by")
        .obtain()
        .expect("derive 1");
    let k2 = passphrase_source(b"passphrase two", b"unlock-salt-16by")
        .obtain()
        .expect("derive 2");
    assert_ne!(*k1, *k2, "different passphrases must derive different keys");
}

/// §8 F-2：passphrase 派生对 salt 敏感——同口令、不同 salt → 不同 32B
/// （salt 进入 argon2id，确保同口令在不同 vault 不撞同一主密钥）。
#[test]
fn passphrase_different_salt_derives_different_master_key() {
    let k1 = passphrase_source(b"same phrase", b"salt-aaaa-16byte")
        .obtain()
        .expect("derive 1");
    let k2 = passphrase_source(b"same phrase", b"salt-bbbb-16byte")
        .obtain()
        .expect("derive 2");
    assert_ne!(*k1, *k2, "different salts must derive different keys");
}

/// §8 F-2：passphrase 输出恰为 32 字节（argon2id output_len=32，作主密钥）。
#[test]
fn passphrase_derives_exactly_32_bytes() {
    let key = passphrase_source(b"any phrase", b"unlock-salt-16by")
        .obtain()
        .expect("derive");
    assert_eq!(key.len(), 32, "derived master key must be exactly 32 bytes");
}

// ── §8 F-2 失败路径：KDF 失败 → fail-closed（UnlockError，不 panic、不放行） ──

/// §8 F-2 / B-6：argon2id 拒绝过短 salt 时，passphrase 来源 map 为 `UnlockError::KdfFailed`，
/// 绝不 unwrap/panic、绝不返回缺省密钥（fail-closed）。argon2 要求 salt ≥ 8 字节，
/// 给 0 长 salt 触发 KDF 失败分支。
#[test]
fn passphrase_kdf_failure_maps_to_unlock_error_not_panic() {
    let err = passphrase_source(b"phrase", b"")
        .obtain()
        .expect_err("empty salt must fail argon2id, not succeed");
    assert_eq!(
        err,
        UnlockError::KdfFailed,
        "KDF failure must surface as UnlockError::KdfFailed (fail-closed)"
    );
}

// ── §8 B-8：解锁强度诚实表述落到来源实现的文档注释（结构检查 checklist） ────
//
// B-8 是"四项逐条 yes 即过"的人工 checklist；此处把每项落成对源文件的文本级断言，
// 使诚实表述的存在成为可复现判定。读取被测来源实现文件内容逐项核对。

const PASSPHRASE_SRC: &str = include_str!("../src/unlock/passphrase.rs");
const KEY_FILE_SRC: &str = include_str!("../src/unlock/key_file.rs");
const OS_KEYCHAIN_SRC: &str = include_str!("../src/unlock/os_keychain.rs");
const SYSTEMD_CRED_SRC: &str = include_str!("../src/unlock/systemd_cred.rs");

/// 文件是否实际**使用** argon2（依赖纪律 / 强度诚实的核心：argon2 只在 passphrase
/// 实现引入使用）。判依据是真实的代码引用标记（`use argon2`/`argon2::`/`Argon2`），
/// 而非散文里提及"argon2"二字——诚实注释可解释"本来源无 argon2 加固"而不算违规。
fn uses_argon2_code(src: &str) -> bool {
    src.contains("use argon2") || src.contains("argon2::") || src.contains("Argon2")
}

/// §8 B-8（1/4）：仅 passphrase 标注 argon2id KDF 派生，且标注与无人值守互斥。
#[test]
fn b8_passphrase_doc_marks_argon2id_and_unattended_exclusion() {
    assert!(
        PASSPHRASE_SRC.contains("argon2id"),
        "passphrase source must document argon2id KDF derivation"
    );
    assert!(
        PASSPHRASE_SRC.contains("有人值守") && PASSPHRASE_SRC.contains("互斥"),
        "passphrase source must document mutual exclusion with unattended operation"
    );
}

/// §8 B-8（2/4）：key_file 标注"直接持有 32B 主密钥、强度等于文件保护"+ 弱模式威胁后果。
#[test]
fn b8_key_file_doc_marks_direct_hold_filesystem_strength_and_weak_mode() {
    assert!(
        KEY_FILE_SRC.contains("直接持有") && KEY_FILE_SRC.contains("32B"),
        "key_file source must document it directly holds the 32B master key"
    );
    assert!(
        KEY_FILE_SRC.contains("文件系统权限"),
        "key_file strength must be documented as equal to filesystem permissions"
    );
    assert!(
        KEY_FILE_SRC.contains("弱模式") && KEY_FILE_SRC.contains("威胁"),
        "key_file must explicitly document its weak-mode threat consequence"
    );
    assert!(
        !uses_argon2_code(KEY_FILE_SRC),
        "key_file must NOT use argon2 (no KDF hardening — honest strength)"
    );
}

/// §8 B-8（3/4）：os_keychain 标注"直接持有 32B、强度等于 OS 钥匙串保护"，无 argon2。
#[test]
fn b8_os_keychain_doc_marks_direct_hold_keychain_strength() {
    assert!(
        OS_KEYCHAIN_SRC.contains("直接持有") && OS_KEYCHAIN_SRC.contains("32B"),
        "os_keychain source must document it directly holds the 32B master key"
    );
    assert!(
        OS_KEYCHAIN_SRC.contains("钥匙串"),
        "os_keychain strength must be documented as equal to OS keychain protection"
    );
    assert!(
        !uses_argon2_code(OS_KEYCHAIN_SRC),
        "os_keychain must NOT use argon2 (no KDF hardening — honest strength)"
    );
}

/// §8 B-8（4/4）：systemd_cred 标注"直接持有 32B、强度等于 systemd 凭据保护"，无 argon2。
#[test]
fn b8_systemd_cred_doc_marks_direct_hold_credential_strength() {
    assert!(
        SYSTEMD_CRED_SRC.contains("直接持有") && SYSTEMD_CRED_SRC.contains("32B"),
        "systemd_cred source must document it directly holds the 32B master key"
    );
    assert!(
        SYSTEMD_CRED_SRC.contains("凭据保护"),
        "systemd_cred strength must be documented as equal to systemd credential protection"
    );
    assert!(
        !uses_argon2_code(SYSTEMD_CRED_SRC),
        "systemd_cred must NOT use argon2 (no KDF hardening — honest strength)"
    );
}

/// §8 B-8 总账：四来源中**只有** passphrase 提及 argon2 —— 强度诚实的核心不变量
/// （argon2 依赖只在 passphrase 实现引入使用，雷区）。
#[test]
fn b8_only_passphrase_source_uses_argon2() {
    assert!(
        uses_argon2_code(PASSPHRASE_SRC),
        "passphrase is the only argon2 source"
    );
    assert!(!uses_argon2_code(KEY_FILE_SRC));
    assert!(!uses_argon2_code(OS_KEYCHAIN_SRC));
    assert!(!uses_argon2_code(SYSTEMD_CRED_SRC));
}
