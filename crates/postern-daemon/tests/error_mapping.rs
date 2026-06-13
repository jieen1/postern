//! error-mapping 单元行为测试（RED）。
//!
//! 钉住 layer-0 错误词汇的两条承诺（模块文档 06 §3.8、§6.1~§6.3、§8 L-3/L-5/L-6）：
//! 1. `deny_stage(&DownstreamError) -> Stage`：每个下游失败族映射到 **确切** 的 `Stage`
//!    变体；尤其 `Credential`/`Transport`/`Resolve` 三族在 daemon 层统一折叠为 "connect"
//!    拒绝阶段（= `Stage::Transport`），fail-closed、不降级（§8 L-5/L-6）。
//! 2. `OutcomeDegraded`「已执行但审计降级」出口码：携带其底层 `AuditError` cause，
//!    语义与 deny 严格区分（§8 L-3 第③分支：已执行绝不返 deny）。
//!
//! 每条测试只钉一个行为，断言精确到 `Stage` 具体变体 / 确切错误码。
//! 实现为 RED 桩（`deny_stage` 体为 `todo!()`），故调用即 panic → 观察到红。
//!
//! 本文件零 SQL 标记；不构造/字面引用 `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`。

use postern_core::error::{
    AuditError, AuthError, ClassifyError, ConstraintError, CredentialError, DiscoverError,
    ExecError, PredicateError, Stage, TransportError,
};
use postern_daemon::error::{deny_stage, DownstreamError, OutcomeDegraded};
use postern_secrets::error::ResolveError;

// ───────────────────────── §8 L-5：求值任一步判拒 → 短路 deny 且 stage 正确 ─────────────────────────
// 认证/归类/RBAC/细则/条件/建连失败各短路；本单元从 error 词汇层钉「错误族 → 确切 stage」。

// §8 L-5：认证 Err → Deny{stage=auth}。覆盖 AuthError 全部变体，逐一映射 Stage::Auth。
#[test]
fn auth_error_family_maps_to_stage_auth() {
    for variant in [
        AuthError::InvalidCredential,
        AuthError::ExpiredCredential,
        AuthError::RevokedCredential,
        AuthError::TrustDomainMismatch,
        AuthError::UndeterminableOrigin,
    ] {
        let err = DownstreamError::Auth(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Auth,
            "AuthError::{variant:?} 必须映射到 Stage::Auth"
        );
    }
}

// §8 L-5：不可归类 ClassifyError → Deny{stage=classify}。
#[test]
fn classify_error_family_maps_to_stage_classify() {
    for variant in [
        ClassifyError::ParseFailed,
        ClassifyError::MultiStatement,
        ClassifyError::UnknownConstruct,
        ClassifyError::Unclassifiable,
    ] {
        let err = DownstreamError::Classify(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Classify,
            "ClassifyError::{variant:?} 必须映射到 Stage::Classify"
        );
    }
}

// §8 L-5：ConstraintCheck 不过（细则检查失败）→ Deny{stage=constraint}。
#[test]
fn constraint_error_family_maps_to_stage_constraint() {
    for variant in [
        ConstraintError::UnknownKind,
        ConstraintError::InvalidSpec,
        ConstraintError::MissingObjects,
    ] {
        let err = DownstreamError::Constraint(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Constraint,
            "ConstraintError::{variant:?} 必须映射到 Stage::Constraint"
        );
    }
}

// §8 L-5：条件谓词不过 → Deny{stage=condition}。PredicateError 映射 Stage::Condition（非 Predicate）。
#[test]
fn predicate_error_family_maps_to_stage_condition() {
    for variant in [
        PredicateError::UnknownKind,
        PredicateError::InvalidSpec,
        PredicateError::Undecidable,
    ] {
        let err = DownstreamError::Predicate(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Condition,
            "PredicateError::{variant:?} 必须映射到 Stage::Condition"
        );
    }
}

