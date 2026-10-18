//! Multi-document projections. Text partitioning belongs to `buffer`; this
//! module owns source documents, history metadata, and editor operations.
//!
//! Adapted from matoous/helix `md/multibuffer`, commit 69bc240d.

mod operations;
mod save;
pub(crate) mod syntax;

use std::{collections::HashMap, path::PathBuf};

use buffer::{
    history::{HistoryDirection, HistoryStep},
    projection::Projection,
};
use editor_core::{ChangeSet, Rope};
use language::Syntax;

use crate::{Document, DocumentId};

pub use operations::{MultiBufferExpand, MultiBufferLocation};
pub type MultiBufferSegment = buffer::projection::Excerpt<DocumentId>;

#[derive(Debug, Clone)]
pub struct Source {
    pub display_name: String,
    pub language_name: Option<String>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) text: Rope,
}

pub struct MultiBuffer {
    pub(crate) projection: Projection<DocumentId>,
    pub(crate) sources: HashMap<DocumentId, Source>,
    pub(crate) modified: bool,
    source_mapping_valid: bool,
    pending: Option<ProjectionRevision>,
    history: HashMap<usize, (ProjectionRevision, ProjectionRevision)>,
    next_projection: Option<ProjectionRevision>,
    pub(crate) writes: HashMap<DocumentId, (usize, Rope)>,
    pub(crate) syntax: syntax::ExcerptSyntax,
}

#[derive(Clone)]
struct ProjectionRevision {
    projection: Projection<DocumentId>,
    source_mapping_valid: bool,
}

impl MultiBuffer {
    pub fn segments(&self) -> &[MultiBufferSegment] {
        &self.projection.segments
    }
    pub fn source(&self, id: DocumentId) -> &Source {
        &self.sources[&id]
    }

    pub(crate) fn new(
        projection: Projection<DocumentId>,
        sources: HashMap<DocumentId, Source>,
    ) -> Self {
        Self {
            projection,
            sources,
            modified: false,
            source_mapping_valid: true,
            pending: None,
            history: HashMap::new(),
            next_projection: None,
            writes: HashMap::new(),
            syntax: Default::default(),
        }
    }

    pub(crate) fn before_change(&mut self, changes: &ChangeSet) {
        if !changes.is_empty() && self.pending.is_none() {
            self.pending = Some(self.revision());
        }
    }

    pub(crate) fn apply_changes(&mut self, changes: &ChangeSet) {
        if let Some(projection) = self.next_projection.take() {
            self.projection = projection.projection;
            self.source_mapping_valid = projection.source_mapping_valid;
        } else {
            self.projection.apply_changes(changes);
        }
        self.syntax.invalidate();
    }

    pub(crate) fn commit(&mut self, revision: Option<usize>) {
        if let (Some(revision), Some(before)) = (revision, self.pending.take()) {
            self.history.insert(revision, (before, self.revision()));
        }
    }

    pub(crate) fn restore(&mut self, step: &HistoryStep) {
        if let Some((before, after)) = self.history.get(&step.revision) {
            self.next_projection = Some(match step.direction {
                HistoryDirection::Undo => before.clone(),
                HistoryDirection::Redo => after.clone(),
            });
        }
    }

    fn revision(&self) -> ProjectionRevision {
        ProjectionRevision {
            projection: self.projection.clone(),
            source_mapping_valid: self.source_mapping_valid,
        }
    }

    pub(crate) fn validate_changes(&self, text: &Rope, changes: &ChangeSet) -> bool {
        let mut after = text.clone();
        if !changes.apply(&mut after) {
            return false;
        }
        if let Some(next) = &self.next_projection {
            return next.projection.is_valid(&after);
        }
        let mut projection = self.projection.clone();
        projection.apply_changes(changes);
        projection.is_valid(&after)
    }

    /// Rebase historical mappings without expanding smaller, older excerpts to
    /// include insertions in context added later. A source edit crossing an old
    /// boundary has no unambiguous inverse mapping; that revision can be viewed
    /// and undone, but cannot overwrite a larger source range on save.
    pub(crate) fn rebase_source(&mut self, id: DocumentId, changes: &ChangeSet, text: &Rope) {
        use editor_core::Assoc;
        let starts: Vec<_> = self
            .segments()
            .iter()
            .filter(|s| s.source == id)
            .map(|s| s.source_range.start)
            .collect();
        let ends: Vec<_> = self
            .segments()
            .iter()
            .filter(|s| s.source == id)
            .map(|s| s.source_range.end)
            .collect();
        let update = |projection: &mut Projection<DocumentId>, valid: &mut bool| {
            for s in &mut projection.segments {
                if s.source != id {
                    continue;
                }
                let mut replace_start = false;
                let mut replace_end = false;
                for (start, end, _) in changes.changes_iter() {
                    *valid &= !(start < s.source_range.start && end > s.source_range.start
                        || start < s.source_range.end && end > s.source_range.end);
                    replace_start |= start == s.source_range.start && start < end;
                    replace_end |= end == s.source_range.end && start < end;
                }
                let start_assoc = if starts.contains(&s.source_range.start) || replace_start {
                    Assoc::Before
                } else {
                    Assoc::After
                };
                let end_assoc = if ends.contains(&s.source_range.end) || replace_end {
                    Assoc::After
                } else {
                    Assoc::Before
                };
                let start = changes.map_pos(s.source_range.start, start_assoc);
                let end = changes.map_pos(s.source_range.end, end_assoc);
                s.source_range = start.min(end)..end.max(start);
                s.source_line_start = text.char_to_line(s.source_range.start);
                s.original_text = text.slice(s.source_range.clone()).into();
            }
        };
        update(&mut self.projection, &mut self.source_mapping_valid);
        if let Some(pending) = &mut self.pending {
            update(&mut pending.projection, &mut pending.source_mapping_valid);
        }
        for (before, after) in self.history.values_mut() {
            update(&mut before.projection, &mut before.source_mapping_valid);
            update(&mut after.projection, &mut after.source_mapping_valid);
        }
        self.sources.get_mut(&id).unwrap().text = text.clone();
    }
}

impl Document {
    pub fn multibuffer(&self) -> Option<&MultiBuffer> {
        self.multibuffer.as_ref()
    }

    pub fn multibuffer_segment_index_at_char(&self, pos: usize) -> Option<usize> {
        self.multibuffer()?.projection.segment_at(pos)
    }

    pub fn multibuffer_segment_starting_at(&self, pos: usize) -> Option<&MultiBufferSegment> {
        self.multibuffer()?
            .segments()
            .iter()
            .find(|s| s.projection_range.start == pos)
    }

    pub fn multibuffer_source_line_at(&self, line: usize, pos: usize) -> Option<usize> {
        let multi = self.multibuffer()?;
        multi.segments()[multi.projection.segment_at(pos)?]
            .source_line_for_projection_line(self.text(), line)
    }

    pub fn multibuffer_source_line(&self, line: usize) -> Option<usize> {
        self.multibuffer()?
            .segments()
            .iter()
            .find_map(|s| s.source_line_for_projection_line(self.text(), line))
    }

    /// Syntax trees are prepared off-thread and published for this text version.
    pub fn multibuffer_syntax(&self) -> Option<&[Option<Syntax>]> {
        self.multibuffer()?.syntax.trees.as_deref()
    }
}
