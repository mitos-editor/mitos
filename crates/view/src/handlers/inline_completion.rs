//! Editor-owned inline completions. Previews never modify the document.
use parking_lot::Mutex;
use std::{sync::Arc, time::Duration};

use editor_core::{
    movement::Direction, syntax::config::LanguageServerFeature, Range, Selection, Transaction,
};
use event::{cancelable_future, register_hook, send_blocking, AsyncHook, TaskController};
use lsp_client::{lsp, util::lsp_range_to_range, LanguageServerId, OffsetEncoding};
use tokio::{sync::mpsc::Sender, time::Instant};

use super::lsp::DocumentRequest;
use crate::{
    callbacks::EditorCallbackSender,
    document::{InlineCompletion, Mode},
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentFocusLost,
        LanguageServerExited, SelectionDidChange,
    },
    DocumentId, Editor, ViewId,
};

struct AutomaticRequest {
    doc: DocumentId,
    view: ViewId,
    cursor: usize,
    timeout: Duration,
    cancel: event::TaskHandle,
}

#[derive(Default)]
struct InlineCompletionSession {
    controller: TaskController,
    target: Option<(DocumentId, ViewId, usize)>,
}

#[derive(Clone)]
pub(crate) struct InlineCompletionTrigger {
    events: Sender<Option<AutomaticRequest>>,
    session: Arc<Mutex<InlineCompletionSession>>,
}

impl InlineCompletionTrigger {
    fn start(&self, doc: DocumentId, view: ViewId, cursor: usize) -> event::TaskHandle {
        let mut session = self.session.lock();
        let cancel = session.controller.restart();
        session.target = Some((doc, view, cursor));
        cancel
    }
    fn schedule(&self, doc: DocumentId, view: ViewId, cursor: usize, timeout: Duration) {
        let cancel = self.start(doc, view, cursor);
        send_blocking(
            &self.events,
            Some(AutomaticRequest {
                doc,
                view,
                cursor,
                timeout,
                cancel,
            }),
        );
    }
    fn cancel(&self) {
        let mut session = self.session.lock();
        session.controller.cancel();
        session.target = None;
        drop(session);
        send_blocking(&self.events, None);
    }
    fn cancel_document(&self, doc: DocumentId) {
        if self
            .session
            .lock()
            .target
            .is_some_and(|(id, _, _)| id == doc)
        {
            self.cancel();
        }
    }
    pub(crate) fn cancel_view(&self, doc: DocumentId, view: ViewId) {
        if self
            .session
            .lock()
            .target
            .is_some_and(|(id, v, _)| id == doc && v == view)
        {
            self.cancel();
        }
    }
    fn selection_changed(&self, doc: &crate::Document, view: ViewId) {
        let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
        if self.session.lock().target.is_some_and(|(id, v, pos)| {
            id == doc.id() && v == view && (pos != cursor || doc.selection(view).len() != 1)
        }) {
            self.cancel();
        }
    }
}

pub struct InlineCompletionHandler {
    pub(super) trigger: InlineCompletionTrigger,
    callbacks: EditorCallbackSender,
}

impl InlineCompletionHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            trigger: InlineCompletionTrigger {
                events: Debounce {
                    callbacks: callbacks.clone(),
                    target: None,
                }
                .spawn(),
                session: Arc::new(Mutex::new(InlineCompletionSession::default())),
            },
            callbacks,
        }
    }
    pub(crate) fn cancel(&self) {
        self.trigger.cancel();
    }
}

struct Debounce {
    callbacks: EditorCallbackSender,
    target: Option<AutomaticRequest>,
}

