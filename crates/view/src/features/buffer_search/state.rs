use std::sync::Arc;

use editor_core::{
    search::statistics::{Position, Statistics},
    Range,
};
use event::TaskController;

use super::Query;

/// This state lives in DocumentViewState. Removing the relationship cancels its
/// request; presentation copies retain values without copying live services.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(super) query: Option<Query>,
    pub(super) snapshot: Option<buffer::Snapshot>,
    pub(super) limit: Option<usize>,
    pub(super) statistics: Option<Arc<Statistics>>,
    pub(super) request: TaskController,
    pub(super) target: Option<Range>,
}

impl State {
    pub(crate) fn is_current(&self, buffer: &buffer::Buffer) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.is_current(buffer))
    }

    pub(crate) fn presentation(&self) -> Self {
        Self {
            query: self.query.clone(),
            snapshot: self.snapshot.clone(),
            limit: self.limit,
            statistics: self.statistics.clone(),
            target: self.target,
            ..Self::default()
        }
    }

    pub(crate) fn clear_position(&mut self) {
        self.target = None;
    }

    pub(crate) fn leave_document(&mut self) {
        self.clear_position();
        self.request.cancel();
    }

    pub(crate) fn position(&self) -> Option<Position> {
        let target = self.target?;
        Some(
            self.statistics
                .as_ref()
                .map_or(Position::PENDING, |stats| stats.position(target)),
        )
    }
}
