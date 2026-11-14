//! LSP structural ranges, with syntax-based fallback and editor-owned request lifetimes.

use editor_core::{Range, Selection, Uri};
use event::{cancelable_future, register_hook, TaskController};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId, OffsetEncoding};
use std::sync::Arc;

use crate::{
    document::FoldAction,
    editing::syntax::{self, SelectionAction},
    events::{
        ConfigDidChange, DocumentDidChange, DocumentFocusLost, DocumentLanguageDidChange,
        LanguageServerExited, SelectionDidChange,
    },
    features::request::{DocumentRequest, SelectionRequest},
    Document, DocumentId, Editor, ViewId,
};

/// Callback destination for structural-range requests owned by one editor.
pub struct StructuralRangesHandler {
    callbacks: crate::callbacks::EditorCallbackSender,
}

impl StructuralRangesHandler {
    pub fn new(callbacks: crate::callbacks::EditorCallbackSender) -> Self {
        Self { callbacks }
    }
}

#[derive(Debug)]
pub(crate) struct PendingSelection {
    action: SelectionAction,
    repeats: usize,
}

#[derive(Default)]
pub(crate) struct FoldingRanges {
    request: TaskController,
    cache: Option<FoldCache>,
    pending: Vec<PendingFold>,
}

struct PendingFold {
    target: SelectionRequest,
    operations: Vec<(FoldAction, usize)>,
}

struct FoldCache {
    snapshot: buffer::Snapshot,
    uri: Option<Uri>,
    server: LanguageServerId,
    ranges: Arc<[(usize, usize)]>,
}

impl FoldingRanges {
    pub(crate) fn invalidate(&mut self) {
        self.refresh();
        self.pending.clear();
    }

    fn refresh(&mut self) {
        self.request.cancel();
        self.cache = None;
    }
}

fn fold_provider(doc: &Document) -> Option<&lsp_client::Client> {
    doc.language_servers_with_feature(LanguageServerFeature::FoldingRanges)
        .next()
}

fn cancel(doc: &mut Document) {
    doc.folding_ranges.invalidate();
    for state in doc.view_states.values_mut() {
        state.structural_request.cancel();
        state.structural_repeat = None;
    }
}

fn syntax_fold(
    editor: &mut Editor,
    doc: DocumentId,
    view: ViewId,
    action: FoldAction,
    count: usize,
) {
    let action = match editor.document(doc) {
        Some(doc) => doc.resolve_fold_action(view, action),
        None => return,
    };
    if matches!(
        action,
        FoldAction::Close | FoldAction::Toggle | FoldAction::CloseAll
    ) {
        editor.ensure_syntax(doc);
    }
    let Some(doc) = editor.document_mut(doc) else {
        return;
    };
    for _ in 0..count {
        doc.fold(view, action);
    }
}

impl Editor {
    /// Prefer a configured LSP folding provider; ordinary open/manual operations stay immediate.
    pub fn fold(&mut self, view: ViewId, action: FoldAction, count: usize) {
        let Some(target) = self.tree.try_get(view) else {
            return;
        };
        let id = target.doc;
        let enabled = self.config().lsp.enable;
        let doc = self.document_mut(id).unwrap();
        doc.folding_ranges
            .pending
            .retain(|pending| !pending.target.cancel_handle().is_canceled());
        if let Some(pending) = doc
            .folding_ranges
            .pending
            .iter_mut()
            .find(|pending| pending.target.view_id() == view)
            && !matches!(action, FoldAction::OpenAll)
        {
            pending.operations.push((action, count));
            return;
        }
        doc.view_state_mut(view).structural_request.cancel();
        doc.view_state_mut(view).structural_repeat = None;
        doc.folding_ranges
            .pending
            .retain(|pending| pending.target.view_id() != view);
        let action = doc.resolve_fold_action(view, action);
        let needs_ranges = matches!(
            action,
            FoldAction::Close | FoldAction::Toggle | FoldAction::CloseAll
        );
        let server = enabled
            .then(|| fold_provider(doc))
            .flatten()
            .map(|server| server.id());
        if !needs_ranges || server.is_none() {
            syntax_fold(self, id, view, action, count);
            return;
        }
        let cache = doc
            .folding_ranges
            .cache
            .as_ref()
            .filter(|cache| {
                Some(cache.server) == server
                    && cache.uri == doc.uri()
                    && cache.snapshot.is_current(&doc.buffer)
            })
            .map(|cache| cache.ranges.clone());
        if let Some(ranges) = cache {
            for _ in 0..count {
                doc.fold_with_ranges(view, action, Some(&ranges));
            }
        } else {
            let handle = doc.view_state_mut(view).structural_request.restart();
            let target = SelectionRequest::new(doc, view, handle, vec![server.unwrap()]);
            doc.folding_ranges.pending.push(PendingFold {
                target,
                operations: vec![(action, count)],
            });
            if !doc.folding_ranges.request.is_running() {
                request_folds(self, id);
            }
        }
    }

