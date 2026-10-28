//! Capture partial Git edits using source coordinates, including live review buffers.

use anyhow::{ensure, Context, Result};
use editor_core::{encoding, Selection};
use stdx::rope::RopeExt;
use vcs::operations::{FileOperation, PartialSelection, PartialSource};

use super::{GitAction, GitRequest};
use crate::{DocumentId, Editor, ViewId};
use std::path::PathBuf;

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
    project_path: Option<PathBuf>,
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
            project_path: None,
        }),
    ))
}

impl NextPartialTarget {
    pub fn select_line(self, editor: &mut Editor, line: u32) {
        self.select_after(editor, Some(line));
    }

    /// Aggregate reviews resolve the next file after their asynchronous refresh.
    pub fn select_after(self, editor: &mut Editor, line: Option<u32>) {
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
        if let Some(path) = self.project_path {
            editor.advance_project_review_after_refresh(
                self.document,
                crate::features::project_review::ProjectAdvance {
                    view: self.view,
                    snapshot: self.snapshot,
                    selection: self.selection,
                    path,
                    line,
                },
            );
            return;
        }
        let Some(line) = line else {
            return;
        };
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
    if doc.project_review().is_some() {
        return capture_project(editor, kind, selected_lines, disk);
    }
    ensure!(!doc.is_binary(), "Partial staging requires a text buffer");
    ensure!(
        doc.encoding() == encoding::UTF_8,
        "Partial staging requires UTF-8 text; use whole-file staging for other encodings"
    );
    let selection = doc.selection(view.id);
    let mut review_rows = None;
    let mut review_guard = None;
    let (path, workspace, source, ranges) = if let Some(review) = doc.review() {
        use crate::features::change_review::ReviewComparison;
        ensure!(
            review.is_live() && !review.is_loading(),
            "Wait for a live diff review before staging"
        );
        ensure!(
            review.source_is_current(editor),
            "Source or Git state changed since this review; refresh and try again"
        );
        review_guard = Some(
            review
                .action_guard()
                .context("Review inputs are not available; refresh and try again")?,
        );
        ensure!(!disk, "Disk staging commands are for editable file buffers");
        let source = review.source().context("Review source is not available")?;
        let source_buffer = review.source_document().and_then(|id| editor.document(id));
        ensure!(
            source_buffer.is_none_or(|doc| !doc.is_binary() && doc.encoding() == encoding::UTF_8),
            "Partial staging requires a UTF-8 source buffer"
        );
        let staged = review.comparison() == ReviewComparison::Staged;
        ensure!(
            kind != FileOperation::Stage || !staged,
            "Use unstage commands in a staged review"
        );
        let use_buffer = matches!(
            review.comparison(),
            ReviewComparison::Head | ReviewComparison::Buffer
        ) && source_buffer.is_some();
        let text = if use_buffer {
            buffer_text(source_buffer.unwrap())
        } else {
            source.after().to_string()
        };
        let input = match review.comparison() {
            ReviewComparison::Staged => PartialSource::IndexSnapshot(text),
            ReviewComparison::Buffer | ReviewComparison::Unstaged => PartialSource::Snapshot {
                index: source.before().to_string(),
                text,
                buffer: use_buffer,
            },
            ReviewComparison::Head if use_buffer => PartialSource::Buffer(text),
            ReviewComparison::Head | ReviewComparison::HeadToWorktree => PartialSource::Worktree,
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
    let request = if let Some(guard) = review_guard {
        request.with_review_guard(guard)
    } else {
        request
    };
    let next = Some(NextPartialTarget {
        document: doc.id(),
        view: view.id,
        snapshot: doc.snapshot(),
        selection: doc.selection(view.id).clone(),
        project_path: None,
    });
    Ok((request, next))
}

fn capture_project(
    editor: &Editor,
    kind: FileOperation,
    selected_lines: bool,
    disk: bool,
) -> Result<(GitRequest, Option<NextPartialTarget>)> {
    use crate::features::change_review::{ReviewComparison, ReviewLineKind};
    let (view, doc) = crate::current_ref!(editor);
    let project = doc.project_review().unwrap();
    ensure!(
        !project.is_loading(),
        "Wait for the project review to finish loading before staging"
    );
    ensure!(!disk, "Disk staging commands are for editable file buffers");
    let mut selected_section: Option<&crate::features::project_review::ProjectSection> = None;
    let mut rows = Vec::new();
    for (start, end) in doc.selection(view.id).line_ranges(doc.text().slice(..)) {
        for row in start..=end {
            let section = project
                .section(row)
                .context("Select source changes within one file in the project review")?;
            ensure!(
                selected_section
                    .is_none_or(|selected| selected.status.path() == section.status.path()),
                "Partial staging selections cannot span multiple files"
            );
            selected_section = Some(section);
            rows.push(row);
        }
    }
    let section = selected_section.context("Select a changed source line in this review")?;
    ensure!(
        section.is_current(editor),
        "Source buffer changed since this project review; refresh and try again"
    );
    let snapshot = section
        .snapshot
        .as_ref()
        .context("This file could not be reviewed; open its source before staging")?;
    ensure!(
        snapshot.is_current(editor),
        "Git state or source changed since this review; refresh and try again"
    );
    let review = snapshot.diff()?;
    ensure!(
        !review.truncated,
        "This file review was shortened; open the file before staging"
    );
    let source_document = section.source_document();
    let buffer = source_document.and_then(|id| editor.document(id));
    ensure!(
        buffer.is_none_or(|doc| !doc.is_binary() && doc.encoding() == encoding::UTF_8),
        "Partial staging requires a UTF-8 source buffer"
    );
    let input = match section.comparison {
        ReviewComparison::Staged => {
            ensure!(
                kind == FileOperation::Unstage,
                "Use unstage commands in a staged project review"
            );
            PartialSource::IndexSnapshot(snapshot.source.after().to_string())
        }
        ReviewComparison::Buffer | ReviewComparison::Unstaged => {
            let use_buffer = section.comparison == ReviewComparison::Buffer && buffer.is_some();
            PartialSource::Snapshot {
                index: snapshot.source.before().to_string(),
                text: if use_buffer {
                    buffer_text(buffer.unwrap())
                } else {
                    snapshot.source.after().to_string()
                },
                buffer: use_buffer,
            }
        }
        _ => {
            anyhow::bail!("Partial staging requires a live staged, unstaged, or buffer comparison")
        }
    };
    let end = section.start_line + 1 + review.lines.len();
    let mut ranges = Vec::new();
    let mut before = Vec::new();
    let mut after = Vec::new();
    for row in rows {
        let row = if !selected_lines
            && section
                .row(row)
                .is_none_or(|mapped| mapped.kind == ReviewLineKind::Header)
        {
            // Change motions land on headers. Stage their first actual change,
            // rather than the leading context or all changes in that file.
            ((row + 1)..end)
                .find(|row| {
                    section.row(*row).is_some_and(|mapped| {
                        matches!(mapped.kind, ReviewLineKind::Added | ReviewLineKind::Removed)
                    })
                })
                .unwrap_or(row)
        } else {
            row
        };
        let Some(mapped) = section.row(row) else {
            continue;
        };
        if mapped.before.is_none() && mapped.after.is_none() {
            continue;
        }
        if let Some(line) = mapped.before {
            before.push(line as u32..(line as u32).saturating_add(1));
        }
        if let Some(line) = mapped.after {
            after.push(line as u32..(line as u32).saturating_add(1));
        }
        let line = mapped
            .after
            .or_else(|| ((row + 1)..end).find_map(|row| section.row(row).and_then(|row| row.after)))
            .unwrap_or_else(|| {
                snapshot
                    .source
                    .after()
                    .len_lines(stdx::rope::LINE_TYPE)
                    .saturating_sub(1)
            });
        ranges.push(line as u32..(line as u32).saturating_add(1));
    }
    ensure!(
        !ranges.is_empty(),
        "Select a changed source line in this review"
    );
    let selection = if selected_lines && section.comparison == ReviewComparison::Staged {
        PartialSelection::ReviewLines {
            before: after,
            after: before,
        }
    } else if selected_lines && kind == FileOperation::Stage {
        PartialSelection::ReviewLines { before, after }
    } else if selected_lines {
        PartialSelection::Lines(ranges)
    } else {
        PartialSelection::Hunks(ranges)
    };
    let path = section.status.path().to_owned();
    let request = GitRequest::capture_with_document(
        editor,
        project.workspace.clone(),
        GitAction::Partial {
            path: path.clone(),
            kind,
            source: input,
            selection,
        },
        source_document,
    )?
    .with_review_guard(snapshot.action_guard());
    Ok((
        request,
        Some(NextPartialTarget {
            document: doc.id(),
            view: view.id,
            snapshot: doc.snapshot(),
            selection: doc.selection(view.id).clone(),
            project_path: Some(path),
        }),
    ))
}
