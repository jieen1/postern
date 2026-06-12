//! `PolicySnapshot` / `CredentialView` - the immutable policy projection
//! the evaluator consumes (module design 3.1; detailed design 6.2, 8.11).
//!
//! TYPE ownership is here; CONSTRUCTION logic is the store's (snapshot
//! build: one transaction, role-inheritance and selector expansion, atomic
//! `Arc` replacement). Containers are deterministic only - `BTreeMap` and
//! `Vec`, never a hash map - so identical snapshots yield identical
//! iteration order, serializations and traces.

use std::collections::BTreeMap;

use super::{Capability, CredentialTier, GrantCell, PrincipalId, ResourceCode, Timestamp};

/// Tier declaration for one resource: which verbs this engine-account
/// credential tier carries. Declaring tiers is the policy state's job;
/// selecting one (verb -> tier at step [6]) is the evaluator's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierDecl {
    /// Tier name, e.g. `"readonly"`.
    pub tier: CredentialTier,
    /// Verbs this tier carries. A verb carried by no tier of the resource
    /// denies - there is no default tier.
    pub carries: Vec<Capability>,
}

/// Gateway-credential metadata for step [1]. Metadata and the secret HASH
/// only - credential plaintext never enters a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialMeta {
    /// Principal this credential authenticates.
    pub principal: PrincipalId,
    /// Authenticator kind it is checked by (`local_process` / `api_key` /
    /// `token` ...).
    pub kind: String,
    /// Hash of the credential plaintext (the plaintext itself is barred
    /// from snapshots and storage alike).
    pub secret_hash: String,
    /// Expiry instant; re-checked against the evaluation wall clock at
    /// step [1] - expiry is effective immediately, not sweeper-dependent.
    pub expires_at: Option<Timestamp>,
    /// Revocation instant, if revoked.
    pub revoked_at: Option<Timestamp>,
}

/// Credential metadata view consumed by `Authenticator::authenticate`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CredentialView {
    /// All loaded gateway-credential metadata rows.
    pub credentials: Vec<CredentialMeta>,
}

/// Immutable projection of the authoritative policy state, atomically
/// replaced as a whole (`Arc` swap in the store). The `Default` value is
/// the empty snapshot, which grants nothing - the deny-everything world
/// (axiom one: absence of a grant cell is a deny).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolicySnapshot {
    /// Policy revision, incremented on every snapshot rebuild - the audit
    /// reconciliation anchor (same rev + same request => same decision).
    pub policy_rev: u64,
    /// Expanded authorization space: per principal, `(resource, verb)` to
    /// its grant cell. Cell-attached constraint/condition declarations
    /// ride on `GrantCell`.
    pub grants: BTreeMap<PrincipalId, BTreeMap<(ResourceCode, Capability), GrantCell>>,
    /// Tier declarations per resource (step [6] tier-selection source).
    pub tiers: BTreeMap<ResourceCode, Vec<TierDecl>>,
    /// Credential metadata view (step [1] input).
    pub credentials: CredentialView,
    /// Operator-prewritten deny notes per `(resource, verb)`; relayed
    /// verbatim as `operator_note`, absent by default.
    pub deny_notes: BTreeMap<(ResourceCode, Capability), String>,
    /// Grantable capability set per resource - the mechanical source of
    /// `request_hint`; a verb absent here yields `request_hint = None`.
    pub grantable: BTreeMap<ResourceCode, Vec<Capability>>,
}
