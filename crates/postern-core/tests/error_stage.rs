//! Behavior tests for the error_stage unit: domain error enums and the
//! authoritative "error -> deny stage" mapping (module design §3.4, §8 F-5;
//! detailed design 7.1 error model, 7.2-1 error-string sanitization).
//!
//! F-5 has two halves and BOTH are pinned in this file:
//!
//! - Value level: every variant of every domain error enum is attributed to
//!   exactly one deny stage, serializes/renders as constant English text, and
//!   the variant tables driving these assertions are derived from
//!   successor-chain functions whose exhaustive `match` (no `_ =>` arm) makes
//!   THIS TEST FILE fail to compile whenever a variant is added to an enum.
//!   A new variant therefore cannot silently bypass the table-driven
//!   assertions: it must be spliced into the chain, at which point its stage
//!   attribution, Display text and serde form are all checked.
//! - Structural level: `test_source_*` below scan the implementation source
//!   (embedded via `include_str!`) and fail if the `stage()` / `as_str()`
//!   mappings lose their explicit per-variant arms, grow a `_ =>` wildcard,
//!   if an enum becomes `#[non_exhaustive]`, or if any `#[error("...")]`
//!   text stops being a brace-free constant (red line 7.2-1: no
//!   interpolation of external input into error strings).

use postern_core::error::{
    AuditError, AuthError, ClassifyError, ConstraintError, CredentialError, DiscoverError,
    ExecError, PredicateError, Stage, TransportError,
};

/// Renders an error via `Display`. The `std::error::Error` bound also pins,
/// at compile time, that every domain enum is a real thiserror error.
fn rendered<E: std::error::Error>(e: &E) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------------
// Exhaustiveness pins: successor chains.
//
// Each `next_*` function steps through an enum's variants in declaration
// order using an exhaustive `match` with no `_ =>` arm. Adding a variant to
// an enum makes the corresponding `next_*` a compile error until the new
// variant is spliced into the chain — which feeds it into every table-driven
// test below. This is the strongest no-extra-dependency guard available:
// without it, a new variant misattributed to the wrong stage (or carrying an
// interpolated Display text) would leave the whole suite green.
// ---------------------------------------------------------------------------

/// Walks a successor chain into the complete, duplicate-free variant list.
fn chain<T: PartialEq + std::fmt::Debug>(next: fn(Option<&T>) -> Option<T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    while let Some(v) = next(out.last()) {
        assert!(
            !out.contains(&v),
            "successor chain revisits {v:?}; the chain must list every variant exactly once"
        );
        out.push(v);
    }
    out
}

fn next_stage(s: Option<&Stage>) -> Option<Stage> {
    match s {
        None => Some(Stage::Auth),
        Some(Stage::Auth) => Some(Stage::Classify),
        Some(Stage::Classify) => Some(Stage::Rbac),
        Some(Stage::Rbac) => Some(Stage::Constraint),
        Some(Stage::Constraint) => Some(Stage::Condition),
        Some(Stage::Condition) => Some(Stage::Tier),
        Some(Stage::Tier) => Some(Stage::Transport),
        Some(Stage::Transport) => Some(Stage::Exec),
        Some(Stage::Exec) => Some(Stage::Audit),
        Some(Stage::Audit) => Some(Stage::Discover),
        Some(Stage::Discover) => None,
    }
}

fn all_stage_variants() -> Vec<Stage> {
    chain(next_stage)
}

fn next_auth_error(e: Option<&AuthError>) -> Option<AuthError> {
    match e {
        None => Some(AuthError::InvalidCredential),
        Some(AuthError::InvalidCredential) => Some(AuthError::ExpiredCredential),
        Some(AuthError::ExpiredCredential) => Some(AuthError::RevokedCredential),
        Some(AuthError::RevokedCredential) => Some(AuthError::TrustDomainMismatch),
        Some(AuthError::TrustDomainMismatch) => Some(AuthError::UndeterminableOrigin),
        Some(AuthError::UndeterminableOrigin) => None,
    }
}

fn all_auth_error_variants() -> Vec<AuthError> {
    chain(next_auth_error)
}

