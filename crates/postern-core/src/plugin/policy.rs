//! Policy-read-view plugin trait (detailed design 4.1; module design 5.2).
//! Core only DECLARES the shape; the store implements it (snapshot build is
//! one transaction with atomic `Arc` replacement - hot-effective, detailed
//! design 6.2).

use std::sync::Arc;

use crate::domain::PolicySnapshot;

/// Policy-read view: the "single authoritative policy state" the evaluator
/// faces. Returns the current immutable snapshot; the store swaps the `Arc`
/// as a whole on every rebuild, so a reader either sees the old snapshot or
/// the new one in full, never a torn intermediate (detailed design 6.2).
pub trait PolicyView: Send + Sync {
    /// The current immutable policy snapshot.
    fn snapshot(&self) -> Arc<PolicySnapshot>;
}