// §8 L-6：连接不可建 → deny{stage=connect}（= Stage::Transport），不降级、不改路。
// 凭据物化失败属 "connect" 折叠的一支：daemon 层把 CredentialError 归 Stage::Transport，
// 区别于 core per-enum stage()（那里 CredentialError -> Stage::Tier）——本测试钉 daemon 语义。
#[test]
fn credential_error_family_maps_to_connect_stage_transport() {
    for variant in [
        CredentialError::NotFound,
        CredentialError::VaultUnavailable,
        CredentialError::RefreshFailed,
        CredentialError::InteractiveAuthRequired,
    ] {
        let err = DownstreamError::Credential(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Transport,
            "CredentialError::{variant:?} 必须折叠到 connect 阶段（Stage::Transport）"
        );
    }
}

// §8 L-6：Transport::open 失败 → acquire Err → deny{stage=connect}（= Stage::Transport）。
#[test]
fn transport_error_family_maps_to_connect_stage_transport() {
    for variant in [
        TransportError::ConnectFailed,
        TransportError::HandshakeFailed,
        TransportError::ChannelClosed,
        TransportError::CloseFailed,
    ] {
        let err = DownstreamError::Transport(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Transport,
            "TransportError::{variant:?} 必须映射到 connect 阶段（Stage::Transport）"
        );
    }
}

// §8 L-6：代号→真实地址解析失败 → deny{stage=connect}（= Stage::Transport），脱敏不含真实地址。
#[test]
fn resolve_error_family_maps_to_connect_stage_transport() {
    for variant in [ResolveError::UnknownCode, ResolveError::VaultUnavailable] {
        let err = DownstreamError::Resolve(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Transport,
            "ResolveError::{variant:?} 必须折叠到 connect 阶段（Stage::Transport）"
        );
    }
}

// §8 L-6：三族（Credential/Transport/Resolve）共享同一 connect 拒绝阶段——逐对相等钉死折叠。
#[test]
fn connect_stage_collapses_credential_transport_resolve_identically() {
    let cred = deny_stage(&DownstreamError::Credential(CredentialError::NotFound));
    let transport = deny_stage(&DownstreamError::Transport(TransportError::ConnectFailed));
    let resolve = deny_stage(&DownstreamError::Resolve(ResolveError::UnknownCode));
    // §8 L-6：建连失败三支不可彼此区分，统一 Stage::Transport（"connect"）。
    assert_eq!(cred, Stage::Transport);
    assert_eq!(transport, Stage::Transport);
    assert_eq!(resolve, Stage::Transport);
    assert_eq!(
        cred, transport,
        "凭据物化失败与通路建立失败须落同一 connect 阶段"
    );
    assert_eq!(
        transport, resolve,
        "通路建立失败与地址解析失败须落同一 connect 阶段"
    );
}

// §8 L-5（执行域）：执行失败 → Deny{stage=exec}（错误→stage 不吞错）。
#[test]
fn exec_error_family_maps_to_stage_exec() {
    for variant in [
        ExecError::ChannelLost,
        ExecError::ProtocolViolation,
        ExecError::ExecutionFailed,
    ] {
        let err = DownstreamError::Exec(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Exec,
            "ExecError::{variant:?} 必须映射到 Stage::Exec"
        );
    }
}

// §8 L-3（审计域）：审计写失败 → Stage::Audit（只读动词审计写失败按 deny{stage=audit} 返回）。
#[test]
fn audit_error_family_maps_to_stage_audit() {
    for variant in [AuditError::WriteFailed, AuditError::StorageUnavailable] {
        let err = DownstreamError::Audit(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Audit,
            "AuditError::{variant:?} 必须映射到 Stage::Audit"
        );
    }
}

