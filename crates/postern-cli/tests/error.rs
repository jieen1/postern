//! CLI 错误词汇与退出码映射的行为测试（RED）。
//!
//! 被测对象：`postern_cli::error::{CliError, exit_code, EXIT_OK}`。本枚举是 CLI 其余
//! 单元（command/transport/render/dispatch）把各自可观测退出行为映射上去的共享基底，
//! 本测试只钉它自身的行为——不依赖兄弟单元、不构造任何机密类型、不嵌裸数据库写标记。
//!
//! 钉死的契约（07-postern-cli §3.6 / §3.9 / §8、详细设计 7.1）：
//! - `CliError` 恰建模三类可观测失败（本地语法拒绝 / daemon 不可达 / daemon 错误信封）
//!   外加一个响应解析失败变体——**无**数据面 `Stage` / 拒绝阶段词汇（CLI 不在求值管线）；
//! - 退出码映射：成功 → 0；每类失败 → 互异的非零码（构造每个变体断言其码）；
//! - fail-closed 客户端延续（L-3）：响应解析失败是一等公民失败变体，绝不塌成成功；
//! - 输出只转述事实（L-4 / 公理六）：错误文案不含真实地址 / 凭据 / 账号明文；
//! - `anyhow` 不出现在本模块（结构化库侧错误只用 thiserror）。
//!
//! 本单元不持 §8 第一/二组的具体 F-/L- 条目（那些落在 command/transport/render 单元），
//! 它是「可观测退出行为」的共享地基；相关条目以注释标注其映射来源。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use postern_cli::error::{exit_code, CliError, EXIT_OK};

/// 错误文案跨进程边界呈现前必须只含 daemon 已脱敏事实：绝不能出现的敏感子串清单
/// （真实地址 / 凭据 / 账号明文样本）。L-4、公理六。
/// 注意：本数组本身也是测试夹具，受全波次雷区约束——不含任何裸数据库写标记。
const FORBIDDEN_SUBSTRINGS: &[&str] = &[
    "10.0.3.17", // 真实地址样本，CLI 结构上无从触达，更不应出现在错误文案
    "5432",      // 真实端口样本
    "password",  // 凭据字段名样本
    "key_pem",   // 凭据字段名样本
    "i-0abc",    // instance_id 样本
];

/// 构造四个 `CliError` 变体各一，作为遍历夹具（payload 取常量、不含敏感子串）。
fn all_variants() -> Vec<CliError> {
    vec![
        CliError::LocalReject {
            usage: "usage: postern elevate <principal> --cap <verb> --ttl <duration>".to_string(),
        },
        CliError::DaemonUnreachable,
        CliError::DaemonError {
            code: "conflict".to_string(),
            message: "version conflict; re-read latest version".to_string(),
        },
        CliError::DecodeFailed {
            detail: "missing required field".to_string(),
        },
    ]
}

// ── 类型契约：恰四个失败类别，无数据面拒绝阶段词汇（§3.6 / §3.9） ───────────────

/// §3.6 / §3.9：`CliError` 恰建模三类可观测失败 + 一个响应解析失败变体。穷尽 match 钉死
/// 变体集——增删任一变体即编译失败，从而锁住"恰好这四类、不混入第五类（如数据面 Stage）"。
/// 本测试**不** import 任何 `Stage` / deny-stage 类型（CLI 不在 [0]~[10] 求值管线，§3.9 明示）。
#[test]
fn cli_error_models_exactly_the_four_failure_classes() {
    for v in all_variants() {
        // 穷尽 match：编译期保证变体集恰为这四个；运行期仅做存在性遍历。
        match v {
            CliError::LocalReject { .. } => {}
            CliError::DaemonUnreachable => {}
            CliError::DaemonError { .. } => {}
            CliError::DecodeFailed { .. } => {}
        }
    }
}

