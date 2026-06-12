//! Decision model: the three-valued `Decision`, the structured
//! `DenyResponse` and the stepwise `EvalTrace` (module design 3.1/5.1;
//! technical design 6.4).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::domain::{Capability, CredentialTier, MatchedGrant, ResourceCode};
use crate::error::Stage;
use crate::request::ObjectRef;

/// Three-valued decision - never a bare boolean, so downstream code can
/// never lose the granting context (allow) or the structured refusal
/// facts (deny).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Granted: the matched cell plus the tier selected for the verb at
    /// step [6] (no matching tier would have denied instead).
    Allow {
        grant: MatchedGrant,
        tier: CredentialTier,
    },
    /// Refused, with the structured response.
    Deny(DenyResponse),
    /// Escalation cell hit; with approval closed it folds to its fallback
    /// (always a deny) - core holds no pending state.
    Escalate { fallback: DenyResponse },
}

/// Anonymized, sanitized facts of what was denied. The sanitization itself
/// is the kernel egress's guarantee, not this type's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeniedFacts {
    /// Resource the request targeted (always a code, never an address).
    pub resource: ResourceCode,
    /// Verb the intent classified into.
    pub capability: Capability,
    /// Objects the intent touched.
    pub objects: Vec<ObjectRef>,
}

/// Structured deny response (technical design 6.4; axiom six: policy facts
/// or operator-prewritten content only - nothing invented).
///
/// The field set is a design promise (module design 5.1): exactly
/// `decision` / `denied` / `reason` / `your_grants` / `request_hint` /
/// `operator_note`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DenyResponse {
    /// Constant `"deny"`.
    pub decision: &'static str,
    /// Anonymized, sanitized denial facts.
    pub denied: DeniedFacts,
    /// Cites policy facts.
    pub reason: String,
    /// The principal's OWN authorization world only (scope-bounded:
    /// out-of-scope and nonexistent resources are indistinguishable).
    pub your_grants: BTreeMap<ResourceCode, Vec<String>>,
    /// Mechanically generated `postern elevate` command; `None`
    /// (serialized as `null`) for ungrantable capabilities.
    pub request_hint: Option<String>,
    /// Operator-prewritten note, relayed verbatim; ABSENT from the JSON
    /// when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator_note: Option<String>,
}

/// One step record of the pipeline walk: which step was reached and what
/// was decided there (hit/miss, predicate name plus verdict, tier choice)
/// - policy facts only, never secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceStep {
    /// The pipeline step, in the closed deny-stage vocabulary.
    pub stage: Stage,
    /// What was decided at this step, citing policy facts.
    pub detail: String,
}

/// Complete evaluation trace: stepwise `Vec` records in pipeline order
/// (deterministic - same inputs, byte-identical trace; never a hash map).
/// The trace is data handed back to the kernel; core itself logs nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct EvalTrace {
    /// Step records in pipeline order; on a short-circuit the trace ends
    /// at the deciding step.
    pub steps: Vec<TraceStep>,
}

impl EvalTrace {
    /// Stage of the last recorded step - on a short-circuit this IS the
    /// deny stage fed to the audit `stage` field and to response assembly.
    /// An empty trace has no stage (`None`).
    pub fn final_stage(&self) -> Option<Stage> {
        self.steps.last().map(|step| step.stage)
    }
}