// §8（discover 域）：控制面 discover 失败 → Stage::Discover（discovery 非授权）。
#[test]
fn discover_error_family_maps_to_stage_discover() {
    for variant in [DiscoverError::ProbeFailed, DiscoverError::ChannelLost] {
        let err = DownstreamError::Discover(variant.clone());
        assert_eq!(
            deny_stage(&err),
            Stage::Discover,
            "DiscoverError::{variant:?} 必须映射到 Stage::Discover"
        );
    }
}

// §8 L-5/L-6：全 10 族映射一次性核对——把承诺的 (族, stage) 对逐条钉死，
// 防止任何一族被错配（穷尽 match 无 _ => 兜底，错配即此处红）。
//
// 关键：用例表 **不再** 是手维护的硬编码数组（旧实现的盲点：第 11 族经 `_ =>`
// 兜底时不会被发现）。每条用例的 `DownstreamError` 由 `next_downstream` 后继链
// 构造（见下方穷尽 match 无 `_ =>` 的 `next_downstream`），故新增一个 `DownstreamError`
// 变体会让 `next_downstream` 编译失败，逼其拼接进链 → 自动喂入本断言并需声明其 stage。
#[test]
fn every_downstream_family_maps_to_its_promised_stage() {
    for err in all_downstream_variants() {
        let expected = promised_stage(&err);
        assert_eq!(
            deny_stage(&err),
            expected,
            "下游族 {err:?} 的拒绝 stage 必须恰为 {expected:?}"
        );
    }
    // 链长 == DownstreamError 当前变体数：旧硬编码数组曾固定为 10，此处由链动态决定，
    // 新增变体必经 next_downstream（穷尽 match）→ 必有断言覆盖，不会被 `_ =>` 吞掉。
    assert_eq!(
        all_downstream_variants().len(),
        10,
        "DownstreamError 当前应有 10 个下游族；新增族需同步 next_downstream 链与 promised_stage"
    );
}

// ───────────────────────── 穷尽性 + 无 `_ =>` 兜底守护（assertion-1 核心修复） ─────────────────────────
//
// 旧测试只在注释里声称「穷尽 match 无 _ => 兜底」，但无任何断言守护：把
// `DownstreamError::Discover(_) => Stage::Discover` 换成 `_ => Stage::Discover` 后
// 14 个测试全绿——design §8 禁止的 fail-open footgun（未来 DownstreamError 新增族被
// 兜底臂静默映射，而非编译失败）原样存在。本块镜像 core `error_stage.rs` 的双重防线：
// (a) 后继链：穷尽 match 无 `_ =>`，新增变体即编译失败，逼其进链 → 自动喂入值级断言；
// (b) 源文本结构扫描：经 include_str! 嵌入 `src/error.rs`，断言 deny_stage 不含 `_ =>`
//     兜底臂、DownstreamError 未 #[non_exhaustive]、且每族保留显式 `Variant(_) => Stage::X` 臂。
// (b) 直接捕获 reviewer 指出的 mutation：把显式臂换成 `_ =>` 会同时触发 (a) 的链漂移
// 检测（若删变体）与 (b) 的「显式臂缺失」「出现 `_=>`」两条断言变红。

/// 穷尽走 `DownstreamError` 全部变体（声明序），无 `_ =>` 兜底臂：新增一个变体
/// 会让此 `match` 编译失败，直到把它拼接进链——届时它被喂入 [`every_downstream_family_maps_to_its_promised_stage`]
/// 与下方结构扫描，无法静默绕过断言。
fn next_downstream(prev: Option<&DownstreamError>) -> Option<DownstreamError> {
    match prev {
        None => Some(DownstreamError::Auth(AuthError::InvalidCredential)),
        Some(DownstreamError::Auth(_)) => {
            Some(DownstreamError::Classify(ClassifyError::ParseFailed))
        }
        Some(DownstreamError::Classify(_)) => {
            Some(DownstreamError::Constraint(ConstraintError::UnknownKind))
        }
        Some(DownstreamError::Constraint(_)) => {
            Some(DownstreamError::Predicate(PredicateError::UnknownKind))
        }
        Some(DownstreamError::Predicate(_)) => {
            Some(DownstreamError::Credential(CredentialError::NotFound))
        }
        Some(DownstreamError::Credential(_)) => {
            Some(DownstreamError::Transport(TransportError::ConnectFailed))
        }
        Some(DownstreamError::Transport(_)) => {
            Some(DownstreamError::Resolve(ResolveError::UnknownCode))
        }
        Some(DownstreamError::Resolve(_)) => {
            Some(DownstreamError::Exec(ExecError::ExecutionFailed))
        }
        Some(DownstreamError::Exec(_)) => Some(DownstreamError::Audit(AuditError::WriteFailed)),
        Some(DownstreamError::Audit(_)) => {
            Some(DownstreamError::Discover(DiscoverError::ProbeFailed))
        }
        Some(DownstreamError::Discover(_)) => None,
    }
}

