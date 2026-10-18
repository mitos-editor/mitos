use std::{collections::HashMap, path::PathBuf};

use anyhow::{bail, ensure, Context};
use buffer::projection::{excerpt_lines, merge_line_ranges, Excerpt, Projection};
use editor_core::{diff::compare_ropes, Assoc, Rope, Selection, Tendril, Transaction};

use super::{MultiBuffer, Source};
use crate::{editor::Action, Align, Document, DocumentId, Editor, ViewId};

pub const CONTEXT_LINES: usize = 3;

/// An owned location, independent of picker and frontend lifetimes.
#[derive(Debug, Clone)]
pub enum MultiBufferLocation {
    Document(DocumentId, Option<(usize, usize)>),
    Path(PathBuf, Option<(usize, usize)>),
}

#[derive(Debug, Clone, Copy)]
pub enum MultiBufferExpand {
    Up,
    Down,
    Both,
}

impl Editor {
    fn multibuffer_in_view(&self, id: DocumentId, view: ViewId) -> anyhow::Result<&Document> {
        let doc = self.document(id).context("Multi-buffer is closed")?;
        ensure!(
            self.tree
                .try_get(view)
                .is_some_and(|v| v.document_id() == id),
            "View no longer displays this multi-buffer"
        );
        ensure!(doc.multibuffer().is_some(), "Not a multi-buffer");
        Ok(doc)
    }

    /// Group locations by source, merge touching context, and open an editable projection.
    /// Sources use stable document identities, so closing/reopening a path cannot
    /// silently redirect a pending edit to another document.
    pub fn open_multibuffer(
        &mut self,
        locations: impl IntoIterator<Item = MultiBufferLocation>,
        action: Action,
    ) -> anyhow::Result<DocumentId> {
        let mut sources = HashMap::new();
        let mut groups: Vec<(DocumentId, Vec<std::ops::Range<usize>>)> = Vec::new();
        let mut indices = HashMap::new();
        for location in locations {
            let (id, lines) = match location {
                MultiBufferLocation::Document(id, lines) => (id, lines),
                MultiBufferLocation::Path(path, lines) => (self.open(&path, Action::Load)?, lines),
            };
            let doc = self.document(id).context("Multi-buffer source is closed")?;
            ensure!(
                !doc.is_binary() && doc.multibuffer().is_none(),
                "Only ordinary text documents can be used as excerpts"
            );
            let Some(range) = excerpt_lines(doc.text(), lines, CONTEXT_LINES) else {
                continue;
            };
            let index = *indices.entry(id).or_insert_with(|| {
                let index = groups.len();
                groups.push((id, Vec::new()));
                sources.insert(
                    id,
                    Source {
                        display_name: doc.display_name().into_owned(),
                        language_name: doc.language_name().map(str::to_owned),
                        path: doc.path().map(ToOwned::to_owned),
                        text: doc.text().clone(),
                    },
                );
                index
            });
            groups[index].1.push(range);
        }
        let mut text = Rope::new();
        let mut segments = Vec::new();
        for (source, mut ranges) in groups {
            merge_line_ranges(&mut ranges);
            let original = &sources[&source].text;
            for lines in ranges {
                let source_range =
                    original.line_to_char(lines.start)..original.line_to_char(lines.end);
                let original_text = original.slice(source_range.clone()).into();
                let start = text.len_chars();
                text.append(original.slice(source_range.clone()).into());
                segments.push(Excerpt {
                    source,
                    projection_range: start..text.len_chars(),
                    source_range,
                    source_line_start: lines.start,
                    original_text,
                });
            }
        }
        ensure!(!segments.is_empty(), "No file locations to open");
        let projection = Projection { segments };
        ensure!(
            projection.is_valid(&text),
            "These excerpts join parts of one character; open them in separate buffers"
        );
        let mut doc = Document::from(text, None, self.config.clone(), self.syn_loader.clone());
        doc.multibuffer = Some(MultiBuffer::new(projection, sources));
        let id = self.new_file_from_document(action, doc);
        Ok(id)
    }

