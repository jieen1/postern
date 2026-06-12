//! Audit-write plugin trait and its event carrier (detailed design 4.1,
//! 5.3 audit-event stream; module design 5.2, 8.9). Core only DECLARES the
//! shapes; the store implements the sink (`JsonlAuditSink`, append-only
//! day-rotated JSONL).
//!
//! `AuditEvent` here is the minimal carrier core needs to type
//! `AuditSink::record`. The full envelope schema, kind taxonomy and
//! versioning are the observability plane's (detailed design 8.9); audit
//! content never holds credential values or real addresses (it passes the
//! same `Sanitizer` before write).

use crate::domain::{Capability, PrincipalId, ResourceCode};
use crate::error::{AuditError, Stage};
use crate::request::{ConnOrigin, ObjectRef};

/// One audit event handed to the sink for append (detailed design 5.3).
/// Minimal load-bearing shape: the request-event facts the kernel always
/// has. All ids serialize as snowflake-id strings; `principal` / `resource`
/// are kept as readable name fields alongside.
pub struct AuditEvent {
    /// Schema version of the envelope (`v` in the JSONL line).
    pub v: u32,
    /// Event-kind discriminant (`request` / `policy_change` / ...); a stable
    /// string so the carrier stays open to the full kind taxonomy.
    pub kind: String,
    /// Shell entry that produced the request (`mcp` / `http`).
    pub entry: String,
    /// Gateway-observed connection origin (a gateway-side observable fact;
    /// not returned to the Agent).
    pub origin: ConnOrigin,
    /// Authenticated principal, if any (`None` before/at a step [1] deny).
    pub principal: Option<PrincipalId>,
    /// Target resource code (always a code, never a real address).
    pub resource: ResourceCode,
    /// Classified verb, if classification reached (`None` on a classify
    /// deny that produced no capability).
    pub capability: Option<Capability>,
    /// Objects the intent touched (anonymized denial facts on a deny).
    pub objects: Vec<ObjectRef>,
    /// Decision word as recorded (`allow` / `deny` / `escalate_denied`).
    pub decision: String,
    /// Deny stage attribution; `None` on an allow.
    pub stage: Option<Stage>,
    /// Reason text citing policy facts; empty on an allow.
    pub reason: String,
    /// Policy revision at decision time - the reconciliation anchor.
    pub policy_rev: u64,
}

/// Audit-write plugin (isolation point: append-only self-observation /
/// tamper-evident implementation once scaled up). Implementation:
/// `JsonlAuditSink` (store, detailed design 8.11 / 8.9).
pub trait AuditSink: Send + Sync {
    /// Appends one audit event. For read-only verbs a failed write denies
    /// (not recordable means not grantable) - the caller maps `Err` to a
    /// `Stage::Audit` deny.
    fn record(&self, event: AuditEvent) -> Result<(), AuditError>;
}
