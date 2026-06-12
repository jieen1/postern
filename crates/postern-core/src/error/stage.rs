//! Closed deny-stage vocabulary (module design §3.4).
//!
//! `Stage` is the system-wide audit dimension for "at which pipeline step a
//! request was denied". It is a closed enum (deliberately NOT
//! `#[non_exhaustive]`): downstream matches must stay exhaustive so that the
//! "error -> stage" mapping promise (no wildcard arms) holds at compile time.
//! Variant names align one-to-one with the evaluation pipeline step names and
//! with the `EvalTrace` cut-off step.

use serde::Serialize;

/// Deny stage, aligned with pipeline step naming:
/// `auth` / `classify` / `rbac` / `constraint` / `condition` / `tier` /
/// `transport` / `exec` / `audit` / `discover`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// Step [1] authentication.
    Auth,
    /// Step [2] intent classification.
    Classify,
    /// Step [3] RBAC grant-cell lookup.
    Rbac,
    /// Step [4] constraint check.
    Constraint,
    /// Step [5] condition predicates.
    Condition,
    /// Step [6] tier selection / credential-tier materialization.
    Tier,
    /// Step [7b] transport channel establishment.
    Transport,
    /// Step [8] execution against the resource.
    Exec,
    /// Audit recording (write failure on read-only verbs denies).
    Audit,
    /// Control-plane discovery (discovery is not authorization).
    Discover,
}

impl Stage {
    /// Canonical pipeline step name carried by audit `stage` fields.
    ///
    /// Exhaustive per-variant match; no `_ =>` arm.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Auth => "auth",
            Stage::Classify => "classify",
            Stage::Rbac => "rbac",
            Stage::Constraint => "constraint",
            Stage::Condition => "condition",
            Stage::Tier => "tier",
            Stage::Transport => "transport",
            Stage::Exec => "exec",
            Stage::Audit => "audit",
            Stage::Discover => "discover",
        }
    }
}