    pub fn extend_multibuffer_excerpt(
        &mut self,
        id: DocumentId,
        view: ViewId,
    ) -> anyhow::Result<()> {
        self.expand_multibuffer_excerpt(id, view, MultiBufferExpand::Both)
    }

    pub fn expand_multibuffer_excerpt(
        &mut self,
        id: DocumentId,
        view: ViewId,
        direction: MultiBufferExpand,
    ) -> anyhow::Result<()> {
        let doc = self.multibuffer_in_view(id, view)?;
        let multi = doc.multibuffer().unwrap();
        ensure!(multi.source_mapping_valid, "Source edits crossed an older excerpt boundary; redo the expansion or reopen the multi-buffer");
        ensure!(
            multi.writes.is_empty(),
            "Wait for multi-buffer writes to complete before expanding excerpts"
        );
        let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
        let index = multi
            .projection
            .segment_at(cursor)
            .context("Cursor is outside an excerpt")?;
        let source_id = multi.segments()[index].source;
        let source = self
            .document(source_id)
            .context("Multi-buffer source is closed")?;
        let source_text = source.text().clone();
        let source_changes = compare_ropes(&multi.source(source_id).text, &source_text);
        let mut projection = multi.projection.clone();
        super::save::map_source(
            &mut projection,
            source_id,
            source_changes.changes(),
            &source_text,
            false,
        )?;
        let segment = &projection.segments[index];
        let start_line = source_text.char_to_line(segment.source_range.start);
        // A non-newline-terminated excerpt includes its final partial line.
        let end_line = if segment.source_range.end == 0 {
            0
        } else {
            source_text.char_to_line(segment.source_range.end - 1) + 1
        };
        let start = if matches!(direction, MultiBufferExpand::Up | MultiBufferExpand::Both) {
            source_text.line_to_char(start_line.saturating_sub(CONTEXT_LINES))
        } else {
            segment.source_range.start
        };
        let end = if matches!(direction, MultiBufferExpand::Down | MultiBufferExpand::Both) {
            source_text.line_to_char(
                end_line
                    .saturating_add(CONTEXT_LINES)
                    .min(source_text.len_lines()),
            )
        } else {
            segment.source_range.end
        };
        ensure!(
            start != segment.source_range.start || end != segment.source_range.end,
            "Excerpt cannot be expanded further"
        );
        let (mut first, mut last) = (index, index);
        let (mut start, mut end) = (start, end);
        while first > 0 {
            let prev = &projection.segments[first - 1];
            if prev.source != source_id || prev.source_range.end < start {
                break;
            }
            first -= 1;
            start = start.min(prev.source_range.start);
            end = end.max(prev.source_range.end);
        }
        while last + 1 < projection.segments.len() {
            let next = &projection.segments[last + 1];
            if next.source != source_id || next.source_range.start > end {
                break;
            }
            last += 1;
            start = start.min(next.source_range.start);
            end = end.max(next.source_range.end);
        }
        let original_text: Rope = source_text.slice(start..end).into();
        let mut replacement = original_text.clone();
        let mut edits = Vec::new();
        for s in &projection.segments[first..=last] {
            let current: Rope = doc.text().slice(s.projection_range.clone()).into();
            let before: Rope = source_text.slice(s.source_range.clone()).into();
            for (a, b, insert) in compare_ropes(&before, &current).changes_iter() {
                edits.push((
                    s.source_range.start - start + a,
                    s.source_range.start - start + b,
                    insert,
                ));
            }
        }
        let changes = Transaction::change(&replacement, edits.into_iter());
        ensure!(
            changes.changes().apply(&mut replacement),
            "Invalid excerpt edit"
        );
        let range = projection.segments[first].projection_range.start
            ..projection.segments[last].projection_range.end;
        let mut new_text = doc.text().clone();
        let replacement_tx = Transaction::change(
            &new_text,
            [(
                range.start,
                range.end,
                Some(Tendril::from(replacement.to_string())),
            )]
            .into_iter(),
        );
        replacement_tx.changes().apply(&mut new_text);
        let tx = compare_ropes(doc.text(), &new_text);
        let new_end = range.start + replacement.len_chars();
        let delta = new_end as isize - range.end as isize;
        for s in &mut projection.segments[last + 1..] {
            s.projection_range = s.projection_range.start.saturating_add_signed(delta)
                ..s.projection_range.end.saturating_add_signed(delta);
        }
        projection.segments.splice(
            first..=last,
            [Excerpt {
                source: source_id,
                source_range: start..end,
                source_line_start: source_text.char_to_line(start),
                projection_range: range.start..new_end,
                original_text,
            }],
        );
        ensure!(
            projection.is_valid(&new_text),
            "Invalid expanded projection"
        );
        let doc = self.document_mut(id).unwrap();
        doc.commit_changes();
        let multi = doc.multibuffer.as_mut().unwrap();
        multi.rebase_source(source_id, source_changes.changes(), &source_text);
        multi.next_projection = Some(super::ProjectionRevision {
            projection,
            source_mapping_valid: true,
        });
        // Expansion preserves every split's selection through the text diff.
        doc.apply(tx.changes())?;
        doc.commit_changes();
        self.sync_document_views(id);
        Ok(())
    }