fn next_classify_error(e: Option<&ClassifyError>) -> Option<ClassifyError> {
    match e {
        None => Some(ClassifyError::ParseFailed),
        Some(ClassifyError::ParseFailed) => Some(ClassifyError::MultiStatement),
        Some(ClassifyError::MultiStatement) => Some(ClassifyError::UnknownConstruct),
        Some(ClassifyError::UnknownConstruct) => Some(ClassifyError::Unclassifiable),
        Some(ClassifyError::Unclassifiable) => None,
    }
}

fn all_classify_error_variants() -> Vec<ClassifyError> {
    chain(next_classify_error)
}

fn next_constraint_error(e: Option<&ConstraintError>) -> Option<ConstraintError> {
    match e {
        None => Some(ConstraintError::UnknownKind),
        Some(ConstraintError::UnknownKind) => Some(ConstraintError::InvalidSpec),
        Some(ConstraintError::InvalidSpec) => Some(ConstraintError::MissingObjects),
        Some(ConstraintError::MissingObjects) => None,
    }
}

fn all_constraint_error_variants() -> Vec<ConstraintError> {
    chain(next_constraint_error)
}

fn next_predicate_error(e: Option<&PredicateError>) -> Option<PredicateError> {
    match e {
        None => Some(PredicateError::UnknownKind),
        Some(PredicateError::UnknownKind) => Some(PredicateError::InvalidSpec),
        Some(PredicateError::InvalidSpec) => Some(PredicateError::Undecidable),
        Some(PredicateError::Undecidable) => None,
    }
}

fn all_predicate_error_variants() -> Vec<PredicateError> {
    chain(next_predicate_error)
}

fn next_transport_error(e: Option<&TransportError>) -> Option<TransportError> {
    match e {
        None => Some(TransportError::ConnectFailed),
        Some(TransportError::ConnectFailed) => Some(TransportError::HandshakeFailed),
        Some(TransportError::HandshakeFailed) => Some(TransportError::ChannelClosed),
        Some(TransportError::ChannelClosed) => Some(TransportError::CloseFailed),
        Some(TransportError::CloseFailed) => None,
    }
}

fn all_transport_error_variants() -> Vec<TransportError> {
    chain(next_transport_error)
}

fn next_credential_error(e: Option<&CredentialError>) -> Option<CredentialError> {
    match e {
        None => Some(CredentialError::NotFound),
        Some(CredentialError::NotFound) => Some(CredentialError::VaultUnavailable),
        Some(CredentialError::VaultUnavailable) => Some(CredentialError::RefreshFailed),
        Some(CredentialError::RefreshFailed) => Some(CredentialError::InteractiveAuthRequired),
        Some(CredentialError::InteractiveAuthRequired) => None,
    }
}

fn all_credential_error_variants() -> Vec<CredentialError> {
    chain(next_credential_error)
}

fn next_exec_error(e: Option<&ExecError>) -> Option<ExecError> {
    match e {
        None => Some(ExecError::ChannelLost),
        Some(ExecError::ChannelLost) => Some(ExecError::ProtocolViolation),
        Some(ExecError::ProtocolViolation) => Some(ExecError::ExecutionFailed),
        Some(ExecError::ExecutionFailed) => None,
    }
}

fn all_exec_error_variants() -> Vec<ExecError> {
    chain(next_exec_error)
}

fn next_discover_error(e: Option<&DiscoverError>) -> Option<DiscoverError> {
    match e {
        None => Some(DiscoverError::ProbeFailed),
        Some(DiscoverError::ProbeFailed) => Some(DiscoverError::ChannelLost),
        Some(DiscoverError::ChannelLost) => None,
    }
}

fn all_discover_error_variants() -> Vec<DiscoverError> {
    chain(next_discover_error)
}

fn next_audit_error(e: Option<&AuditError>) -> Option<AuditError> {
    match e {
        None => Some(AuditError::WriteFailed),
        Some(AuditError::WriteFailed) => Some(AuditError::StorageUnavailable),
        Some(AuditError::StorageUnavailable) => None,
    }
}

fn all_audit_error_variants() -> Vec<AuditError> {
    chain(next_audit_error)
}

// ---------------------------------------------------------------------------
// Display pin tables (7.2-1): one exhaustive match per enum, mapping every
// variant to its exact constant English text. Adding a variant is a compile
// error here until its constant text is pinned; an interpolating variant
// cannot render byte-identical to any `&'static str` pinned below.
// ---------------------------------------------------------------------------

