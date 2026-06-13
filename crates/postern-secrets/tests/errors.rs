//! 机密面错误词汇基底的行为测试（RED）。
//!
//! 被测对象：`postern_secrets::error::{UnlockError, ResolveError}`。
//! 这两个枚举是本 crate 其余单元的错误词汇基底，本测试只钉它们自身的行为：
//! 每变体存在且彼此可判别、`Display`/`Debug` 文案为常量英文、不含真实地址/
//! 凭据/账号明文、不插值外部输入（§8 L-11，红线 7.2-1：跨 crate 边界前已脱敏）。
//!
//! 覆盖 §8 条目：F-1 / L-1 / L-2（解锁失败返回类型与分支）、F-4 / L-5（地址解析
//! 失败返回类型）、L-11（错误侧明文不出边界）。本单元不依赖兄弟单元，不构造机密类型。

use postern_secrets::error::{ResolveError, UnlockError};

/// 跨 crate 边界前错误必须已脱敏：错误文案里绝不能出现的敏感子串清单
/// （真实地址 / 凭据 / 账号明文样本）。L-11、红线 7.2-1。
/// 注意：本数组本身也是测试夹具，受全波次雷区约束——不含任何裸数据库写标记。
const FORBIDDEN_SUBSTRINGS: &[&str] = &[
    "10.0.3.17", // 详细设计 §6.3 点名的真实地址样本，绝不外泄
    "connection refused to",
    "i-0abc",   // 详细设计 5.4 payload 里的 instance_id 样本
    "password", // payload secrets 字段名
    "key_pem",  // payload secrets 字段名
    "5432",     // 真实端口样本
    "readonly", // tier/账号样本
    "readwrite",
];

// ── UnlockError：解锁失败面（§8 F-1 / L-1 / L-2） ──────────────────────────

/// §8 F-1：obtain 失败有专属变体，Display 为该常量英文文案。
#[test]
fn obtain_failed_displays_constant_master_key_source_message() {
    assert_eq!(
        UnlockError::ObtainFailed.to_string(),
        "master key source unavailable"
    );
}

/// §8 L-1：format_version 不识别是独立变体，Display 为该常量英文文案。
#[test]
fn unknown_format_version_displays_constant_version_message() {
    assert_eq!(
        UnlockError::UnknownFormatVersion.to_string(),
        "vault format version not recognized"
    );
}

/// §8 L-2：AAD 校验失败（头部篡改/降级）是独立变体，Display 为该常量英文文案。
#[test]
fn aad_mismatch_displays_constant_integrity_message() {
    assert_eq!(
        UnlockError::AadMismatch.to_string(),
        "vault header integrity check failed"
    );
}

/// §8 F-1：payload 解密失败是独立变体，Display 为该常量英文文案。
#[test]
fn payload_decrypt_failed_displays_constant_decrypt_message() {
    assert_eq!(
        UnlockError::PayloadDecryptFailed.to_string(),
        "vault payload decryption failed"
    );
}

/// §8 F-1：KDF 派生失败（仅 passphrase 来源）是独立变体，Display 为该常量英文文案。
#[test]
fn kdf_failed_displays_constant_kdf_message() {
    assert_eq!(UnlockError::KdfFailed.to_string(), "key derivation failed");
}

/// §8 L-2 / fail-closed：KDF 参数越界（被篡改保险箱的病态 m_cost）是独立变体，Display 为
/// 该常量英文文案——消费侧据此与「派生失败」区分（前者是 argon2 前的范围拒绝）。
#[test]
fn kdf_params_out_of_range_displays_constant_message() {
    assert_eq!(
        UnlockError::KdfParamsOutOfRange.to_string(),
        "key derivation parameters out of accepted range"
    );
}

/// §8 L-1/L-2：五个解锁失败分支两两可判别——AAD 篡改与版本不识别绝不能塌成同一值，
/// 否则消费侧无法区分"降级攻击"与"未知格式"。
#[test]
fn unlock_error_variants_are_pairwise_distinct() {
    let variants = [
        UnlockError::ObtainFailed,
        UnlockError::UnknownFormatVersion,
        UnlockError::AadMismatch,
        UnlockError::PayloadDecryptFailed,
        UnlockError::KdfFailed,
        UnlockError::KdfParamsOutOfRange,
    ];
    for i in 0..variants.len() {
        for j in (i + 1)..variants.len() {
            assert_ne!(
                variants[i], variants[j],
                "UnlockError variants {i} and {j} must not be equal"
            );
        }
    }
}

