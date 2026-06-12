//! Capability verbs and the authorization lattice (Resource x Capability):
//! grant cells, attached constraint/condition declarations and the matched
//! grant carried by an allow decision (module design 3.1/5.1).

use serde::Serialize;

use super::{ResourceCode, Role};

/// The closed set of six orthogonal capability verbs (read/write/manage/
/// destroy axes). The authorization lattice is the cartesian decision space
/// Resource x Capability: whether a principal may perform a verb class on a
/// resource is decided solely by grant-cell existence.
///
/// There is deliberately NO seventh "grant-everything" variant: that
/// privilege is not grantable by axiom three, and the enum's shape makes
/// granting it unrepresentable in Rust (contract SEC_ADMIN_NOT_GRANTABLE)
/// instead of checked at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    /// Passive read: logs, status.
    Observe,
    /// Explicit read: data, config, pages.
    Query,
    /// Write data, submit forms.
    Mutate,
    /// Run parameterized command/script templates (never free-form text).
    Execute,
    /// Lifecycle: start/stop/deploy/restart/scale.
    Manage,
    /// Irreversible destruction; granted only per single cell, with a TTL.
    Destroy,
}

impl Capability {
    /// Canonical lowercase verb name - the same text as the serde form -
    /// used to assemble `your_grants` capability-name lists.
    ///
    /// Exhaustive per-variant match; no `_ =>` arm.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Observe => "observe",
            Capability::Query => "query",
            Capability::Mutate => "mutate",
            Capability::Execute => "execute",
            Capability::Manage => "manage",
            Capability::Destroy => "destroy",
        }
    }
}

/// Action annotation on a grant cell, read at pipeline step [6].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantAction {
    /// Granting cell: proceed to tier selection.
    Allow,
    /// Escalation cell: with approval closed it folds to its fallback
    /// (always a deny) - core never holds a pending state.
    Escalate,
}

/// Adapter-interpreted object constraint attached to a grant cell
/// (step [4]; kinds like `table_allow` / `container_prefix`). The spec
/// payload is raw JSON text - only the owning adapter interprets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConstraintSpec {
    /// Constraint kind, declared by the adapter's constraint matrix.
    pub kind: String,
    /// Raw JSON spec payload for the adapter.
    pub spec: String,
}

/// Condition-predicate declaration attached to a grant cell (step [5];
/// built-in kinds: `rate_limit` / `time_window` / `mode` / `ttl`). The
/// spec payload is raw JSON text for the registered predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConditionSpec {
    /// Predicate kind, resolved against the predicate registry.
    pub kind: String,
    /// Raw JSON spec payload for the predicate.
    pub spec: String,
}

/// One expanded cell of the authorization lattice, as materialized into the
/// snapshot by the store (binding x role inheritance, selectors already
/// expanded). Evaluation is pure table lookup over these cells; absence of
/// a cell means deny (axiom one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantCell {
    /// Lattice coordinate: the resource.
    pub resource: ResourceCode,
    /// Lattice coordinate: the verb.
    pub capability: Capability,
    /// Provenance: the role whose expansion produced this cell (deny
    /// reasons and audit cite policy facts such as the role name).
    pub role: Role,
    /// Step [6] routing: allow or escalate.
    pub action: GrantAction,
    /// Object constraints checked at step [4].
    pub constraints: Vec<ConstraintSpec>,
    /// Condition predicates evaluated at step [5].
    pub conditions: Vec<ConditionSpec>,
}

/// The grant cell a request matched - carried inside an allow decision so
/// downstream code can never hold a bare boolean stripped of the granting
/// facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchedGrant {
    /// Matched lattice coordinate: the resource.
    pub resource: ResourceCode,
    /// Matched lattice coordinate: the verb.
    pub capability: Capability,
    /// Provenance: the role whose expansion granted the matched cell.
    pub role: Role,
}
