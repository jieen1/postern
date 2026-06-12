//! Condition-predicate plugin trait (detailed design 4.1; module design
//! 5.2, step [5]). Core only DECLARES the shape; built-in kinds
//! (`rate_limit` / `time_window` / `mode` / `ttl`) and extensions implement
//! it in the data plane.

use crate::domain::EvalContext;
use crate::error::PredicateError;

/// Condition predicate (step [5], an extensible set).
///
/// The `spec` payload is raw JSON (`serde_json::Value`) interpreted by the
/// registered predicate. `Err` or an undecidable verdict counts as "not
/// satisfied" and denies (axiom two) - the verdict is never silently
/// coerced to a grant.
pub trait ConditionPredicate: Send + Sync {
    /// Predicate-registry selection key (`rate_limit` / `time_window` /
    /// `mode` / `ttl`).
    fn kind(&self) -> &'static str;

    /// Evaluates the predicate against the evaluation context and its JSON
    /// spec. `Ok(false)` or `Err` means "condition not satisfied" -> deny.
    fn eval(&self, ctx: &EvalContext, spec: &serde_json::Value) -> Result<bool, PredicateError>;
}
