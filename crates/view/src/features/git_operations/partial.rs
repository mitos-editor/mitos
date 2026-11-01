//! Capture partial Git edits using source coordinates, including live review buffers.

use anyhow::{ensure, Context, Result};
use editor_core::{encoding, Selection};
use stdx::rope::RopeExt;
use vcs::operations::{FileOperation, PartialSelection, PartialSource};

use super::{GitAction, GitRequest};
use crate::{DocumentId, Editor, ViewId};

/// Match the UTF-8 bytes that saving the buffer would write, including its BOM.
pub fn buffer_text(doc: &crate::Document) -> String {
    let mut text = doc.text().to_string();
    if doc.has_bom() {
        text.insert(0, '\u{feff}');
    }
    text
}

pub struct NextPartialTarget {
    document: DocumentId,
    view: ViewId,
    snapshot: buffer::Snapshot,
    selection: Selection,
}

/// Popup actions operate only at the originating cursor, even with multiple selections.
pub fn capture_cursor_hunk(
    editor: &Editor,
    kind: FileOperation,
    inspected: &crate::features::change_review::HunkReviewSnapshot,
) -> Result<(GitRequest, Option<NextPartialTarget>)> {
    ensure!(
        inspected.is_current(editor),
        "Change inspection is stale; inspect the change again before staging"
    );
    ensure!(
        !inspected.review.truncated,
        "This inspection is truncated; use whole-file staging"
    );
    let (view, doc) = crate::current_ref!(editor);
    ensure!(
        !doc.is_binary() && !doc.is_content_readonly(),
        "Choose an editable text buffer"
    );
    ensure!(
        doc.encoding() == encoding::UTF_8,
        "Partial staging requires UTF-8 text"
    );
    let path = doc
        .path()
        .context("Save this buffer to a path before staging")?
        .to_owned();
    let line = doc.cursor_line(view.id) as u32;
    let request = GitRequest::capture(
        editor,
        doc.workspace_root().to_owned(),
        GitAction::Partial {
            path,
            kind,
            source: PartialSource::Review {
                comparison: vcs::ComparisonKind::Head,
                before: inspected.source.before().to_string(),
                after: buffer_text(doc),
            },
            selection: PartialSelection::Hunks(
                std::iter::once(line..line.saturating_add(1)).collect(),
            ),
        },
    )?;
    Ok((
        request,
        Some(NextPartialTarget {
            document: doc.id(),
            view: view.id,
            snapshot: doc.snapshot(),
            selection: doc.selection(view.id).clone(),
        }),
    ))
}

impl NextPartialTarget {
    pub fn select_line(self, editor: &mut Editor, line: u32) {
        if editor.tree.focus() != self.view {
            return;
        }
        let scrolloff = editor.config().scroll.padding;
        let (view, doc) = crate::current!(editor);
        if doc.id() != self.document
            || !self.snapshot.is_current(&doc.buffer)
            || doc.selection(view.id) != &self.selection
        {
            return;
        }
        let line = doc
            .review()
            .and_then(|review| {
                (0..doc.text().len_lines(stdx::rope::LINE_TYPE)).find(|row| {
                    review
                        .row(*row)
                        .is_some_and(|row| row.after == Some(line as usize))
                })
            })
            .unwrap_or(line as usize)
            .min(
                doc.text()
                    .len_lines(stdx::rope::LINE_TYPE)
                    .saturating_sub(1),
            );
        let point = doc.text().line_to_char_idx(line);
        doc.set_selection(view.id, Selection::point(point));
        view.ensure_cursor_in_view(doc, scrolloff);
    }
}

