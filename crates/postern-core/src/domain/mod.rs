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

/// Jurisdiction operating mode. `Ord` is strictness, ascending:
/// `Normal < Observe < Maintain < Freeze` - when several modes apply,
/// taking the maximum takes the strictest (never the loosest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    /// Normal operation.
    Normal,
    /// Read-only posture.
    Observe,
    /// Maintenance posture.
    Maintain,
    /// Everything denied.
    Freeze,
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