impl AsyncHook for Debounce {
    type Event = Option<AutomaticRequest>;
    fn handle_event(&mut self, target: Self::Event, _: Option<Instant>) -> Option<Instant> {
        self.target = target;
        self.target
            .as_ref()
            .map(|target| Instant::now() + target.timeout)
    }
    fn finish_debounce(&mut self) {
        if let Some(target) = self.target.take() {
            self.callbacks.send_blocking(move |editor| {
                if !target.cancel.is_canceled()
                    && editor.config().inline_completion_auto_trigger
                    && editor.tree.focus == target.view
                    && editor
                        .tree
                        .try_get(target.view)
                        .is_some_and(|v| v.doc == target.doc)
                    && editor.document(target.doc).is_some_and(|doc| {
                        doc.selection(target.view)
                            .primary()
                            .cursor(doc.text().slice(..))
                            == target.cursor
                    })
                {
                    trigger(editor, lsp::InlineCompletionTriggerKind::Automatic);
                }
            });
        }
    }
}

pub fn trigger(editor: &mut Editor, kind: lsp::InlineCompletionTriggerKind) {
    if editor.mode() != Mode::Insert || !editor.config().lsp.enable {
        return;
    }
    if matches!(
        editor.last_completion,
        Some(crate::editor::CompleteAction::Selected { .. })
    ) {
        dismiss(editor);
        return;
    }
    editor.handlers.inline_completions.cancel();
    let callbacks = editor.handlers.inline_completions.callbacks.clone();
    let trigger = editor.handlers.inline_completions.trigger.clone();
    let (view, doc) = current!(editor);
    doc.inline_completions.clear();
    // A primary-only suggestion must never collapse a multiple-cursor selection.
    if doc.selection(view.id).len() != 1 {
        return;
    }
    let Some(server) = doc
        .language_servers_with_feature(LanguageServerFeature::InlineCompletion)
        .next()
    else {
        return;
    };
    let doc_id = doc.id();
    let view_id = view.id;
    let selection = doc.selection(view_id).clone();
    let cursor = selection.primary().cursor(doc.text().slice(..));
    let encoding = server.offset_encoding();
    let server_id = server.id();
    let Some(future) =
        server.inline_completion(doc.identifier(), doc.position(view_id, encoding), kind)
    else {
        return;
    };
    doc.inline_completions.view = Some(view_id);
    let cancel = doc.inline_completions.controller.restart();
    let request = DocumentRequest::new(doc, cancel.clone(), vec![server_id]);
    let session_cancel = trigger.start(doc_id, view_id, cursor);
    tokio::spawn(async move {
        let Some(Some(result)) =
            cancelable_future(cancelable_future(future, &cancel), &session_cancel).await
        else {
            return;
        };
        callbacks
            .send(move |editor| {
                if session_cancel.is_canceled()
                    || !request.is_current(editor)
                    || editor.mode() != Mode::Insert
                    || editor.tree.focus != view_id
                    || !editor
                        .tree
                        .try_get(view_id)
                        .is_some_and(|view| view.doc == doc_id)
                    || !editor
                        .document(doc_id)
                        .is_some_and(|doc| doc.selection(view_id) == &selection)
                {
                    return;
                }
                let items = match result {
                    Ok(Some(lsp::InlineCompletionResponse::Array(items))) => items,
                    Ok(Some(lsp::InlineCompletionResponse::List(list))) => list.items,
                    Ok(None) => Vec::new(),
                    Err(err) => {
                        log::error!("inline completion request failed: {err}");
                        return;
                    }
                };
                let doc = editor.document_mut(doc_id).unwrap();
                let items = items
                    .into_iter()
                    .filter_map(|item| prepare(doc.text(), cursor, encoding, server_id, item))
                    .collect();
                doc.inline_completions.items = items;
                doc.inline_completions.view = Some(view_id);
                event::request_redraw();
            })
            .await;
    });
}

