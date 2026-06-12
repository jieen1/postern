//! Domain error enums and the authoritative "error -> deny stage" mapping
//! (module design §3.4; detailed design 7.1 error model, 7.2-1 sanitization).
//!
//! Design promises enforced here:
//! - One thiserror enum per domain failure surface; variants carry only
//!   constant English text / error-code discriminants — never secret types
//!   (resolved targets, resource credentials, presented credentials) and
//!   never raw address strings (red line 7.2-1).
//! - `Display` messages are constant English text; no interpolation of
//!   external input.
//! - Enums are deliberately NOT `#[non_exhaustive]`: the per-enum
//!   `stage()` mapping is an exhaustive per-variant `match` with no `_ =>`
//!   wildcard, so adding a variant without classifying its deny stage is a
//!   compile error (completeness is a compile-time obligation, not a test
//!   obligation).

pub mod stage;

pub use stage::Stage;

use thiserror::Error;

/// Step [1] authentication failures (`Authenticator::authenticate`).
/// Invalid / expired / revoked / trust-domain mismatch / undeterminable
/// origin — every variant resolves to deny (axiom two).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuthError {
    /// Presented credential does not authenticate any principal.
    #[error("credential invalid")]
    InvalidCredential,
    /// Credential `expires_at` is in the past at evaluation wall-clock time.
    #[error("credential expired")]
    ExpiredCredential,
    /// Credential has been revoked.
    #[error("credential revoked")]
    RevokedCredential,
    /// Connection origin falls outside the credential's trust domain.
    #[error("trust domain mismatch")]
    TrustDomainMismatch,
    /// Connection origin cannot be reliably determined.
    #[error("connection origin undeterminable")]
    UndeterminableOrigin,
}

impl AuthError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            AuthError::InvalidCredential => Stage::Auth,
            AuthError::ExpiredCredential => Stage::Auth,
            AuthError::RevokedCredential => Stage::Auth,
            AuthError::TrustDomainMismatch => Stage::Auth,
            AuthError::UndeterminableOrigin => Stage::Auth,
        }
    }
}

/// Step [2] classification failures (`Adapter::classify`). Whitelist
/// classification: anything that cannot be reliably classified denies.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClassifyError {
    /// Intent could not be parsed.
    #[error("intent parse failed")]
    ParseFailed,
    /// Intent contains multiple statements.
    #[error("multiple statements rejected")]
    MultiStatement,
    /// Intent contains a construct unknown to the classifier.
    #[error("unknown construct rejected")]
    UnknownConstruct,
    /// Intent cannot be reliably classified or alters session semantics.
    #[error("intent cannot be reliably classified")]
    Unclassifiable,
}

impl ClassifyError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            ClassifyError::ParseFailed => Stage::Classify,
            ClassifyError::MultiStatement => Stage::Classify,
            ClassifyError::UnknownConstruct => Stage::Classify,
            ClassifyError::Unclassifiable => Stage::Classify,
        }
    }
}

/// Step [4] constraint-check failures (`Adapter::check_constraint`).
/// "Cannot decide" is equivalent to "not passed" — never grants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConstraintError {
    /// Constraint kind is not known to this adapter.
    #[error("unknown constraint kind")]
    UnknownKind,
    /// Constraint spec is malformed.
    #[error("invalid constraint spec")]
    InvalidSpec,
    /// Classified objects are insufficient to decide the constraint.
    #[error("objects required for constraint check are missing")]
    MissingObjects,
}

impl ConstraintError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            ConstraintError::UnknownKind => Stage::Constraint,
            ConstraintError::InvalidSpec => Stage::Constraint,
            ConstraintError::MissingObjects => Stage::Constraint,
        }
    }
}

/// Step [5] condition-predicate failures (`ConditionPredicate::eval`).
/// Err or undecidable counts as "condition not satisfied" (axiom two).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PredicateError {
    /// No predicate is registered for the requested kind.
    #[error("unknown predicate kind")]
    UnknownKind,
    /// Predicate spec is malformed.
    #[error("invalid predicate spec")]
    InvalidSpec,
    /// Predicate cannot be decided from the evaluation context.
    #[error("predicate undecidable")]
    Undecidable,
}

