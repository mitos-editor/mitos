//! Review annotations supplied by optional comparison engines.

use std::{fmt, ops::Range};

use editor_core::Rope;

/// Sorted, non-overlapping UTF-8 byte ranges in the original snapshots.
/// Empty ranges are omitted. Text outside these ranges matched structurally.
#[derive(Debug, Default)]
pub struct ReviewChanges {
    pub before: Vec<Range<usize>>,
    pub after: Vec<Range<usize>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewUnavailable(pub &'static str);

impl fmt::Display for ReviewUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ReviewUnavailable {}

/// A review provider annotates immutable snapshots; it cannot change edit hunks.
/// Implementations must bound work and check cancellation during expensive operations.
pub trait ReviewProvider {
    fn name(&self) -> &'static str;

    fn compare(
        &self,
        before: &Rope,
        after: &Rope,
        canceled: &dyn Fn() -> bool,
    ) -> Result<ReviewChanges, ReviewUnavailable>;
}