    /// Invalidate cached server ranges and recompute ranges for visible documents.
    /// Existing closed folds retain their positions, just as they do after syntax refreshes.
    pub fn refresh_folding_ranges(&mut self, server: LanguageServerId) {
        let visible: std::collections::HashSet<_> =
            self.tree.views().map(|(view, _)| view.doc).collect();
        let ids: Vec<_> = self
            .documents()
            .filter(|doc| fold_provider(doc).is_some_and(|provider| provider.id() == server))
            .map(Document::id)
            .collect();
        for id in ids {
            self.document_mut(id).unwrap().folding_ranges.refresh();
            if visible.contains(&id) && self.config().lsp.enable {
                request_folds(self, id);
            }
        }
    }

    /// Expand and shrink through language-server ranges, retaining the existing per-view history.
    /// Sibling and child commands continue to use the syntax tree.
    pub fn select_structure(&mut self, view: ViewId, action: SelectionAction) {
        let Some(target) = self.tree.try_get(view) else {
            return;
        };
        let id = target.doc;
        let enabled = self.config().lsp.enable;
        let mode = self.mode();
        let callbacks = self.handlers.structural_ranges.callbacks.clone();
        let doc = self.document_mut(id).unwrap();
        let state = doc.view_state_mut(view);
        // Repeat motions issued while a response is pending must run after it,
        // rather than canceling one another against the same old selection.
        if matches!(action, SelectionAction::Expand | SelectionAction::Shrink)
            && state.structural_request.is_running()
            && let Some(pending) = &mut state.structural_repeat
            && pending.action == action
        {
            pending.repeats = pending.repeats.saturating_add(1);
            return;
        }
        state.structural_request.cancel();
        state.structural_repeat = None;
        if action == SelectionAction::Shrink {
            let mut history = doc.take_expansion_history(view);
            if let Some(previous) = history.pop() {
                doc.set_expanded_selection(view, previous, history);
                return;
            }
        }
        let server = enabled
            .then(|| {
                doc.language_servers_with_feature(LanguageServerFeature::SelectionRanges)
                    .next()
            })
            .flatten();
        let Some(server) =
            server.filter(|_| matches!(action, SelectionAction::Expand | SelectionAction::Shrink))
        else {
            syntax::select(doc, view, action);
            return;
        };
        let server_id = server.id();
        let encoding = server.offset_encoding();
        let positions = doc
            .selection(view)
            .iter()
            .map(|range| {
                lsp_client::util::pos_to_lsp_pos(
                    doc.text(),
                    range.cursor(doc.text().slice(..)),
                    encoding,
                )
            })
            .collect();
        let future = server.selection_ranges(doc.identifier(), positions);
        let handle = doc.view_state_mut(view).structural_request.restart();
        doc.view_state_mut(view).structural_repeat = Some(PendingSelection { action, repeats: 1 });
        let request = SelectionRequest::new(doc, view, handle, vec![server_id]);
        let owner = callbacks.clone();
        tokio::spawn(async move {
            let Some(result) = cancelable_future(future, request.cancel_handle()).await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if !editor
                        .handlers
                        .structural_ranges
                        .callbacks
                        .same_destination(&owner)
                    {
                        return;
                    }
                    let Some((target, doc)) = request.get(editor) else {
                        return;
                    };
                    if !editor.config().lsp.enable
                        || editor.mode() != mode
                        || editor.tree.focus() != target.id
                        || !doc
                            .language_servers_with_feature(LanguageServerFeature::SelectionRanges)
                            .next()
                            .is_some_and(|server| server.id() == server_id)
                    {
                        return;
                    }
                    let next = match result {
                        Ok(Some(ranges)) => selection(doc, view, ranges, encoding, action),
                        Ok(None) => None,
                        Err(error) => {
                            log::debug!("selection ranges request failed: {error}");
                            None
                        }
                    };
                    let doc = editor.document_mut(id).unwrap();
                    let repeats = doc
                        .view_state_mut(view)
                        .structural_repeat
                        .take()
                        .map_or(1, |pending| pending.repeats);
                    if let Some(next) = next {
                        let current = doc.selection(view).clone();
                        if next != current {
                            let mut history = doc.take_expansion_history(view);
                            if action == SelectionAction::Expand {
                                history.push(current);
                            }
                            doc.set_expanded_selection(view, next, history);
                        }
                    } else {
                        syntax::select(doc, view, action);
                    }
                    for _ in 1..repeats {
                        editor.select_structure(view, action);
                    }
                    editor.redraw().request();
                })
                .await;
        });
    }
}

