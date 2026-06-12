//! Request model: the shell-normalized access request entering the
//! evaluation pipeline (module design 3.1/5.1). The normalized request is
//! the step [0] output - from here on the request is shell-agnostic
//! (axiom seven).

use std::fmt;
use std::net::SocketAddr;

use serde::Serialize;

use crate::domain::{Capability, PresentedCredential, ResourceCode};

/// Protocol-raw intent, boxed WITHOUT interpretation - the adapter is the
/// payload's sole interpreter (detailed design 8.0 ownership table). The
/// payload can carry business-sensitive text (SQL ...), so the type
/// follows the log red line (detailed design 7.5): hand-written `Debug`
/// always prints `REDACTED`, no `Display`, no serde, no `Clone`.
pub struct Intent {
    payload: Vec<u8>,
}

impl Intent {
    /// Boxes the raw protocol payload (shell layer, step [0]).
    pub fn new(payload: Vec<u8>) -> Self {
        Self { payload }
    }

    /// Raw payload bytes; interpreted only by adapters
    /// (`classify` / `execute`).
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

impl fmt::Debug for Intent {
    /// Always `REDACTED`, independent of the payload.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("REDACTED")
    }
}

/// Connection origin as OBSERVED by the gateway - never trusted from
/// self-reported request fields. Exactly two states, each carrying only
/// the minimal trustworthy fields; pid / exe path are deliberately
/// excluded (PID reuse and /proc TOCTOU make them forgeable). Constructed
/// only by the shell listener (contract SEC_CONSTRUCTION_SITES); this
/// crate defines the type and consumes values. Variants are written only
/// here, inside the enum body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnOrigin {
    /// SO_PEERCRED: uid/gid only, as the trust-domain gate.
    UnixPeer { uid: u32, gid: u32 },
    /// TCP peer address.
    Tcp { remote: SocketAddr },
}

/// Shell-normalization product (step [0] output).
#[derive(Debug)]
pub struct NormalizedRequest {
    /// What the Agent presented (gateway credential or local-process
    /// context). Secret family: its `Debug` is `REDACTED`.
    pub presented: PresentedCredential,
    /// Gateway-observed connection origin.
    pub origin: ConnOrigin,
    /// Target resource code.
    pub resource: ResourceCode,
    /// Protocol-raw intent (boxed, uninterpreted; `Debug` is `REDACTED`).
    pub intent: Intent,
}

/// Adapter-extracted object reference (table/column, container name, path,
/// template id ...), e.g. `"container:app-order"`. Serializes as a plain
/// JSON string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ObjectRef(String);

impl ObjectRef {
    /// Wraps an object reference string.
    pub fn new(reference: impl Into<String>) -> Self {
        Self(reference.into())
    }

    /// The reference as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Adapter semantic-normalization product (step [2] output).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassifiedIntent {
    /// The verb class the intent was whitelist-classified into.
    pub capability: Capability,
    /// Objects the intent touches.
    pub objects: Vec<ObjectRef>,
}
