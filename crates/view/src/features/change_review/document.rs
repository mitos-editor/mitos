//! Editor-owned review documents. Immutable comparison data is projected into
//! ordinary text so selections, search, layout, and split navigation stay native.

use stdx::rope::RopeExt as _;

use std::{ops::Range, path::Path, sync::Arc};

use anyhow::{ensure, Result};
use editor_core::{movement::Direction, Rope, Selection, Transaction};
use event::{TaskController, TaskHandle};
use text_diff::ReviewLine;

use super::{
    run_review, DiffEngine, Review, ReviewLineKind, ReviewRequest, ReviewSnapshot, ReviewSource,
};
use crate::{editor::Action, Document, DocumentId, Editor};

pub struct ReviewDocument {
    request: ReviewRequest,
    review: Arc<Review>,
    source: Option<Arc<ReviewSource>>,
    task: TaskController,
    loading: bool,
    initialized: bool,
    instance: usize,
    changes: RowChanges,
}

impl ReviewDocument {
    pub fn instance(&self) -> usize {
        self.instance
    }
    pub fn path(&self) -> &Path {
        self.request.status.path()
    }
    pub fn workspace(&self) -> &Path {
        &self.request.workspace
    }
    pub fn comparison(&self) -> super::ReviewComparison {
        self.request.comparison
    }
    pub fn engine(&self) -> DiffEngine {
        self.request.engine()
    }
    pub fn review(&self) -> &Review {
        &self.review
    }
    pub fn is_loading(&self) -> bool {
        self.loading
    }
    pub fn source(&self) -> Option<&Arc<ReviewSource>> {
        self.source.as_ref()
    }
    pub fn row_offset(&self) -> usize {
        usize::from(self.review.fallback.is_some())
    }
    pub fn row(&self, document_line: usize) -> Option<&ReviewLine> {
        self.review
            .lines
            .get(document_line.checked_sub(self.row_offset())?)
    }
    /// Changed ranges in projected-row Unicode scalar coordinates, prepared by
    /// the worker so rendering never scans source prefixes to translate bytes.
    pub fn changes(&self, document_line: usize) -> Option<&[Range<usize>]> {
        self.changes
            .get(document_line.checked_sub(self.row_offset())?)?
            .as_deref()
    }
}

impl Document {
    /// Derived content can be selected and searched but cannot be edited or saved.
    /// This is independent of filesystem write permissions and cannot be forced.
    pub fn is_content_readonly(&self) -> bool {
        self.review.is_some()
    }
    pub fn review(&self) -> Option<&ReviewDocument> {
        self.review.as_deref()
    }
}

enum Work {
    Read {
        request: Arc<ReviewRequest>,
        previous: Option<Arc<ReviewSnapshot>>,
    },
    Engine {
        source: Arc<ReviewSource>,
        engine: DiffEngine,
    },
}

type RowChanges = Vec<Option<Vec<Range<usize>>>>;

struct Projection {
    text: Rope,
    changes: RowChanges,
}

struct Computed {
    source: Arc<ReviewSource>,
    review: Arc<Review>,
    transaction: Transaction,
    changes: RowChanges,
}

impl Editor {
    /// Open an independent review in a regular editor view. No filesystem identity
    /// is assigned to the projection; the source remains a separate document.
    pub fn open_review(
        &mut self,
        request: ReviewRequest,
        action: Action,
        previous: Option<Arc<ReviewSnapshot>>,
    ) -> DocumentId {
        let review = Arc::new(Review::notice("Loading diff…"));
        let projection = projection(&review);
        let instance = self
            .documents()
            .filter_map(Document::review)
            .filter(|review| {
                review.path() == request.status.path() && review.comparison() == request.comparison
            })
            .map(ReviewDocument::instance)
            .max()
            .unwrap_or(0)
            + 1;
        let state = ReviewDocument {
            request: request.clone(),
            review: review.clone(),
            source: None,
            task: TaskController::new(),
            loading: true,
            initialized: false,
            instance,
            changes: projection.changes,
        };
        let mut doc = Document::from(
            projection.text,
            None,
            self.config.clone(),
            self.syn_loader.load_full(),
        );
        doc.review = Some(Box::new(state));
        let id = self.new_file_from_document(action, doc);
        event::dispatch(crate::events::DocumentDidOpen {
            editor: self,
            doc: id,
        })
        .report_to(self.status_sender());
        self.start_review(
            id,
            Work::Read {
                request: Arc::new(request),
                previous,
            },
        );
        id
    }

