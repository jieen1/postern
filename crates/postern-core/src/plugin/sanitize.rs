//! Response-sanitization plugin traits and their carriers (detailed design
//! 4.1, 6.4 streaming-scrub model; module design 5.2, step [9]). Core only
//! DECLARES the shapes; `daemon::sanitize` implements them, applying the
//! secrets-plane-issued `ScrubSet` opaque handle plus declarative
//! `MaskRule`s.

use crate::plugin::channel::RawResponse;

/// One declarative mask rule (detailed design 7.2; sourced from
/// `grant_constraints.kind='mask_fields'`): erase or mask a named
/// column/field. The spec payload is raw JSON text interpreted by the
/// sanitizer.
pub struct MaskRule {
    /// Field / column name the rule targets.
    pub field: String,
    /// Raw JSON spec payload (erase vs mask, mask pattern, ...).
    pub spec: String,
}

/// Sanitized response ready for kernel egress (detailed design step [9]
/// output). Whatever real addresses / credential values / declared fields
/// the raw payload held have been scrubbed; this is what leaves the kernel.
pub struct SanitizedResponse {
    /// Scrubbed response bytes, safe to return to the Agent.
    pub payload: Vec<u8>,
}

/// Streaming sanitizer for large outputs (detailed design 6.4): scrubs
/// chunk by chunk, retaining the previous chunk's trailing N bytes to
/// participate in the next chunk's match, eliminating boundary-split
/// escapes. Bounded buffering and backpressure constrain large `observe`
/// streams.
pub trait StreamScrubber: Send {
    /// Scrubs one chunk, returning the bytes safe to emit now. The
    /// implementation may withhold trailing bytes that could span the next
    /// chunk boundary.
    fn push(&mut self, chunk: &[u8]) -> Vec<u8>;

    /// Flushes any withheld tail at end-of-stream, returning the final
    /// scrubbed bytes.
    fn finish(&mut self) -> Vec<u8>;
}

/// Response sanitizer (step [9]). Small responses are scrubbed whole;
/// large streaming output goes through a sliding overlap window (detailed
/// design 6.4). Implementation: `daemon::sanitize`.
pub trait Sanitizer: Send + Sync {
    /// Scrubs a full payload in one pass (small responses, error strings,
    /// deny responses).
    fn scrub(&self, payload: RawResponse, declared: &[MaskRule]) -> SanitizedResponse;

    /// Opens a streaming scrub: processes chunk by chunk, retaining the
    /// previous chunk's trailing N bytes so boundary-split secrets cannot
    /// escape.
    fn scrub_stream(&self, declared: &[MaskRule]) -> Box<dyn StreamScrubber>;
}