fn request_folds(editor: &mut Editor, id: DocumentId) {
    let callbacks = editor.handlers.structural_ranges.callbacks.clone();
    let Some(doc) = editor.document_mut(id) else {
        return;
    };
    let Some(server) = fold_provider(doc) else {
        return;
    };
    let server_id = server.id();
    let future = server.folding_ranges(doc.identifier());
    let handle = doc.folding_ranges.request.restart();
    let request = DocumentRequest::new(doc, handle, vec![server_id]);
    let owner = callbacks.clone();
    tokio::spawn(async move {
        let Some(result) = cancelable_future(future, &request.cancel).await else {
            return;
        };
        callbacks
            .send(move |editor| {
                if !editor
                    .handlers
                    .structural_ranges
                    .callbacks
                    .same_destination(&owner)
                {
                    return;
                }
                let Some(doc) = request.get(editor) else {
                    return;
                };
                if !editor.config().lsp.enable
                    || !fold_provider(doc).is_some_and(|server| server.id() == server_id)
                {
                    return;
                }
                let ranges: Option<Arc<[(usize, usize)]>> = match result {
                    Ok(Some(ranges)) => Some(folding_lines(doc, ranges).into()),
                    Ok(None) => None,
                    Err(error) => {
                        log::debug!("folding ranges request failed: {error}");
                        None
                    }
                };
                let doc = editor.document_mut(id).unwrap();
                if let Some(ranges) = &ranges {
                    doc.folding_ranges.cache = Some(FoldCache {
                        snapshot: doc.snapshot(),
                        uri: doc.uri(),
                        server: server_id,
                        ranges: ranges.clone(),
                    });
                }
                // All commands waiting on this document share its range snapshot,
                // but each split retains its own selection guard and ordered actions.
                let pending = std::mem::take(&mut doc.folding_ranges.pending);
                for pending in pending {
                    if pending.target.get(editor).is_none() {
                        continue;
                    }
                    let view = pending.target.view_id();
                    for (action, count) in pending.operations {
                        if let Some(ranges) = &ranges {
                            let doc = editor.document_mut(id).unwrap();
                            for _ in 0..count {
                                doc.fold_with_ranges(view, action, Some(ranges));
                            }
                        } else {
                            syntax_fold(editor, id, view, action, count);
                        }
                    }
                }
                editor.redraw().request();
            })
            .await;
    });
}

fn folding_lines(doc: &Document, ranges: Vec<lsp::FoldingRange>) -> Vec<(usize, usize)> {
    let lines = doc.text().len_lines(stdx::rope::LINE_TYPE);
    ranges
        .into_iter()
        .filter_map(|range| {
            let (start, end) = (range.start_line as usize, range.end_line as usize);
            (start < end && end < lines).then_some((start, end))
        })
        .collect()
}

fn selection(
    doc: &Document,
    view: ViewId,
    ranges: Vec<lsp::SelectionRange>,
    encoding: OffsetEncoding,
    action: SelectionAction,
) -> Option<Selection> {
    let current = doc.selection(view);
    if ranges.len() != current.len() {
        return None;
    }
    let mut next = Vec::with_capacity(ranges.len());
    for (original, root) in current.iter().zip(ranges) {
        let cursor = original.cursor(doc.text().slice(..));
        let mut result = *original;
        let mut node = Some(root);
        let mut child: Option<Range> = None;
        let mut found = false;
        while let Some(range) = node {
            if range.range.start > range.range.end {
                return None;
            }
            let candidate =
                lsp_client::util::lsp_range_to_range(doc.text(), range.range, encoding)?;
            if candidate.from() > cursor
                || candidate.to() < cursor
                || child.is_some_and(|child| {
                    candidate.from() > child.from() || candidate.to() < child.to()
                })
            {
                return None;
            }
            let strict = candidate.from() < original.from() || candidate.to() > original.to();
            if !found
                && action == SelectionAction::Expand
                && candidate.from() <= original.from()
                && candidate.to() >= original.to()
                && strict
            {
                result = candidate;
                found = true;
            }
            if !found
                && action == SelectionAction::Shrink
                && candidate.from() >= original.from()
                && candidate.to() <= original.to()
                && (candidate.from() > original.from() || candidate.to() < original.to())
            {
                result = candidate;
                found = true;
            }
            child = Some(candidate);
            node = range.parent.map(|parent| *parent);
        }
        if original.anchor > original.head {
            result = Range::new(result.to(), result.from());
        }
        next.push(result);
    }
    Some(Selection::new(next.into(), current.primary_index()))
}

pub(crate) fn mode_changed(editor: &mut Editor) {
    for doc in editor.documents_mut() {
        for state in doc.view_states.values_mut() {
            state.structural_request.cancel();
            state.structural_repeat = None;
        }
    }
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut DocumentDidChange<'_>| {
        cancel(event.doc);
        Ok(())
    });
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        if let Some(state) = event.doc.view_states.get_mut(&event.view) {
            state.structural_request.cancel();
            state.structural_repeat = None;
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentLanguageDidChange<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            cancel(doc);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc)
            && let Some(state) = doc.view_states.get_mut(&event.view)
            && state.structural_repeat.is_some()
        {
            // Selection motions belong to focus. Fold operations can finish in
            // their original split while another split becomes active.
            state.structural_request.cancel();
            state.structural_repeat = None;
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut().filter(|doc| doc.supports_language_server(event.server_id)) {
            cancel(doc);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            for doc in event.editor.documents_mut() {
                cancel(doc);
            }
        }
        Ok(())
    });
}
