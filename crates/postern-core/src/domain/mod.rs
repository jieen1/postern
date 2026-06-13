//! Domain vocabulary: participants, objects, authorization structures,
//! policy-snapshot views and the opaque secret-family declarations
//! (module design 3.1/5.1; detailed design 4.1 core types, 8.1).
//!
//! The vocabulary encodes security semantics into type shape so violations
//! are unrepresentable, rather than merely declaring structs: snowflake-backed
//! id types and `ResourceCode` always serialize as JSON strings (JS `Number`
//! is 53-bit safe only), and all aggregate types use deterministic containers
//! (`BTreeMap`/`Vec`, never a hash map) so identical inputs yield identical
//! iteration order, serializations and traces.

pub mod capability;
pub mod secret;
pub mod snapshot;

pub use capability::{
    Capability, ConditionSpec, ConstraintSpec, GrantAction, GrantCell, MatchedGrant,
};
pub use secret::{PresentedCredential, ResolvedTarget, ResourceCredential};
pub use snapshot::{CredentialMeta, CredentialView, PolicySnapshot, TierDecl};

use serde::{Deserialize, Serialize};

use crate::id::SnowflakeId;
use crate::request::ObjectRef;

/// Resource code, e.g. `"db-main"` - always a code name, never a real
/// address (real-address types exist only in the secrets crate).
/// Serializes as a plain JSON string, including when keying JSON maps
/// (`your_grants`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceCode(String);

impl ResourceCode {
    /// Wraps a resource code name.
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    /// The code name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Principal id - a snowflake id; the only id source workspace-wide is
/// `core::id::IdGen`. JSON form is a decimal string in BOTH directions;
/// a JSON number is rejected on deserialization (fail-closed, never a
/// silently truncated 53-bit read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PrincipalId(SnowflakeId);

impl PrincipalId {
    /// Wraps an already-issued snowflake id.
    pub fn new(id: SnowflakeId) -> Self {
        Self(id)
    }

    /// The underlying snowflake id.
    pub fn as_snowflake(self) -> SnowflakeId {
        self.0
    }
}

/// Resource credential-tier name, e.g. `"readonly"` (technical design 10.5;
/// "Tier" is the only word for this concept - no synonyms).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct CredentialTier(String);

impl CredentialTier {
    /// Wraps a tier name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The tier name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Role name on the trust ladder, e.g. `"observer"`. Roles describe verb
/// sets only and are resource-type agnostic (detailed design 5.2bis-1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Role(String);

impl Role {
    /// Wraps a role name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The role name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Binding jurisdiction: which resources a `(principal, role)` binding
/// covers (detailed design 5.2bis-2). Selector expansion happens at
/// snapshot build time in the store; evaluation only ever sees the
/// expanded cells. An unparsable or empty-expanding selector grants
/// nothing (fail-closed: empty set, not an error, never a grant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Enumerated concrete resource codes.
    Resources(Vec<ResourceCode>),
    /// Raw label-selector spec text, matched against resource labels at
    /// snapshot build.
    Selector(String),
}

/// Jurisdiction operating mode (the kill-switch posture).
///
/// Strictness, by pass-set inclusion, is `Normal < Maintain < Observe <
/// Freeze` (technical design 643 / detailed design 377-380): `Observe` is
/// read-only (only `{Observe, Query}`), so it is STRICTER than `Maintain`
/// (which also admits `{Mutate, Execute}`); `Freeze` admits nothing and is
/// strictest. When several modes apply to one request the effective mode is
/// the strictest ([`Mode::meet`]) - never the loosest (L-10 "same
/// jurisdiction, multiple modes -> strictest").
///
/// NOTE: this strictness order is NOT the derived `Ord` (which follows the
/// variant declaration order `Normal < Observe < Maintain < Freeze`); the
/// variant order is kept only for a stable total order on the enum.
/// [`Mode::meet`] therefore ranks strictness explicitly rather than relying
/// on `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    /// Normal operation: no mode restriction (RBAC governs).
    Normal,
    /// Read-only posture: only the read verbs (`Observe`, `Query`) pass.
    Observe,
    /// Maintenance posture: read verbs plus controlled `Mutate` / `Execute`
    /// (deliberately NOT `Manage` / `Destroy`).
    Maintain,
    /// Frozen: every verb is denied.
    Freeze,
}

impl Mode {
    /// Whether `cap` is in this mode's pass set (the mode override admits it).
    /// Mode never *grants* - it can only further restrict what RBAC allows -
    /// so `Normal` admits everything and the lattice narrows from there.
    ///
    /// Exhaustive per-(mode, cap) match; no `_ =>` catch-all on `cap` for the
    /// restrictive modes, so a new verb forces a deliberate decision here
    /// (fail-closed by construction).
    pub fn allows(self, cap: Capability) -> bool {
        match self {
            // Normal: mode imposes no restriction; RBAC is the sole arbiter.
            Mode::Normal => true,
            // Observe: read-only - only the two read verbs.
            Mode::Observe => matches!(cap, Capability::Observe | Capability::Query),
            // Maintain: read verbs plus controlled mutate/execute; manage and
            // destroy stay denied even under maintenance.
            Mode::Maintain => matches!(
                cap,
                Capability::Observe
                    | Capability::Query
                    | Capability::Mutate
                    | Capability::Execute
            ),
            // Freeze: nothing passes.
            Mode::Freeze => false,
        }
    }