/// §8 L-11：UnlockError 每个变体的 Display 文案均不含真实地址/凭据/账号明文。
#[test]
fn unlock_error_display_contains_no_secret_substrings() {
    let variants = [
        UnlockError::ObtainFailed,
        UnlockError::UnknownFormatVersion,
        UnlockError::AadMismatch,
        UnlockError::PayloadDecryptFailed,
        UnlockError::KdfFailed,
        UnlockError::KdfParamsOutOfRange,
    ];
    for v in &variants {
        let rendered = v.to_string();
        for needle in FORBIDDEN_SUBSTRINGS {
            assert!(
                !rendered.contains(needle),
                "UnlockError Display {rendered:?} must not contain secret substring {needle:?}"
            );
        }
    }
}

/// §8 L-11：UnlockError 每个变体的 Debug 文案均不含真实地址/凭据/账号明文。
#[test]
fn unlock_error_debug_contains_no_secret_substrings() {
    let variants = [
        UnlockError::ObtainFailed,
        UnlockError::UnknownFormatVersion,
        UnlockError::AadMismatch,
        UnlockError::PayloadDecryptFailed,
        UnlockError::KdfFailed,
        UnlockError::KdfParamsOutOfRange,
    ];
    for v in &variants {
        let rendered = format!("{v:?}");
        for needle in FORBIDDEN_SUBSTRINGS {
            assert!(
                !rendered.contains(needle),
                "UnlockError Debug {rendered:?} must not contain secret substring {needle:?}"
            );
        }
    }
}

// ── ResolveError：地址解析失败面（§8 F-4 / L-5） ──────────────────────────

/// §8 F-4/L-5：未知代号是独立变体，Display 为该常量英文文案（无产物、fail-closed）。
#[test]
fn unknown_code_displays_constant_no_target_message() {
    assert_eq!(
        ResolveError::UnknownCode.to_string(),
        "no target for requested resource code"
    );
}

/// §8 L-5：库/保险箱不可用是独立变体，Display 为该常量英文文案
/// （用"unavailable"表述，绝不写裸数据库写标记词）。
#[test]
fn vault_unavailable_displays_constant_unavailable_message() {
    assert_eq!(
        ResolveError::VaultUnavailable.to_string(),
        "vault unavailable"
    );
}

/// §8 F-4/L-5：未知代号与不可用两个失败分支可判别——下游 connpool 据此 deny，
/// 二者不可塌成同一值。
#[test]
fn resolve_error_variants_are_distinct() {
    assert_ne!(ResolveError::UnknownCode, ResolveError::VaultUnavailable);
}

/// §8 L-11：ResolveError 每个变体的 Display 文案均不含真实地址/凭据/账号明文。
#[test]
fn resolve_error_display_contains_no_secret_substrings() {
    let variants = [ResolveError::UnknownCode, ResolveError::VaultUnavailable];
    for v in &variants {
        let rendered = v.to_string();
        for needle in FORBIDDEN_SUBSTRINGS {
            assert!(
                !rendered.contains(needle),
                "ResolveError Display {rendered:?} must not contain secret substring {needle:?}"
            );
        }
    }
}

/// §8 L-11：ResolveError 每个变体的 Debug 文案均不含真实地址/凭据/账号明文。
#[test]
fn resolve_error_debug_contains_no_secret_substrings() {
    let variants = [ResolveError::UnknownCode, ResolveError::VaultUnavailable];
    for v in &variants {
        let rendered = format!("{v:?}");
        for needle in FORBIDDEN_SUBSTRINGS {
            assert!(
                !rendered.contains(needle),
                "ResolveError Debug {rendered:?} must not contain secret substring {needle:?}"
            );
        }
    }
}

// ── 错误词汇作为类型契约（实现/Clone/Eq 是设计承诺的一部分） ──────────────

/// UnlockError 是 std::error::Error（thiserror 派生）——可作为 `?` 传播的错误类型，
/// 是其余单元解锁路径返回签名 `Result<_, UnlockError>` 的基底。
#[test]
fn unlock_error_is_std_error() {
    fn assert_is_error<E: std::error::Error>(_: &E) {}
    assert_is_error(&UnlockError::ObtainFailed);
}

/// ResolveError 是 std::error::Error——是 `resolve(code) -> Result<_, ResolveError>`
/// 返回签名的基底。
#[test]
fn resolve_error_is_std_error() {
    fn assert_is_error<E: std::error::Error>(_: &E) {}
    assert_is_error(&ResolveError::UnknownCode);
}