/// §3.6 第一类（L-1 映射基底）：本地语法拒绝是独立变体，携带 clap 已渲染的用法文本
/// （纯本地语法事实），Display 为常量英文文案。
#[test]
fn local_reject_is_a_distinct_variant_carrying_usage_text() {
    let usage = "usage: postern elevate <principal> --cap <verb> --ttl <duration>".to_string();
    let err = CliError::LocalReject {
        usage: usage.clone(),
    };
    // payload 原样可取（消费侧据此打印用法）。
    match &err {
        CliError::LocalReject { usage: u } => assert_eq!(u, &usage),
        other => panic!("expected LocalReject, got {other:?}"),
    }
    // Display 是常量类别文案，不把用法串拼进去（用法另行打印）。
    assert_eq!(err.to_string(), "invalid command usage");
}

/// §3.6 第二类（L-2 映射基底）：daemon 不可达是无 payload 的独立变体，Display 为常量英文。
/// 它不携带任何决策结论（无 allow/deny 字段可承载）——结构上即无本地决策可回退。
#[test]
fn daemon_unreachable_is_a_payloadless_distinct_variant() {
    let err = CliError::DaemonUnreachable;
    assert_eq!(err.to_string(), "daemon unreachable");
}

/// §3.6 第三类（L-7 / L-4 映射基底）：daemon 错误信封是独立变体，原样携带信封的
/// `code` / `message` 两字段（逐字转述，不展开、不补全、不重写）。
#[test]
fn daemon_error_carries_envelope_code_and_message_verbatim() {
    let code = "conflict".to_string();
    let message = "version conflict; re-read latest version".to_string();
    let err = CliError::DaemonError {
        code: code.clone(),
        message: message.clone(),
    };
    match &err {
        CliError::DaemonError {
            code: c,
            message: m,
        } => {
            assert_eq!(c, &code, "envelope code must be transcribed verbatim");
            assert_eq!(m, &message, "envelope message must be transcribed verbatim");
        }
        other => panic!("expected DaemonError, got {other:?}"),
    }
    assert_eq!(err.to_string(), "daemon returned error envelope");
}

/// §3.9 / L-3（fail-closed 客户端延续）：响应不可解析是一等公民失败变体——绝不塌成成功，
/// 携带本地解码器给出的类别描述（不回显响应原文）。Display 为常量英文。
#[test]
fn decode_failed_is_a_first_class_failure_variant() {
    let err = CliError::DecodeFailed {
        detail: "missing required field".to_string(),
    };
    match &err {
        CliError::DecodeFailed { detail } => assert_eq!(detail, "missing required field"),
        other => panic!("expected DecodeFailed, got {other:?}"),
    }
    assert_eq!(
        err.to_string(),
        "response did not match shared-type contract"
    );
}

// ── 退出码映射：成功 0；四类失败互异非零（§3.6，本单元核心承诺） ──────────────

/// §3.6：成功路径退出码为 0。
#[test]
fn success_outcome_maps_to_exit_zero() {
    let ok: Result<(), CliError> = Ok(());
    assert_eq!(exit_code(&ok), 0);
    assert_eq!(EXIT_OK, 0, "EXIT_OK 常量必须为 0");
}

/// §3.6：本地语法拒绝映射到一个非零退出码（≠0）。
#[test]
fn local_reject_maps_to_nonzero_exit() {
    let err = CliError::LocalReject {
        usage: "usage: ...".to_string(),
    };
    let code = err.code();
    assert_ne!(code, 0, "LocalReject 必须非零退出");
    // 经 outcome 映射与直接取码一致。
    let outcome: Result<(), CliError> = Err(err);
    assert_eq!(exit_code(&outcome), code);
}

/// §3.6：daemon 不可达映射到一个非零退出码（≠0）。
#[test]
fn daemon_unreachable_maps_to_nonzero_exit() {
    let err = CliError::DaemonUnreachable;
    let code = err.code();
    assert_ne!(code, 0, "DaemonUnreachable 必须非零退出");
    let outcome: Result<(), CliError> = Err(CliError::DaemonUnreachable);
    assert_eq!(exit_code(&outcome), code);
}

