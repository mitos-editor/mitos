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
) -> Result<(GitRequest, Option<NextPartialTarget>)> {
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
            source: PartialSource::Buffer(buffer_text(doc)),
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
            "Source buffer changed since this review; refresh and try again"
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
        let input = match review.comparison() {
            ReviewComparison::Staged => PartialSource::IndexSnapshot(text),
            ReviewComparison::Buffer | ReviewComparison::Unstaged => PartialSource::Snapshot {
                index: source.before().to_string(),
                text,
                buffer: review.comparison() == ReviewComparison::Buffer,
            },
            ReviewComparison::Head => PartialSource::Buffer(text),
            ReviewComparison::Historical => unreachable!(),
        };
        let mut ranges = Vec::new();
        let mut before_rows = Vec::new();
        let mut after_rows = Vec::new();
        for (start, end) in selection.line_ranges(doc.text().slice(..)) {
            for row in start..=end {
                let Some(mapped) = review.row(row) else {
                    continue;
                };
                if mapped.before.is_none() && mapped.after.is_none() {
                    continue;
                }
                if let Some(line) = mapped.before {
                    before_rows.push(line as u32..(line as u32).saturating_add(1));
                }
                if let Some(line) = mapped.after {
                    after_rows.push(line as u32..(line as u32).saturating_add(1));
                }
                let line = mapped
                    .after
                    .or_else(|| {
                        ((row + 1)..doc.text().len_lines(stdx::rope::LINE_TYPE))
                            .find_map(|row| review.row(row).and_then(|row| row.after))
                    })
                    .unwrap_or_else(|| {
                        source
                            .after()
                            .len_lines(stdx::rope::LINE_TYPE)
                            .saturating_sub(1)
                    });
                ranges.push(line as u32..(line as u32).saturating_add(1));
            }
        }
        if selected_lines {
            if staged {
                review_rows = Some((after_rows, before_rows));
            } else if kind == FileOperation::Stage && review.comparison() != ReviewComparison::Head
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