    pub fn refresh_review(&mut self) -> Result<()> {
        let (view, doc) = crate::current_ref!(self);
        let state = doc
            .review()
            .ok_or_else(|| anyhow::anyhow!("The current buffer is not a diff review"))?;
        let id = view.document_id();
        let request = ReviewRequest::new(
            self,
            &state.request.status,
            state.comparison(),
            state.workspace(),
        )
        .with_engine(Some(state.engine()))
        .with_fresh_status();
        self.documents
            .get_mut(&id)
            .unwrap()
            .review
            .as_mut()
            .unwrap()
            .request = request.clone();
        self.start_review(
            id,
            Work::Read {
                request: Arc::new(request),
                previous: None,
            },
        );
        Ok(())
    }

    pub fn toggle_review_engine(&mut self) -> Result<()> {
        let (_, doc) = crate::current_ref!(self);
        let id = doc.id();
        let state = doc
            .review()
            .ok_or_else(|| anyhow::anyhow!("The current buffer is not a diff review"))?;
        let engine = state.engine().toggled();
        let source = state.source.clone();
        let request = state.request.clone().with_engine(Some(engine));
        self.documents
            .get_mut(&id)
            .unwrap()
            .review
            .as_mut()
            .unwrap()
            .request = request.clone();
        let work = match source {
            Some(source) => Work::Engine { source, engine },
            None => Work::Read {
                request: Arc::new(request),
                previous: None,
            },
        };
        self.start_review(id, work);
        Ok(())
    }

    fn start_review(&mut self, id: DocumentId, work: Work) {
        let doc = self.documents.get_mut(&id).unwrap();
        let old = doc.snapshot();
        let state = doc.review.as_mut().unwrap();
        let cancel = state.task.restart();
        state.loading = true;
        if matches!(work, Work::Read { .. }) {
            state.source = None;
        }
        let request = match &work {
            Work::Read { request, .. } => Some(request.clone()),
            _ => None,
        };
        let worker = self.handlers.change_review_worker.clone();
        let callbacks = self.callback_sender().clone();
        let check = cancel.clone();
        let before = old.text().clone();
        tokio::spawn(async move {
            let result = run_review(worker, cancel.clone(), move || {
                let (source, review) = match work {
                    Work::Read { request, previous } => {
                        let snapshot = request
                            .compute_cached_cancellable(previous, &|| check.is_canceled())?;
                        (snapshot.source.clone(), snapshot.review.clone())
                    }
                    Work::Engine { source, engine } => {
                        let review = source.compute(Some(engine), &|| check.is_canceled());
                        (source, review)
                    }
                };
                ensure!(!check.is_canceled(), "Review canceled");
                if !review.truncated {
                    source.prepare_syntax(&|| check.is_canceled());
                }
                ensure!(!check.is_canceled(), "Review canceled");
                let after = projection(&review);
                let transaction = text_diff::compare_ropes(&before, &after.text);
                Ok(Computed {
                    source,
                    review,
                    transaction,
                    changes: after.changes,
                })
            })
            .await;
            if cancel.is_canceled() {
                return;
            }
            callbacks
                .send(move |editor| {
                    if cancel.is_canceled()
                        || editor
                            .document(id)
                            .is_none_or(|doc| !old.is_current(&doc.buffer))
                    {
                        return;
                    }
                    let result = if request
                        .as_ref()
                        .is_some_and(|request| !request.is_current(editor))
                    {
                        Err(anyhow::anyhow!(
                            "Source changed while loading the diff; use :diff-refresh"
                        ))
                    } else {
                        result
                    };
                    editor.complete_review(id, cancel, result);
                })
                .await;
        });
        self.redraw().request();
    }

    fn complete_review(&mut self, id: DocumentId, cancel: TaskHandle, result: Result<Computed>) {
        if cancel.is_canceled() {
            return;
        }
        let doc = self.documents.get_mut(&id).unwrap();
        let (source, review, transaction, changes) = match result {
            Ok(Computed {
                source,
                review,
                transaction,
                changes,
            }) => (Some(source), review, transaction, changes),
            Err(error) => {
                let review = Arc::new(Review::notice(&error.to_string()));
                let projection = projection(&review);
                let text = projection.text.to_string();
                let transaction = Transaction::change(
                    doc.text(),
                    std::iter::once((0, doc.text().len_chars(), Some(text.into()))),
                );
                (None, review, transaction, projection.changes)
            }
        };
        let state = doc.review.as_mut().unwrap();
        let initialize = !state.initialized;
        state.initialized = source.is_some();
        state.review = review;
        state.source = source;
        state.changes = changes;
        state.loading = false;
        // Only this coordinator replaces derived content. It uses the normal
        // state-mapping/publication path without enabling editing or recording undo.
        doc.replace_readonly_content(transaction.changes(), initialize)
            .expect("review update uses the captured buffer snapshot");
        // Derived buffers have no undo revisions to replay later. Map native
        // jump histories now, including views currently displaying another file.
        if !initialize {
            for (view, _) in self.tree.views_mut() {
                view.apply(&transaction, doc);
            }
        }
        self.redraw().request();
    }