/// 由后继链走出完整、无重复的 `DownstreamError` 变体清单。
fn all_downstream_variants() -> Vec<DownstreamError> {
    let mut out: Vec<DownstreamError> = Vec::new();
    while let Some(v) = next_downstream(out.last()) {
        assert!(
            !out.contains(&v),
            "DownstreamError 后继链重访 {v:?}；链必须恰好列出每个变体一次"
        );
        out.push(v);
    }
    out
}

/// 每族承诺的拒绝 stage（穷尽 match 无 `_ =>`：新增变体在此编译失败，逼其声明 stage）。
/// 与 `deny_stage` 的实现 **独立** 维护（一处实现、一处期望），二者一致性由
/// [`every_downstream_family_maps_to_its_promised_stage`] 逐对核对。
fn promised_stage(err: &DownstreamError) -> Stage {
    match err {
        DownstreamError::Auth(_) => Stage::Auth,
        DownstreamError::Classify(_) => Stage::Classify,
        DownstreamError::Constraint(_) => Stage::Constraint,
        DownstreamError::Predicate(_) => Stage::Condition,
        DownstreamError::Credential(_) => Stage::Transport,
        DownstreamError::Transport(_) => Stage::Transport,
        DownstreamError::Resolve(_) => Stage::Transport,
        DownstreamError::Exec(_) => Stage::Exec,
        DownstreamError::Audit(_) => Stage::Audit,
        DownstreamError::Discover(_) => Stage::Discover,
    }
}

/// 嵌入 daemon 错误词汇实现源，供结构扫描断言其 `deny_stage` 无 `_ =>` 兜底臂。
const ERROR_SRC: &str = include_str!("../src/error.rs");

/// 剥去 `//` 行注释与全部空白，留下可扫描的代码文本（散文里提到 `_ =>` 不计）。
fn stripped_code(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        let code = match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        };
        out.extend(code.chars().filter(|c| !c.is_whitespace()));
    }
    out
}

// §8（assertion-1）：「新增下游变体而不写映射臂即编译失败」要求 deny_stage 源中
// 绝无 `_ =>` 兜底臂（它会静默吸收新变体、重开 fail-open footgun），且 DownstreamError
// 不得 #[non_exhaustive]（否则下游 match 失去穷尽性义务）。直接捕获 reviewer 的 mutation：
// `DownstreamError::Discover(_) => Stage::Discover` 换成 `_ => Stage::Discover` → 此处红。
#[test]
fn deny_stage_source_has_no_wildcard_arm_and_enum_not_non_exhaustive() {
    let code = stripped_code(ERROR_SRC);
    assert!(
        !code.contains("_=>"),
        "结构钉死: src/error.rs（deny_stage）绝不可含 `_ =>` 兜底臂——\
         否则未来 DownstreamError 新增族会被静默映射而非编译失败（fail-open footgun）"
    );
    assert!(
        !code.contains("#[non_exhaustive]"),
        "结构钉死: src/error.rs 的 DownstreamError/DaemonError 不可标 #[non_exhaustive]——\
         否则消费侧 match 失去穷尽义务"
    );
}