/// `disk` selects the on-disk source explicitly; ordinary stage commands include unsaved text.
pub fn capture(
    editor: &Editor,
    kind: FileOperation,
    selected_lines: bool,
    disk: bool,
) -> Result<(GitRequest, Option<NextPartialTarget>)> {
    let (view, doc) = crate::current_ref!(editor);
    ensure!(!doc.is_binary(), "Partial staging requires a text buffer");
    ensure!(
        doc.encoding() == encoding::UTF_8,
        "Partial staging requires UTF-8 text; use whole-file staging for other encodings"
    );
    let selection = doc.selection(view.id);
    let mut review_rows = None;
    let (path, workspace, source, ranges) = if let Some(review) = doc.review() {
        use crate::features::change_review::ReviewComparison;
        ensure!(
            review.is_live() && !review.is_loading(),
            "Wait for a live diff review before staging"
        );
        ensure!(
            review.source_is_current(editor),
            "Comparison changed since this review; refresh and try again"
        );
        ensure!(
            selected_lines || !review.review().truncated,
            "This review is truncated; select visible lines or use whole-file staging"
        );
        ensure!(!disk, "Disk staging commands are for editable file buffers");
        let source = review.source().context("Review source is not available")?;
        let staged = review.comparison() == ReviewComparison::Staged;
        ensure!(
            kind != FileOperation::Stage || !staged,
            "Use unstage commands in a staged review"
        );
        let mut text = source.after().to_string();
        if matches!(
            review.comparison(),
            ReviewComparison::Head | ReviewComparison::Buffer
        ) && review
            .source_document()
            .and_then(|id| editor.document(id))
            .is_some_and(|doc| doc.has_bom())
        {
            text.insert(0, '\u{feff}');
        }
        let has_buffer = review.source_document().is_some();
        let comparison = match review.comparison() {
            ReviewComparison::Staged => vcs::ComparisonKind::Staged,
            ReviewComparison::Buffer if has_buffer => vcs::ComparisonKind::Index,
            ReviewComparison::Buffer | ReviewComparison::Unstaged => vcs::ComparisonKind::Unstaged,
            ReviewComparison::Head if has_buffer => vcs::ComparisonKind::Head,
            ReviewComparison::Head | ReviewComparison::HeadToWorktree => {
                vcs::ComparisonKind::HeadToWorktree
            }
            ReviewComparison::Historical => unreachable!(),
        };
        let input = PartialSource::Review {
            comparison,
            before: source.before().to_string(),
            after: text,
        };
        // Resolve deleted-row anchors once, rather than scanning the suffix for
        // every selected removal. Consecutive source lines remain ranges.
        let row_count = doc.text().len_lines(stdx::rope::LINE_TYPE);
        let mut next_after = vec![0; row_count];
        let mut next = source
            .after()
            .len_lines(stdx::rope::LINE_TYPE)
            .saturating_sub(1);
        for row in (0..row_count).rev() {
            if let Some(line) = review.row(row).and_then(|row| row.after) {
                next = line;
            }
            next_after[row] = next;
        }

        let mut ranges = Vec::new();
        let mut before_rows = Vec::new();
        let mut after_rows = Vec::new();
        for (start, end) in selection.line_ranges(doc.text().slice(..)) {
            for (row, &anchor) in next_after.iter().enumerate().take(end + 1).skip(start) {
                let Some(mapped) = review.row(row) else {
                    continue;
                };
                if mapped.before.is_none() && mapped.after.is_none() {
                    continue;
                }
                if let Some(line) = mapped.before {
                    push_line(&mut before_rows, line as u32);
                }
                if let Some(line) = mapped.after {
                    push_line(&mut after_rows, line as u32);
                }
                push_line(&mut ranges, anchor as u32);
            }
        }
        if selected_lines {
            if staged {
                review_rows = Some((after_rows, before_rows));
            } else if kind == FileOperation::Stage
                && !matches!(
                    review.comparison(),
                    ReviewComparison::Head | ReviewComparison::HeadToWorktree
                )
            {
                review_rows = Some((before_rows, after_rows));
            } else if kind == FileOperation::Stage && after_rows.is_empty() {
                anyhow::bail!("Select removed lines in :diff buffer or :diff unstaged to stage their deletion");
            }
        }
        (
            review.path().to_owned(),
            review.workspace().to_owned(),
            input,
            ranges,
        )
    } else {
        ensure!(
            !doc.is_content_readonly(),
            "Historical versions cannot change today's index"
        );
        ensure!(
            !disk || !doc.is_modified(),
            "Save this buffer before selecting changes on disk"
        );
        let path = doc
            .path()
            .context("Save this buffer to a path before staging")?
            .to_owned();
        let ranges = selection
            .line_ranges(doc.text().slice(..))
            .map(|(start, end)| start as u32..(end as u32).saturating_add(1))
            .collect();
        let source = if disk {
            PartialSource::Worktree
        } else {
            PartialSource::Buffer(buffer_text(doc))
        };
        (path, doc.workspace_root().to_owned(), source, ranges)
    };
    ensure!(
        !ranges.is_empty(),
        "Select a changed source line in this review"
    );
    let selection = if let Some((before, after)) = review_rows {
        PartialSelection::ReviewLines { before, after }
    } else if selected_lines {
        PartialSelection::Lines(ranges)
    } else {
        PartialSelection::Hunks(ranges)
    };
    let source_document = doc
        .review()
        .map_or(Some(doc.id()), |review| review.source_document());
    let request = GitRequest::capture_with_document(
        editor,
        workspace,
        GitAction::Partial {
            path,
            kind,
            source,
            selection,
        },
        source_document,
    )?;
    let next = Some(NextPartialTarget {
        document: doc.id(),
        view: view.id,
        snapshot: doc.snapshot(),
        selection: doc.selection(view.id).clone(),
    });
    Ok((request, next))
}

fn push_line(ranges: &mut Vec<std::ops::Range<u32>>, line: u32) {
    if let Some(previous) = ranges.last_mut().filter(|range| range.end == line) {
        previous.end = line.saturating_add(1);
    } else if !ranges.last().is_some_and(|range| range.contains(&line)) {
        ranges.push(line..line.saturating_add(1));
    }
}