fn pinned_auth_display(e: &AuthError) -> &'static str {
    match e {
        AuthError::InvalidCredential => "credential invalid",
        AuthError::ExpiredCredential => "credential expired",
        AuthError::RevokedCredential => "credential revoked",
        AuthError::TrustDomainMismatch => "trust domain mismatch",
        AuthError::UndeterminableOrigin => "connection origin undeterminable",
    }
}

fn pinned_classify_display(e: &ClassifyError) -> &'static str {
    match e {
        ClassifyError::ParseFailed => "intent parse failed",
        ClassifyError::MultiStatement => "multiple statements rejected",
        ClassifyError::UnknownConstruct => "unknown construct rejected",
        ClassifyError::Unclassifiable => "intent cannot be reliably classified",
    }
}

fn pinned_constraint_display(e: &ConstraintError) -> &'static str {
    match e {
        ConstraintError::UnknownKind => "unknown constraint kind",
        ConstraintError::InvalidSpec => "invalid constraint spec",
        ConstraintError::MissingObjects => "objects required for constraint check are missing",
    }
}

fn pinned_predicate_display(e: &PredicateError) -> &'static str {
    match e {
        PredicateError::UnknownKind => "unknown predicate kind",
        PredicateError::InvalidSpec => "invalid predicate spec",
        PredicateError::Undecidable => "predicate undecidable",
    }
}

fn pinned_transport_display(e: &TransportError) -> &'static str {
    match e {
        TransportError::ConnectFailed => "transport connect failed",
        TransportError::HandshakeFailed => "transport handshake failed",
        TransportError::ChannelClosed => "transport channel closed",
        TransportError::CloseFailed => "transport close failed",
    }
}

fn pinned_credential_display(e: &CredentialError) -> &'static str {
    match e {
        CredentialError::NotFound => "no credential for requested resource and tier",
        CredentialError::VaultUnavailable => "vault unavailable",
        CredentialError::RefreshFailed => "credential refresh failed",
        CredentialError::InteractiveAuthRequired => "interactive authentication required",
    }
}

fn pinned_exec_display(e: &ExecError) -> &'static str {
    match e {
        ExecError::ChannelLost => "channel lost during execution",
        ExecError::ProtocolViolation => "protocol violation during execution",
        ExecError::ExecutionFailed => "execution failed",
    }
}

fn pinned_discover_display(e: &DiscoverError) -> &'static str {
    match e {
        DiscoverError::ProbeFailed => "discovery probe failed",
        DiscoverError::ChannelLost => "channel lost during discovery",
    }
}

fn pinned_audit_display(e: &AuditError) -> &'static str {
    match e {
        AuditError::WriteFailed => "audit write failed",
        AuditError::StorageUnavailable => "audit storage unavailable",
    }
}

// ---------------------------------------------------------------------------
// Stage vocabulary: closed enum aligned with pipeline step names.
// ---------------------------------------------------------------------------

// §8-F-5 — the deny `stage` vocabulary is exactly the pipeline step names,
// one-to-one with the EvalTrace cut-off step.
#[test]
fn test_stage_names_align_with_pipeline_steps() {
    assert_eq!(Stage::Auth.as_str(), "auth");
    assert_eq!(Stage::Classify.as_str(), "classify");
    assert_eq!(Stage::Rbac.as_str(), "rbac");
    assert_eq!(Stage::Constraint.as_str(), "constraint");
    assert_eq!(Stage::Condition.as_str(), "condition");
    assert_eq!(Stage::Tier.as_str(), "tier");
    assert_eq!(Stage::Transport.as_str(), "transport");
    assert_eq!(Stage::Exec.as_str(), "exec");
    assert_eq!(Stage::Audit.as_str(), "audit");
    assert_eq!(Stage::Discover.as_str(), "discover");
}

// §3.4 / §8-F-5 — the stage vocabulary is closed: exactly the ten pipeline
// steps, in pipeline order. Adding a Stage variant fails compilation in
// `next_stage` and must then be reconciled with this canonical list.
#[test]
fn test_stage_vocabulary_is_exactly_the_ten_pipeline_steps() {
    let names: Vec<&str> = all_stage_variants().iter().map(|s| s.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "auth",
            "classify",
            "rbac",
            "constraint",
            "condition",
            "tier",
            "transport",
            "exec",
            "audit",
            "discover",
        ],
        "deny-stage vocabulary must be exactly the pipeline step names, in order"
    );
}