    /// The stricter (meet) of two modes - the effective mode when several
    /// apply to the same request (e.g. a global mode and a resource-level
    /// override). Strictness is `Normal < Maintain < Observe < Freeze` by
    /// pass-set inclusion, so the meet keeps the higher-ranked (never the
    /// loosest). Idempotent and commutative.
    pub fn meet(self, other: Mode) -> Mode {
        if self.strictness() >= other.strictness() {
            self
        } else {
            other
        }
    }

    /// Explicit strictness rank (higher = stricter), by pass-set inclusion:
    /// `Normal` (admits all) < `Maintain` (4 verbs) < `Observe` (2 verbs) <
    /// `Freeze` (none). Deliberately NOT the derived `Ord` (variant order),
    /// since `Observe` is read-only and thus stricter than `Maintain`.
    ///
    /// Exhaustive per-variant match; no `_ =>` arm, so a new mode forces a
    /// deliberate rank here.
    fn strictness(self) -> u8 {
        match self {
            Mode::Normal => 0,
            Mode::Maintain => 1,
            Mode::Observe => 2,
            Mode::Freeze => 3,
        }
    }
}

/// Wall-clock instant as milliseconds since the Unix epoch. Evaluation
/// never reads the system clock: `now` is always passed explicitly so the
/// same inputs yield the same decision (determinism, module design 3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Wraps a Unix-epoch millisecond reading.
    pub fn from_unix_ms(ms: u64) -> Self {
        Self(ms)
    }

    /// Milliseconds since the Unix epoch.
    pub fn as_unix_ms(self) -> u64 {
        self.0
    }
}

/// Context for condition-predicate evaluation (pipeline step [5]).
///
/// Defined in `domain` (not `eval`) to break the plugin <-> eval module
/// cycle: plugin traits consume it, the evaluator assembles it. Every fact
/// comes from explicit inputs (request / classified intent / `now` /
/// snapshot facts such as the jurisdiction mode) - there is no implicit
/// source (determinism, module design 3.3 step [5]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalContext {
    /// Authenticated principal (step [1] conclusion).
    pub principal: PrincipalId,
    /// Target resource code from the normalized request.
    pub resource: ResourceCode,
    /// Classified verb (step [2] conclusion).
    pub capability: Capability,
    /// Classified object references (step [2] conclusion).
    pub objects: Vec<ObjectRef>,
    /// Evaluation wall clock, passed explicitly by `evaluate`.
    pub now: Timestamp,
    /// Jurisdiction mode fact from the snapshot.
    pub mode: Mode,
}