fn prepare(
    text: &editor_core::Rope,
    cursor: usize,
    encoding: OffsetEncoding,
    server: LanguageServerId,
    item: lsp::InlineCompletionItem,
) -> Option<InlineCompletion> {
    // Snippets require tabstop expansion, so don't offer them as literal ghost text.
    if item.insert_text_format == Some(lsp::InsertTextFormat::SNIPPET) {
        return None;
    }
    let range = match item.range {
        Some(range) => {
            if range.start.line != range.end.line
                || range.start > range.end
                || range.start.line as usize != text.char_to_line(cursor)
            {
                return None;
            }
            lsp_range_to_range(text, range, encoding)?
        }
        None => Range::point(cursor),
    };
    if cursor < range.from() || cursor > range.to() {
        return None;
    }
    let replaced = text.slice(range.from()..range.to()).to_string();
    let filter = item
        .filter_text
        .as_deref()
        .filter(|text| !text.is_empty())
        .unwrap_or(&item.insert_text);
    if !filter.starts_with(&replaced) {
        return None;
    }
    let prefix = text.slice(range.from()..cursor).to_string();
    let ghost = item.insert_text.strip_prefix(&prefix)?;
    if ghost.is_empty() {
        return None;
    }
    let line = text.char_to_line(cursor);
    let line_end = if line + 1 < text.len_lines() {
        text.line_to_char(line + 1)
    } else {
        text.len_chars()
    };
    let suffix = text.slice(range.to()..line_end).to_string();
    let preview = format!("{ghost}{}", suffix.trim_end_matches(['\r', '\n']));
    Some(InlineCompletion {
        cursor,
        range,
        text: item.insert_text,
        lines: preview
            .split('\n')
            .map(|line| line.trim_end_matches('\r').to_owned())
            .collect(),
        server,
        command: item.command,
    })
}

pub fn dismiss(editor: &mut Editor) {
    editor.handlers.inline_completions.cancel();
    let (_, doc) = current!(editor);
    doc.inline_completions.clear();
}

/// Concrete edit returned to frontends for insert replay.
#[derive(Debug, Clone)]
pub struct AppliedInlineCompletion {
    pub cursor: usize,
    pub changes: Vec<editor_core::Change>,
    pub selection: usize,
}

pub fn accept(editor: &mut Editor) -> Option<AppliedInlineCompletion> {
    if editor.mode() != Mode::Insert {
        return None;
    }
    editor.handlers.inline_completions.cancel();
    let (view, doc) = current!(editor);
    doc.inline_completion(view.id)?;
    let completion = doc
        .inline_completions
        .items
        .remove(doc.inline_completions.index);
    doc.inline_completions.clear();
    let original_cursor = completion.cursor;
    let changes = vec![(
        completion.range.from(),
        completion.range.to(),
        Some(completion.text.clone().into()),
    )];
    let cursor = completion.range.from() + completion.text.chars().count();
    let transaction = Transaction::change(doc.text(), changes.clone().into_iter())
        .with_selection(Selection::point(cursor));
    if !doc.apply(&transaction, view.id) {
        return None;
    }
    // The server may attach a command to record acceptance.
    if let Some(command) = completion.command
        && let Some(server) = editor.language_server_by_id(completion.server)
        && let Some(future) = server.command(command)
    {
        tokio::spawn(async move {
            if let Err(err) = future.await {
                log::error!("inline completion command failed: {err}");
            }
        });
    }
    Some(AppliedInlineCompletion {
        cursor: original_cursor,
        changes,
        selection: cursor,
    })
}

pub fn cycle(editor: &mut Editor, direction: Direction) {
    let (view, doc) = current!(editor);
    doc.inline_completions.cycle(view.id, direction);
}