// §8-F-5 — audit `stage` fields serialize as the lowercase step name, so the
// audit dimension cannot drift from the pipeline naming. Swept across ALL
// variants: serde output is pinned to `as_str()` (the single source of step
// naming), so a per-variant `#[serde(rename)]` drift on any stage fails here.
#[test]
fn test_stage_serializes_as_lowercase_step_name() {
    let auth = serde_json::to_string(&Stage::Auth).unwrap();
    let rbac = serde_json::to_string(&Stage::Rbac).unwrap();
    let discover = serde_json::to_string(&Stage::Discover).unwrap();
    assert_eq!(auth, "\"auth\"");
    assert_eq!(rbac, "\"rbac\"");
    assert_eq!(discover, "\"discover\"");
    for s in all_stage_variants() {
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            format!("\"{}\"", s.as_str()),
            "serde form of Stage::{s:?} must equal its as_str pipeline step name"
        );
    }
}

// ---------------------------------------------------------------------------
// Error -> stage attribution: every variant of every enum, via the
// compile-pinned chains. Each of these is a fail-closed path: the error is
// the deny reason and the stage is its audit attribution.
// ---------------------------------------------------------------------------

// §8-F-5 — every AuthError variant (invalid / expired / revoked / trust
// domain mismatch / undeterminable origin) is attributed to the auth stage.
#[test]
fn test_every_auth_error_variant_maps_to_auth_stage() {
    for e in all_auth_error_variants() {
        assert_eq!(e.stage(), Stage::Auth, "{e:?} must be attributed to auth");
    }
}

// §8-F-5 — fail-closed: an expired credential is denied and attributed
// exactly to the auth stage (expiry takes effect at evaluation time).
#[test]
fn test_expired_credential_is_attributed_exactly_to_auth_stage() {
    assert_eq!(AuthError::ExpiredCredential.stage(), Stage::Auth);
}

// §8-F-5 — every ClassifyError variant is attributed to the classify stage
// (whitelist classification: unclassifiable means deny, never a guess).
#[test]
fn test_every_classify_error_variant_maps_to_classify_stage() {
    for e in all_classify_error_variants() {
        assert_eq!(
            e.stage(),
            Stage::Classify,
            "{e:?} must be attributed to classify"
        );
    }
}

// §8-F-5 — every ConstraintError variant is attributed to the constraint
// stage ("cannot decide" is equivalent to "not passed").
#[test]
fn test_every_constraint_error_variant_maps_to_constraint_stage() {
    for e in all_constraint_error_variants() {
        assert_eq!(
            e.stage(),
            Stage::Constraint,
            "{e:?} must be attributed to constraint"
        );
    }
}

// §8-F-5 — every PredicateError variant is attributed to the condition stage
// (Err or undecidable counts as "condition not satisfied").
#[test]
fn test_every_predicate_error_variant_maps_to_condition_stage() {
    for e in all_predicate_error_variants() {
        assert_eq!(
            e.stage(),
            Stage::Condition,
            "{e:?} must be attributed to condition"
        );
    }
}

// §8-F-5 — fail-closed: an undecidable predicate is denied and attributed
// exactly to the condition stage, never resolved as satisfied.
#[test]
fn test_undecidable_predicate_is_attributed_exactly_to_condition_stage() {
    assert_eq!(PredicateError::Undecidable.stage(), Stage::Condition);
}

// §8-F-5 — every TransportError variant is attributed to the transport stage
// (open failure means the channel cannot be established -> deny).
#[test]
fn test_every_transport_error_variant_maps_to_transport_stage() {
    for e in all_transport_error_variants() {
        assert_eq!(
            e.stage(),
            Stage::Transport,
            "{e:?} must be attributed to transport"
        );
    }
}

// §8-F-5 — every CredentialError variant is attributed to the tier stage
// (the credential tier could not be materialized).
#[test]
fn test_every_credential_error_variant_maps_to_tier_stage() {
    for e in all_credential_error_variants() {
        assert_eq!(e.stage(), Stage::Tier, "{e:?} must be attributed to tier");
    }
}

// §8-F-5 — fail-closed: a missing (resource, tier) credential is denied at
// the tier stage; there is no default-credential fallback path.
#[test]
fn test_missing_tier_credential_is_attributed_exactly_to_tier_stage() {
    assert_eq!(CredentialError::NotFound.stage(), Stage::Tier);
}