    /// Use the existing change motions for review hunks, with independent cursors
    /// in every split. `edge` selects the first/last hunk instead of a neighbour.
    pub fn navigate_review_hunk(&mut self, direction: Direction, count: usize, edge: bool) -> bool {
        let (view, doc) = crate::current!(self);
        let Some(state) = doc.review() else {
            return false;
        };
        let line = doc.cursor_line(view.id);
        let rows: Vec<_> = state
            .review
            .lines
            .iter()
            .enumerate()
            .filter_map(|(row, line)| {
                (line.kind == ReviewLineKind::Header).then_some(row + state.row_offset())
            })
            .collect();
        let target = if edge {
            match direction {
                Direction::Forward => rows.last().copied(),
                Direction::Backward => rows.first().copied(),
            }
        } else {
            let targets: Vec<_> = match direction {
                Direction::Forward => rows.into_iter().filter(|row| *row > line).collect(),
                Direction::Backward => rows.into_iter().rev().filter(|row| *row < line).collect(),
            };
            targets
                .get(count.saturating_sub(1).min(targets.len().saturating_sub(1)))
                .copied()
        };
        if let Some(row) = target {
            let selection = Selection::point(doc.text().line_to_char_idx(row));
            doc.set_selection(view.id, selection);
        }
        true
    }

    pub fn open_review_source(&mut self, jump: bool, action: Action) -> Result<()> {
        let (view, doc) = crate::current_ref!(self);
        let state = doc
            .review()
            .ok_or_else(|| anyhow::anyhow!("The current buffer is not a diff review"))?;
        let path = state.path().to_owned();
        let target = if jump {
            ensure!(!state.loading, "Wait for the review to finish loading");
            let source = state
                .source
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No source snapshot is available"))?;
            let row = doc.cursor_line(view.id).checked_sub(state.row_offset());
            let line = row
                .and_then(|row| state.review.after_line_at(row))
                .ok_or_else(|| anyhow::anyhow!("This row has no source location"))?;
            Some((source, line))
        } else {
            None
        };
        ensure!(
            self.document_by_path(&path).is_some() || path.is_file(),
            "This file no longer exists; its deleted text remains in the review"
        );
        let id = self.open(&path, Action::Load)?;
        if let Some((source, _)) = &target {
            ensure!(self.document(id).is_some_and(|doc| !doc.is_binary() && doc.text() == source.after()), "The file differs from this snapshot. Refresh or use :diff-open-file to view its current contents");
        }
        self.switch(id, action);
        if let Some((_, line)) = target {
            let padding = self.config().scroll.padding;
            let (view, doc) = crate::current!(self);
            let line = line.min(
                doc.text()
                    .len_lines(stdx::rope::LINE_TYPE)
                    .saturating_sub(1),
            );
            doc.set_selection(view.id, Selection::point(doc.text().line_to_char_idx(line)));
            view.ensure_cursor_in_view_center(doc, padding);
        }
        Ok(())
    }
}

fn literal(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                '�'
            } else {
                ch
            }
        })
        .collect()
}

/// Preserve tabs for the ordinary text formatter; every other source control is
/// literal replacement text. One projected line always corresponds to one row.
fn projection(review: &Review) -> Projection {
    let mut text = String::new();
    let mut changes = Vec::with_capacity(review.lines.len());
    if let Some(reason) = review.fallback {
        text.push_str(&literal(&reason.to_string()));
        text.push('\n');
    }
    for line in &review.lines {
        changes.push(line.changes.as_ref().map(|ranges| {
            let mut byte = 0;
            let mut scalar = 0;
            ranges
                .iter()
                .map(|range| {
                    scalar += line.text[byte..range.start].chars().count();
                    let start = scalar;
                    scalar += line.text[range.clone()].chars().count();
                    byte = range.end;
                    start..scalar
                })
                .collect()
        }));
        for ch in line.text.chars() {
            text.push(
                if ch != '\t' && (ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}')) {
                    '�'
                } else {
                    ch
                },
            );
        }
        text.push('\n');
    }
    Projection {
        text: Rope::from(text),
        changes,
    }
}