// §8（assertion-1）：deny_stage 必须为每个 DownstreamError 族保留 **显式**
// `Variant(_) => Stage::X` 臂。变体清单来自编译期钉死的后继链，故此扫描随枚举自动生长：
// 把任一显式臂折叠成 `_ =>`（或错配 stage）都会让对应 needle 缺失而变红。
#[test]
fn deny_stage_source_keeps_explicit_per_variant_arms() {
    let code = stripped_code(ERROR_SRC);
    for err in all_downstream_variants() {
        let stage = promised_stage(&err);
        // 形如 `DownstreamError::Auth(_)=>Stage::Auth`（剥空白后）；与源臂形态一致。
        let needle = format!("{}=>Stage::{stage:?}", variant_pattern(&err));
        assert!(
            code.contains(&needle),
            "结构钉死: src/error.rs 的 deny_stage 必须保留显式臂 `{} => Stage::{stage:?}`",
            variant_pattern(&err)
        );
    }
}

/// 返回某族在 `deny_stage` 源中的 match 模式文本（剥空白形态），如 `DownstreamError::Auth(_)`。
fn variant_pattern(err: &DownstreamError) -> &'static str {
    match err {
        DownstreamError::Auth(_) => "DownstreamError::Auth(_)",
        DownstreamError::Classify(_) => "DownstreamError::Classify(_)",
        DownstreamError::Constraint(_) => "DownstreamError::Constraint(_)",
        DownstreamError::Predicate(_) => "DownstreamError::Predicate(_)",
        DownstreamError::Credential(_) => "DownstreamError::Credential(_)",
        DownstreamError::Transport(_) => "DownstreamError::Transport(_)",
        DownstreamError::Resolve(_) => "DownstreamError::Resolve(_)",
        DownstreamError::Exec(_) => "DownstreamError::Exec(_)",
        DownstreamError::Audit(_) => "DownstreamError::Audit(_)",
        DownstreamError::Discover(_) => "DownstreamError::Discover(_)",
    }
}

// ───────────────────────── §8 L-3：两阶段审计时序——已执行绝不返 deny ─────────────────────────

// §8 L-3 第③分支：有副作用动词已 execute 后 outcome 审计写失败 → 返回「已执行但审计降级」
// 可识别错误码，**绝不返回 deny**。本测试钉 OutcomeDegraded 携带其底层 AuditError cause，
// 且该码语义与任何 deny stage 严格区分（它不参与 deny_stage 映射）。
#[test]
fn outcome_degraded_carries_audit_cause_and_is_not_a_deny() {
    let degraded = OutcomeDegraded {
        cause: AuditError::WriteFailed,
    };
    // 携带触发降级的底层审计写失败族（仅常量码，无机密）。
    assert_eq!(
        degraded.cause,
        AuditError::WriteFailed,
        "OutcomeDegraded 必须携带触发降级的 AuditError cause"
    );
    // 「已执行但审计降级」是成功后的降级码，不是拒绝——其 Display 文案为常量安全码。
    assert_eq!(
        degraded.to_string(),
        "executed but audit downgraded",
        "OutcomeDegraded 文案须为常量安全码（不泄露机密、不可与 deny 混淆）"
    );
}

// §8 L-3：outcome 降级码对不同底层 AuditError cause 各自保真（StorageUnavailable 分支）。
#[test]
fn outcome_degraded_preserves_distinct_audit_cause() {
    let write = OutcomeDegraded {
        cause: AuditError::WriteFailed,
    };
    let storage = OutcomeDegraded {
        cause: AuditError::StorageUnavailable,
    };
    // 不同底层 cause 的降级码必须可区分（用于审计 reason 归因），不能被抹平为同一码。
    assert_ne!(
        write, storage,
        "不同 AuditError cause 的 OutcomeDegraded 须可区分"
    );
    assert_eq!(storage.cause, AuditError::StorageUnavailable);
}