// §8-F-5 — every ExecError variant is attributed to the exec stage.
#[test]
fn test_every_exec_error_variant_maps_to_exec_stage() {
    for e in all_exec_error_variants() {
        assert_eq!(e.stage(), Stage::Exec, "{e:?} must be attributed to exec");
    }
}

// §8-F-5 — every DiscoverError variant is attributed to the discover stage
// (discovery is not authorization; its failures are its own stage).
#[test]
fn test_every_discover_error_variant_maps_to_discover_stage() {
    for e in all_discover_error_variants() {
        assert_eq!(
            e.stage(),
            Stage::Discover,
            "{e:?} must be attributed to discover"
        );
    }
}

// §8-F-5 — every AuditError variant is attributed to the audit stage
// (not recordable means not grantable for read-only verbs).
#[test]
fn test_every_audit_error_variant_maps_to_audit_stage() {
    for e in all_audit_error_variants() {
        assert_eq!(e.stage(), Stage::Audit, "{e:?} must be attributed to audit");
    }
}

// ---------------------------------------------------------------------------
// Display discipline (7.2-1): constant English text, no interpolation.
// Pinning exact strings proves constancy — a variant that interpolated
// external input could not render byte-identical constant text. The pin
// tables above are exhaustive matches, so EVERY variant (present and future)
// must have its exact constant pinned here.
// ---------------------------------------------------------------------------

// §8-F-5 / 7.2-1 — AuthError Display is constant English text per variant.
#[test]
fn test_auth_error_display_is_constant_english_text() {
    for e in all_auth_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_auth_display(&e),
            "{e:?} Display drifted"
        );
    }
}

// §8-F-5 / 7.2-1 — TransportError Display carries sanitized constant codes
// only: no real address, no raw underlying error string can appear.
#[test]
fn test_transport_error_display_is_sanitized_constant_code() {
    for e in all_transport_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_transport_display(&e),
            "{e:?} Display drifted"
        );
    }
}

// §8-F-5 / 7.2-1 — CredentialError Display names the failure class only;
// no account, token or vault content is interpolated.
#[test]
fn test_credential_error_display_carries_no_secret_material() {
    for e in all_credential_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_credential_display(&e),
            "{e:?} Display drifted"
        );
    }
}

// §8-F-5 / 7.2-1 — the remaining domain errors also render constant English
// text: the intent body (SQL text etc.) is never echoed into error strings.
#[test]
fn test_remaining_error_displays_are_constant_english_text() {
    for e in all_classify_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_classify_display(&e),
            "{e:?} Display drifted"
        );
    }
    for e in all_constraint_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_constraint_display(&e),
            "{e:?} Display drifted"
        );
    }
    for e in all_predicate_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_predicate_display(&e),
            "{e:?} Display drifted"
        );
    }
    for e in all_exec_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_exec_display(&e),
            "{e:?} Display drifted"
        );
    }
    for e in all_discover_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_discover_display(&e),
            "{e:?} Display drifted"
        );
    }
    for e in all_audit_error_variants() {
        assert_eq!(
            rendered(&e),
            pinned_audit_display(&e),
            "{e:?} Display drifted"
        );
    }
}

// ---------------------------------------------------------------------------
// Structural pins (compile-time half of F-5, checked against the embedded
// source text): explicit per-variant arms, no `_ =>` wildcard, enums not
// `#[non_exhaustive]`, error texts brace-free constants. Comment lines are
// stripped before scanning so prose mentioning `_ =>` does not count.
// ---------------------------------------------------------------------------

const ERROR_MOD_SRC: &str = include_str!("../src/error/mod.rs");
const STAGE_SRC: &str = include_str!("../src/error/stage.rs");

/// Drops `//` comments and all whitespace, leaving scannable code text.
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

/// Asserts the stage mapping keeps one explicit `Enum::Variant => Stage::X`
/// arm per variant (so the mapping can never collapse into a wildcard or a
/// binding catch-all without this test going red).
fn assert_explicit_stage_arms<T: std::fmt::Debug>(
    code: &str,
    enum_name: &str,
    stage: Stage,
    variants: &[T],
) {
    for v in variants {
        let needle = format!("{enum_name}::{v:?}=>Stage::{stage:?}");
        assert!(
            code.contains(&needle),
            "F-5 structural pin: src/error/mod.rs must keep the explicit arm \
             `{enum_name}::{v:?} => Stage::{stage:?}`"
        );
    }
}

