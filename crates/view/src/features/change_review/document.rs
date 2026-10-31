//! Editor-owned review documents. Immutable comparison data is projected into
//! ordinary text so selections, search, layout, and split navigation stay native.

use stdx::rope::RopeExt as _;

use std::{ops::Range, path::Path, sync::Arc};

use anyhow::{ensure, Result};
use editor_core::{movement::Direction, Range as SelectionRange, Rope, Selection, Transaction};
use event::{TaskController, TaskHandle};
use text_diff::ReviewLine;

use super::{
    run_review, DiffEngine, Review, ReviewLineKind, ReviewRequest, ReviewSnapshot, ReviewSource,
};
use crate::{editor::Action, Document, DocumentId, Editor};

pub struct ReviewDocument {
    request: Request,
    review: Arc<Review>,
    source: Option<Arc<ReviewSource>>,
    task: TaskController,
    loading: bool,
    initialized: bool,
    instance: usize,
    changes: RowChanges,
    observed: Option<Arc<Observed>>,
}

#[derive(Clone)]
enum Request {
    Live(ReviewRequest),
    Historical(super::HistoricalReviewRequest),
}
impl Request {
    fn path(&self) -> &Path {
        match self {
            Self::Live(r) => r.status.path(),
            Self::Historical(r) => &r.path,
        }
    }
    fn workspace(&self) -> &Path {
        match self {
            Self::Live(r) => &r.workspace,
            Self::Historical(r) => &r.workspace,
        }
    }
    fn comparison(&self) -> super::ReviewComparison {
        match self {
            Self::Live(r) => r.comparison,
            Self::Historical(_) => super::ReviewComparison::Historical,
        }
    }
    fn engine(&self) -> DiffEngine {
        match self {
            Self::Live(r) => r.engine(),
            Self::Historical(r) => r.settings.engine,
        }
    }
    fn is_current(&self, editor: &Editor) -> bool {
        match self {
            Self::Live(r) => r.is_current(editor),
            Self::Historical(r) => r.is_current(editor),
        }
    }
    fn work(self, previous: Option<Arc<ReviewSnapshot>>) -> Work {
        match self {
            Self::Live(r) => Work::Read {
                request: Arc::new(r),
                previous,
                observed: None,
            },
            Self::Historical(r) => Work::Historical(Arc::new(r)),
        }
    }
}

struct Observed {
    request: ReviewRequest,
    stamp: super::RepositoryStamp,
}

impl ReviewDocument {
    #[cfg(feature = "git")]
    pub(crate) fn permalink_source(
        &self,
        before: bool,
    ) -> Option<super::historical::HistoricalPermalink> {
        match &self.request {
            Request::Historical(request) => request.permalink_source(before),
            Request::Live(_) => None,
        }
    }
    pub(crate) fn is_live(&self) -> bool {
        matches!(self.request, Request::Live(_))
    }

    pub(super) fn is_refreshing(&self) -> bool {
        self.task.is_running()
    }

    pub(super) fn affected_by(&self, path: &Path) -> bool {
        self.observed.as_ref().is_none_or(|observed| {
            observed
                .stamp
                .iter()
                .any(|(watched, _)| watched == path || watched.starts_with(path))
        })
    }

    pub fn revision_label(&self) -> Option<&str> {
        match &self.request {
            Request::Historical(r) => Some(&r.label),
            Request::Live(_) => None,
        }
    }

    pub fn instance(&self) -> usize {
        self.instance
    }
    pub fn path(&self) -> &Path {
        self.request.path()
    }
    pub fn workspace(&self) -> &Path {
        self.request.workspace()
    }
    pub fn comparison(&self) -> super::ReviewComparison {
        self.request.comparison()
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
        self.review.is_some() || self.historical.is_some()
    }
    pub fn review(&self) -> Option<&ReviewDocument> {
        self.review.as_deref()
    }
}

enum Work {
    Historical(Arc<super::HistoricalReviewRequest>),
    Read {
        request: Arc<ReviewRequest>,
        previous: Option<Arc<ReviewSnapshot>>,
        observed: Option<Arc<Observed>>,
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
    source: Option<Arc<ReviewSource>>,
    observed: Option<Arc<Observed>>,
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
        let request = Request::Live(request);
        let work = request.clone().work(previous);
        self.open_review_request(request, action, work)
    }

