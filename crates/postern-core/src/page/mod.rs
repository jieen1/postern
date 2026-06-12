//! Unified pagination — the single shape of every collection query in the workspace
//! (detailed design 5.1-⑤, contract `DB_PAGINATION_MANDATORY`).
//!
//! `PageQuery::clamp` is a pure clamping function: out-of-range input is clamped to
//! the legal bounds, never rejected, never a panic.

use serde::{Deserialize, Serialize};

/// Incoming pagination parameters. `page_no` is 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageQuery {
    pub page_no: u32,
    pub page_size: u32,
}

impl PageQuery {
    /// Default page size when the caller omits it.
    pub const DEFAULT_SIZE: u32 = 20;
    /// Global page-size ceiling: larger requests are clamped, not rejected.
    pub const MAX_SIZE: u32 = 200;

    /// Pure clamp into the legal range: `page_no < 1` → `1`;
    /// `page_size` into `[1, MAX_SIZE]`. Never errors, never panics.
    pub fn clamp(self) -> Self {
        Self {
            page_no: self.page_no.max(1),
            page_size: self.page_size.clamp(1, Self::MAX_SIZE),
        }
    }
}

/// Uniform paged-result envelope returned by every collection query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page_no: u32,
    pub page_size: u32,
    pub total: u64,
}