// §8-F-5 — "缺一变体则编译失败" requires that no `_ =>` wildcard (which would
// silently absorb new variants) exists in the error/stage source, and that
// the enums stay exhaustively matchable (not `#[non_exhaustive]`).
#[test]
fn test_source_has_no_wildcard_arm_and_no_non_exhaustive() {
    for (name, src) in [
        ("src/error/mod.rs", ERROR_MOD_SRC),
        ("src/error/stage.rs", STAGE_SRC),
    ] {
        let code = stripped_code(src);
        assert!(
            !code.contains("_=>"),
            "F-5 structural pin: {name} must not contain a `_ =>` wildcard match arm"
        );
        assert!(
            !code.contains("#[non_exhaustive]"),
            "F-5 structural pin: {name} enums must not be #[non_exhaustive]"
        );
    }
}

// §8-F-5 — every variant keeps its own explicit stage arm, and `as_str` keeps
// one explicit arm per Stage variant. Variant lists come from the
// compile-pinned chains, so this scan grows with the enums automatically.
#[test]
fn test_source_keeps_explicit_per_variant_stage_arms() {
    let code = stripped_code(ERROR_MOD_SRC);
    assert_explicit_stage_arms(&code, "AuthError", Stage::Auth, &all_auth_error_variants());
    assert_explicit_stage_arms(
        &code,
        "ClassifyError",
        Stage::Classify,
        &all_classify_error_variants(),
    );
    assert_explicit_stage_arms(
        &code,
        "ConstraintError",
        Stage::Constraint,
        &all_constraint_error_variants(),
    );
    assert_explicit_stage_arms(
        &code,
        "PredicateError",
        Stage::Condition,
        &all_predicate_error_variants(),
    );
    assert_explicit_stage_arms(
        &code,
        "TransportError",
        Stage::Transport,
        &all_transport_error_variants(),
    );
    assert_explicit_stage_arms(
        &code,
        "CredentialError",
        Stage::Tier,
        &all_credential_error_variants(),
    );
    assert_explicit_stage_arms(&code, "ExecError", Stage::Exec, &all_exec_error_variants());
    assert_explicit_stage_arms(
        &code,
        "DiscoverError",
        Stage::Discover,
        &all_discover_error_variants(),
    );
    assert_explicit_stage_arms(
        &code,
        "AuditError",
        Stage::Audit,
        &all_audit_error_variants(),
    );

    let stage_code = stripped_code(STAGE_SRC);
    for s in all_stage_variants() {
        let needle = format!("Stage::{s:?}=>\"{}\"", s.as_str());
        assert!(
            stage_code.contains(&needle),
            "F-5 structural pin: src/error/stage.rs as_str must keep the explicit arm \
             `Stage::{s:?} => \"{}\"`",
            s.as_str()
        );
    }
}

// 7.2-1 — every variant carries exactly one `#[error("...")]` whose text is
// a brace-free constant: a future `#[error("... {0}")]` (or `{addr}` etc.)
// interpolating external input fails here even before any value-level test.
#[test]
fn test_source_error_texts_are_constant_brace_free_literals() {
    let code = stripped_code(ERROR_MOD_SRC);
    let expected = all_auth_error_variants().len()
        + all_classify_error_variants().len()
        + all_constraint_error_variants().len()
        + all_predicate_error_variants().len()
        + all_transport_error_variants().len()
        + all_credential_error_variants().len()
        + all_exec_error_variants().len()
        + all_discover_error_variants().len()
        + all_audit_error_variants().len();
    let mut found = 0usize;
    let mut rest = code.as_str();
    while let Some(i) = rest.find("#[error(") {
        rest = &rest[i + "#[error(".len()..];
        assert!(
            rest.starts_with('"'),
            "7.2-1: every #[error(...)] must be a plain string literal (no transparent/fmt forms)"
        );
        let end = rest[1..]
            .find('"')
            .expect("unterminated #[error] string literal")
            + 1;
        let text = &rest[1..end];
        assert!(
            !text.contains('{') && !text.contains('}'),
            "7.2-1: #[error] text must be a constant without interpolation, got: {text:?}"
        );
        found += 1;
        rest = &rest[end + 1..];
    }
    assert_eq!(
        found, expected,
        "every domain error variant must carry exactly one constant #[error(\"...\")] attribute"
    );
}