    pub fn open_historical_review(
        &mut self,
        request: super::HistoricalReviewRequest,
        action: Action,
    ) -> DocumentId {
        let request = Request::Historical(request);
        let work = request.clone().work(None);
        self.open_review_request(request, action, work)
    }

    /// Open the exact previewed sources without reading historical objects again.
    pub fn open_historical_snapshot(
        &mut self,
        snapshot: Arc<super::HistoricalReviewSnapshot>,
        action: Action,
    ) -> Result<DocumentId> {
        ensure!(
            snapshot.request.is_current(self),
            "Workspace trust changed; reload the historical preview"
        );
        let request = snapshot.request.clone();
        let work = Work::Engine {
            source: snapshot.source.clone(),
            engine: request.engine(),
        };
        Ok(self.open_review_request(Request::Historical(request), action, work))
    }

    fn open_review_request(&mut self, request: Request, action: Action, work: Work) -> DocumentId {
        let review = Arc::new(Review::notice("Loading diff…"));
        let projection = projection(&review);
        let instance = self
            .documents()
            .filter_map(Document::review)
            .filter(|review| {
                review.path() == request.path()
                    && review.comparison() == request.comparison()
                    && review.revision_label()
                        == match &request {
                            Request::Historical(r) => Some(r.label.as_str()),
                            Request::Live(_) => None,
                        }
            })
            .map(ReviewDocument::instance)
            .max()
            .unwrap_or(0)
            + 1;
        let state = ReviewDocument {
            request,
            review: review.clone(),
            source: None,
            task: TaskController::new(),
            loading: true,
            initialized: false,
            instance,
            changes: projection.changes,
            observed: None,
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
        self.start_review(id, work);
        // Pathless projections still need the source repository's metadata watches.
        self.refresh_vcs_watches();
        super::live::configure(self);
        id
    }

    pub fn refresh_review(&mut self) -> Result<()> {
        let id = crate::current_ref!(self).0.document_id();
        ensure!(
            self.document(id).is_some_and(|doc| doc.review().is_some()),
            "The current buffer is not a diff review"
        );
        self.refresh_review_document(id, true);
        Ok(())
    }

    pub(super) fn refresh_scheduled_review(&mut self, id: DocumentId) {
        let Some(state) = self.document(id).and_then(Document::review) else {
            return;
        };
        if let Request::Historical(request) = &state.request {
            let mut updated = request.clone();
            updated.refresh_settings(self);
            if request.same_policy(&updated) {
                return;
            }
        }
        self.refresh_review_document(id, false);
    }

    fn refresh_review_document(&mut self, id: DocumentId, force: bool) {
        let state = self.document(id).and_then(Document::review).unwrap();
        let observed = (!force).then(|| state.observed.clone()).flatten();
        let request = match &state.request {
            Request::Live(request) => Request::Live(
                ReviewRequest::new(self, &request.status, state.comparison(), state.workspace())
                    .with_fresh_status(),
            ),
            // A refresh retries the exact captured objects and buffer snapshot.
            Request::Historical(request) => {
                let mut request = request.clone();
                request.refresh_settings(self);
                Request::Historical(request)
            }
        };
        self.documents
            .get_mut(&id)
            .unwrap()
            .review
            .as_mut()
            .unwrap()
            .request = request.clone();
        let mut work = request.work(None);
        if let Work::Read {
            observed: previous, ..
        } = &mut work
        {
            *previous = observed;
        }
        self.start_review(id, work);
    }

    fn start_review(&mut self, id: DocumentId, work: Work) {
        let doc = self.documents.get_mut(&id).unwrap();
        let old = doc.snapshot();
        let state = doc.review.as_mut().unwrap();
        let cancel = state.task.restart();
        // Metadata-only checks should not flash the loading status on every poll.
        state.loading = !matches!(&work, Work::Read { request, observed: Some(previous), .. }
            if previous.request.same_target(request));
        let request = match &work {
            Work::Read { request, .. } => Some(Request::Live((**request).clone())),
            Work::Historical(request) => Some(Request::Historical((**request).clone())),
            Work::Engine { .. } => match &state.request {
                Request::Historical(request) => Some(Request::Historical(request.clone())),
                Request::Live(_) => None,
            },
        };
        let loading = state.loading;
        let worker = self.handlers.change_review.worker.clone();
        let callbacks = self.callback_sender().clone();
        let check = cancel.clone();
        let started_callbacks = callbacks.clone();
        let started_cancel = cancel.clone();
        let before = old.text().clone();
        tokio::spawn(async move {
            let result = run_review(worker, cancel.clone(), move || {
                let mut observed = None;
                let result = match work {
                    Work::Historical(request) => request.compute(&|| check.is_canceled()),
                    Work::Read {
                        request,
                        previous,
                        observed: previous_inputs,
                    } => {
                        let stamp = request.stamp();
                        if previous_inputs.as_ref().is_some_and(|previous| {
                            previous.stamp == stamp && previous.request.same_target(&request)
                        }) {
                            return Ok(None);
                        }
                        if !loading {
                            started_callbacks.send_blocking(move |editor| {
                                if !started_cancel.is_canceled()
                                    && let Some(state) = editor
                                        .documents
                                        .get_mut(&id)
                                        .and_then(|doc| doc.review.as_mut())
                                {
                                    state.loading = true;
                                    editor.redraw().request();
                                }
                            });
                        }
                        observed = Some(Arc::new(Observed {
                            request: (*request).clone(),
                            stamp,
                        }));
                        request
                            .compute_cached_cancellable(previous, &|| check.is_canceled())
                            .and_then(|snapshot| Ok((snapshot.source.clone(), snapshot.diff()?)))
                    }
                    Work::Engine { source, engine } => {
                        let review = source.compute(Some(engine), &|| check.is_canceled());
                        Ok((source, review))
                    }
                };
                let (source, review) = match result {
                    Ok((source, review)) => (Some(source), review),
                    Err(error) => (None, Arc::new(Review::notice(&error.to_string()))),
                };
                ensure!(!check.is_canceled(), "Review canceled");
                if !review.truncated
                    && let Some(source) = &source
                {
                    source.prepare_syntax(&|| check.is_canceled());
                }
                ensure!(!check.is_canceled(), "Review canceled");
                let after = projection(&review);
                let transaction = text_diff::compare_ropes(&before, &after.text);
                Ok(Some(Computed {
                    source,
                    observed,
                    review,
                    transaction,
                    changes: after.changes,
                }))
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
                        if matches!(request, Some(Request::Live(_))) {
                            editor
                                .documents
                                .get_mut(&id)
                                .unwrap()
                                .review
                                .as_mut()
                                .unwrap()
                                .loading = false;
                            super::live::queue(editor, id);
                            return;
                        }
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
        if self.document(id).unwrap().review().unwrap().loading {
            self.redraw().request();
        }
    }

    fn complete_review(
        &mut self,
        id: DocumentId,
        cancel: TaskHandle,
        result: Result<Option<Computed>>,
    ) {
        if cancel.is_canceled() {
            return;
        }
        let doc = self.documents.get_mut(&id).unwrap();
        let (source, review, transaction, changes, observed) = match result {
            Ok(None) => {
                doc.review.as_mut().unwrap().loading = false;
                return;
            }
            Ok(Some(Computed {
                source,
                review,
                transaction,
                changes,
                observed,
            })) => (source, review, transaction, changes, observed),
            Err(error) => {
                let review = Arc::new(Review::notice(&error.to_string()));
                let projection = projection(&review);
                let text = projection.text.to_string();
                let transaction = Transaction::change(
                    doc.text(),
                    std::iter::once((0, doc.text().len_chars(), Some(text.into()))),
                );
                (None, review, transaction, projection.changes, None)
            }
        };
        let state = doc.review.as_mut().unwrap();
        let initialize = !state.initialized;
        state.initialized = source.is_some();
        state.review = review;
        state.source = source;
        state.changes = changes;
        state.observed = observed;
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
        if crate::current_ref!(self).1.review().is_none() {
            return false;
        }
        let wrap = self.config().search.wrap_around;
        self.apply_motion(move |editor| {
            let mode = editor.mode();
            let (view, doc) = crate::current!(editor);
            let state = doc.review().unwrap();
            let rows: Vec<_> = state
                .review
                .lines
                .iter()
                .enumerate()
                .filter_map(|(row, line)| {
                    (line.kind == ReviewLineKind::Header).then_some(row + state.row_offset())
                })
                .collect();
            let status = std::cell::Cell::new(None);
            let text = doc.text().slice(..);
            let selection = doc.selection(view.id).clone().transform(|range| {
                if rows.is_empty() {
                    status.set(Some("No changes in document"));
                    return range;
                }
                let line = range.cursor_line(text);
                let offset = count.saturating_sub(1);
                let index = if edge {
                    Some(if direction == Direction::Forward {
                        rows.len() - 1
                    } else {
                        0
                    })
                } else {
                    let next = match direction {
                        Direction::Forward => rows
                            .iter()
                            .position(|row| *row > line)
                            .map(|i| i.saturating_add(offset).min(rows.len() - 1)),
                        Direction::Backward => rows
                            .iter()
                            .rposition(|row| *row < line)
                            .map(|i| i.saturating_sub(offset)),
                    };
                    next.or_else(|| {
                        status.set(Some(if wrap {
                            "Wrapped around document"
                        } else {
                            "No more changes in document"
                        }));
                        wrap.then(|| match direction {
                            Direction::Forward => offset.min(rows.len() - 1),
                            Direction::Backward => (rows.len() - 1).saturating_sub(offset),
                        })
                    })
                };
                let Some(index) = index else {
                    return range;
                };
                let point = doc.text().line_to_char_idx(rows[index]);
                if mode == crate::document::Mode::Select {
                    SelectionRange::new(range.anchor, point)
                } else {
                    SelectionRange::point(point)
                }
            });
            doc.commit_changes();
            view.sync_changes(doc);
            let jump = (doc.id(), doc.selection(view.id).clone());
            view.push_jump(doc, jump);
            doc.set_selection(view.id, selection);
            if let Some(status) = status.get() {
                editor.set_status(status);
            }
        });
        true
    }

    pub fn open_review_source(&mut self, jump: bool, action: Action) -> Result<()> {
        let (view, doc) = crate::current_ref!(self);
        if !jump && let Some(source) = &doc.historical {
            let path = source.path.clone();
            ensure!(
                path.is_file(),
                "This file no longer exists in the working tree"
            );
            self.open(&path, action)?;
            return Ok(());
        }
        let state = doc
            .review()
            .ok_or_else(|| anyhow::anyhow!("The current buffer is not a diff review"))?;
        let path = state.path().to_owned();
        if jump && let Request::Historical(request) = &state.request {
            ensure!(!state.loading, "Wait for the review to finish loading");
            let source = state
                .source
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No source snapshot is available"))?;
            let row = doc
                .cursor_line(view.id)
                .checked_sub(state.row_offset())
                .and_then(|row| state.review.lines.get(row));
            let before = row.is_some_and(|row| row.kind == ReviewLineKind::Removed);
            let line = row
                .and_then(|row| if before { row.before } else { row.after })
                .or_else(|| {
                    doc.cursor_line(view.id)
                        .checked_sub(state.row_offset())
                        .and_then(|row| state.review.after_line_at(row))
                })
                .ok_or_else(|| anyhow::anyhow!("This row has no source location"))?;
            let request = request.clone();
            self.open_historical_source(&request, source, action, before);
            let (view, doc) = crate::current!(self);
            let line = line.min(
                doc.text()
                    .len_lines(stdx::rope::LINE_TYPE)
                    .saturating_sub(1),
            );
            doc.set_selection(view.id, Selection::point(doc.text().line_to_char_idx(line)));
            let padding = self.config().scroll.padding;
            let (view, doc) = crate::current!(self);
            view.ensure_cursor_in_view_center(doc, padding);
            return Ok(());
        }
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