    pub fn goto_next_multibuffer_excerpt(
        &mut self,
        id: DocumentId,
        view: ViewId,
    ) -> anyhow::Result<()> {
        self.multibuffer_in_view(id, view)?;
        let doc = self.document_mut(id).unwrap();
        let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
        let pos = doc
            .multibuffer()
            .context("Not a multi-buffer")?
            .segments()
            .iter()
            .map(|s| s.projection_range.start)
            .find(|&start| start > cursor)
            .context("No next excerpt")?;
        doc.set_selection(view, Selection::point(pos));
        Ok(())
    }

    pub fn goto_previous_multibuffer_excerpt_end(
        &mut self,
        id: DocumentId,
        view: ViewId,
    ) -> anyhow::Result<()> {
        self.multibuffer_in_view(id, view)?;
        let doc = self.document_mut(id).unwrap();
        let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
        let pos = doc
            .multibuffer()
            .context("Not a multi-buffer")?
            .segments()
            .iter()
            .rev()
            .map(|s| s.projection_range.end)
            .find(|&end| end <= cursor && end > 0)
            .context("No previous excerpt")?;
        let pos = editor_core::graphemes::prev_grapheme_boundary(doc.text().slice(..), pos);
        doc.set_selection(view, Selection::point(pos));
        Ok(())
    }

    pub fn open_multibuffer_excerpt(
        &mut self,
        id: DocumentId,
        view: ViewId,
        action: Action,
    ) -> anyhow::Result<()> {
        if matches!(action, Action::Load) {
            bail!("Opening an excerpt requires a view");
        }
        let doc = self.multibuffer_in_view(id, view)?;
        let multi = doc.multibuffer().unwrap();
        let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
        let index = multi
            .projection
            .segment_at(cursor)
            .context("Cursor is outside an excerpt")?;
        let segment = &multi.segments()[index];
        let source = self
            .document(segment.source)
            .context("Multi-buffer source is closed")?;
        let changes = compare_ropes(&multi.source(segment.source).text, source.text());
        let start = changes
            .changes()
            .map_pos(segment.source_range.start, Assoc::Before);
        let end = changes
            .changes()
            .map_pos(segment.source_range.end, Assoc::After);
        let source_id = segment.source;
        self.switch(source_id, action);
        let (view, doc) = current!(self);
        doc.set_selection(view.id, Selection::single(start, end));
        crate::align_view(doc, view, Align::Center);
        Ok(())
    }
}