impl PredicateError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            PredicateError::UnknownKind => Stage::Condition,
            PredicateError::InvalidSpec => Stage::Condition,
            PredicateError::Undecidable => Stage::Condition,
        }
    }
}

/// Step [7b] transport failures (`Transport::open` and channel lifecycle).
/// Variants are sanitized error codes only — no real addresses, no raw
/// underlying error strings (red line 7.2-1).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransportError {
    /// Underlying endpoint could not be reached.
    #[error("transport connect failed")]
    ConnectFailed,
    /// Transport-level handshake failed.
    #[error("transport handshake failed")]
    HandshakeFailed,
    /// Channel died (keepalive lost or peer closed).
    #[error("transport channel closed")]
    ChannelClosed,
    /// Closing the underlying channel reported a failure.
    #[error("transport close failed")]
    CloseFailed,
}

impl TransportError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            TransportError::ConnectFailed => Stage::Transport,
            TransportError::HandshakeFailed => Stage::Transport,
            TransportError::ChannelClosed => Stage::Transport,
            TransportError::CloseFailed => Stage::Transport,
        }
    }
}

/// Credential-tier materialization failures
/// (`CredentialProvider::credential_for`). There is no default credential:
/// a missing `(resource, tier)` credential denies at the tier stage.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CredentialError {
    /// No credential is declared for the requested resource and tier.
    #[error("no credential for requested resource and tier")]
    NotFound,
    /// Vault is locked or unreachable.
    #[error("vault unavailable")]
    VaultUnavailable,
    /// Re-login / token refresh for the tier session failed.
    #[error("credential refresh failed")]
    RefreshFailed,
    /// Target system enforces interactive authentication; no long-lived
    /// session can be materialized.
    #[error("interactive authentication required")]
    InteractiveAuthRequired,
}

impl CredentialError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            CredentialError::NotFound => Stage::Tier,
            CredentialError::VaultUnavailable => Stage::Tier,
            CredentialError::RefreshFailed => Stage::Tier,
            CredentialError::InteractiveAuthRequired => Stage::Tier,
        }
    }
}

/// Step [8] execution failures (`Adapter::execute`). Sanitized at the
/// kernel egress before returning; an already-executed request is never
/// reported as deny (detailed design 6.1).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecError {
    /// Channel was lost mid-execution.
    #[error("channel lost during execution")]
    ChannelLost,
    /// Resource protocol was violated.
    #[error("protocol violation during execution")]
    ProtocolViolation,
    /// Resource reported the execution as failed.
    #[error("execution failed")]
    ExecutionFailed,
}

impl ExecError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            ExecError::ChannelLost => Stage::Exec,
            ExecError::ProtocolViolation => Stage::Exec,
            ExecError::ExecutionFailed => Stage::Exec,
        }
    }
}

/// Control-plane discovery failures (`Adapter::discover`).
/// Discovery is not authorization.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiscoverError {
    /// Capability-surface probe failed.
    #[error("discovery probe failed")]
    ProbeFailed,
    /// Channel was lost during discovery.
    #[error("channel lost during discovery")]
    ChannelLost,
}

impl DiscoverError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            DiscoverError::ProbeFailed => Stage::Discover,
            DiscoverError::ChannelLost => Stage::Discover,
        }
    }
}

/// Audit recording failures (`AuditSink::record`). For read-only verbs a
/// failed audit write denies: not recordable means not grantable.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuditError {
    /// Appending the audit event failed.
    #[error("audit write failed")]
    WriteFailed,
    /// Audit storage is unavailable.
    #[error("audit storage unavailable")]
    StorageUnavailable,
}

impl AuditError {
    /// Deny stage attribution. Exhaustive per-variant match; no `_ =>` arm.
    pub fn stage(&self) -> Stage {
        match self {
            AuditError::WriteFailed => Stage::Audit,
            AuditError::StorageUnavailable => Stage::Audit,
        }
    }
}