/// §3.6：daemon 错误信封映射到一个非零退出码（≠0）；含 `409` 冲突这类信封。
#[test]
fn daemon_error_maps_to_nonzero_exit() {
    let err = CliError::DaemonError {
        code: "conflict".to_string(),
        message: "version conflict; re-read latest version".to_string(),
    };
    let code = err.code();
    assert_ne!(code, 0, "DaemonError 必须非零退出");
    let outcome: Result<(), CliError> = Err(CliError::DaemonError {
        code: "conflict".to_string(),
        message: "version conflict; re-read latest version".to_string(),
    });
    assert_eq!(exit_code(&outcome), code);
}

/// §3.9 / L-3：响应解析失败映射到一个非零退出码（≠0）——fail-closed，绝不当成功（0）。
#[test]
fn decode_failed_maps_to_nonzero_exit() {
    let err = CliError::DecodeFailed {
        detail: "type mismatch".to_string(),
    };
    let code = err.code();
    assert_ne!(
        code, 0,
        "DecodeFailed 必须非零退出（fail-closed，绝不塌成成功 0）"
    );
    let outcome: Result<(), CliError> = Err(CliError::DecodeFailed {
        detail: "type mismatch".to_string(),
    });
    assert_eq!(exit_code(&outcome), code);
}

/// §3.6：四类失败的退出码两两互异——消费侧（脚本/CI）须能据码区分"本地拒绝 vs
/// 不可达 vs daemon 错误 vs 解析失败"四种处置，绝不能塌成同一码。
#[test]
fn the_four_failure_classes_have_pairwise_distinct_codes() {
    let codes: Vec<i32> = all_variants().iter().map(CliError::code).collect();
    for i in 0..codes.len() {
        for j in (i + 1)..codes.len() {
            assert_ne!(
                codes[i], codes[j],
                "failure-class exit codes {i} and {j} must be distinct"
            );
        }
    }
}

/// §3.6：每类失败的退出码都非零（成功唯一占用 0）。
#[test]
fn every_failure_class_code_is_nonzero() {
    for v in all_variants() {
        assert_ne!(v.code(), 0, "failure-class code must be nonzero: {v:?}");
    }
}

// ── 输出只转述事实：错误文案不外泄敏感明文（L-4 / 公理六） ──────────────────

/// L-4 / 公理六：每个变体的 Display 文案均不含真实地址 / 凭据 / 账号明文。
#[test]
fn cli_error_display_contains_no_secret_substrings() {
    for v in all_variants() {
        let rendered = v.to_string();
        for needle in FORBIDDEN_SUBSTRINGS {
            assert!(
                !rendered.contains(needle),
                "CliError Display {rendered:?} must not contain secret substring {needle:?}"
            );
        }
    }
}

/// L-4 / 公理六：每个变体的 Debug 文案均不含真实地址 / 凭据 / 账号明文。
#[test]
fn cli_error_debug_contains_no_secret_substrings() {
    for v in all_variants() {
        let rendered = format!("{v:?}");
        for needle in FORBIDDEN_SUBSTRINGS {
            assert!(
                !rendered.contains(needle),
                "CliError Debug {rendered:?} must not contain secret substring {needle:?}"
            );
        }
    }
}

// ── 错误词汇作为类型契约（thiserror 派生、可 `?` 传播） ───────────────────────

/// 详细设计 7.1：`CliError` 是 `std::error::Error`（thiserror 派生）——可作为 `?` 传播的
/// 结构化库侧错误，是 CLI 各命令路径返回签名 `Result<_, CliError>` 的基底。
/// （`anyhow` 仅在二进制 `main`，本结构化错误不依赖它。）
#[test]
fn cli_error_is_std_error() {
    fn assert_is_error<E: std::error::Error>(_: &E) {}
    assert_is_error(&CliError::DaemonUnreachable);
}