pub fn mode_changed(editor: &mut Editor, old: Mode) {
    if old == Mode::Insert && editor.mode() != Mode::Insert {
        dismiss(editor);
    }
}

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event.doc.inline_completions.clear();
        if event.ghost_transaction
            && let Some(trigger) = &event.doc.inline_completions.trigger
        {
            trigger.cancel_document(event.doc.id());
        }
        if !event.ghost_transaction
            && event.doc.config.load().inline_completion_auto_trigger
            && let Some(tx) = &event.doc.inline_completions.trigger
        {
            tx.schedule(
                event.doc.id(),
                event.view,
                event
                    .doc
                    .selection(event.view)
                    .primary()
                    .cursor(event.doc.text().slice(..)),
                event.doc.config.load().inline_completion_timeout,
            );
        }
        Ok(())
    });
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if let Some(trigger) = &event.doc.inline_completions.trigger {
            trigger.selection_changed(event.doc, event.view);
        }
        if event.doc.inline_completions.view == Some(event.view) {
            event.doc.inline_completions.clear();
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        event
            .editor
            .handlers
            .inline_completions
            .trigger
            .cancel_document(event.doc.id());
        event.doc.inline_completions.clear();
        Ok(())
    });
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        event.editor.handlers.inline_completions.cancel();
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.inline_completions.clear();
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                if let Some(trigger) = &doc.inline_completions.trigger {
                    trigger.cancel_document(doc.id());
                }
                doc.inline_completions.clear();
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        event.editor.handlers.inline_completions.cancel();
        for doc in event.editor.documents.values_mut() {
            doc.inline_completions.clear();
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::Rope;

    fn item(text: &str) -> lsp::InlineCompletionItem {
        lsp::InlineCompletionItem {
            insert_text: text.into(),
            filter_text: None,
            range: None,
            command: None,
            insert_text_format: None,
        }
    }

    #[test]
    fn preview_at_eof_without_a_newline_and_with_crlf_suffix() {
        let text = Rope::from_str("🙂");
        let completion = prepare(
            &text,
            1,
            OffsetEncoding::Utf16,
            LanguageServerId::default(),
            item("界\n\tend"),
        )
        .unwrap();
        assert_eq!(completion.range, Range::point(1));
        assert_eq!(completion.lines, ["界", "\tend"]);
        let text = Rope::from_str("pr!\r\n");
        let completion = prepare(
            &text,
            2,
            OffsetEncoding::Utf16,
            LanguageServerId::default(),
            item("int\r\nend"),
        )
        .unwrap();
        assert_eq!(completion.lines, ["int", "end!"]);
    }

    #[test]
    fn filtering_checks_the_replacement_prefix_and_preserves_typed_unicode() {
        let text = Rope::from_str("éx!");
        let mut suggestion = item("éxyz");
        suggestion.range = Some(lsp::Range::new(
            lsp::Position::new(0, 0),
            lsp::Position::new(0, 2),
        ));
        let completion = prepare(
            &text,
            1,
            OffsetEncoding::Utf16,
            LanguageServerId::default(),
            suggestion.clone(),
        )
        .unwrap();
        assert_eq!(completion.lines, ["xyz!"]);
        suggestion.filter_text = Some("different".into());
        assert!(prepare(
            &text,
            1,
            OffsetEncoding::Utf16,
            LanguageServerId::default(),
            suggestion.clone()
        )
        .is_none());
        suggestion.filter_text = Some(String::new());
        assert!(prepare(
            &text,
            1,
            OffsetEncoding::Utf16,
            LanguageServerId::default(),
            suggestion
        )
        .is_some());
    }

    #[test]
    fn invalid_ranges_empty_suggestions_and_snippets_are_not_offered() {
        let text = Rope::from_str("pr\n");
        let server = LanguageServerId::default();
        assert!(prepare(&text, 2, OffsetEncoding::Utf16, server, item("")).is_none());
        let mut suggestion = item("print");
        suggestion.range = Some(lsp::Range::new(
            lsp::Position::new(0, 0),
            lsp::Position::new(1, 0),
        ));
        assert!(prepare(&text, 2, OffsetEncoding::Utf16, server, suggestion.clone()).is_none());
        suggestion.range = Some(lsp::Range::new(
            lsp::Position::new(0, 2),
            lsp::Position::new(0, 0),
        ));
        assert!(prepare(&text, 2, OffsetEncoding::Utf16, server, suggestion.clone()).is_none());
        suggestion.range = None;
        suggestion.insert_text_format = Some(lsp::InsertTextFormat::SNIPPET);
        assert!(prepare(&text, 2, OffsetEncoding::Utf16, server, suggestion).is_none());
    }
}
