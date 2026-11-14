//! Automatic linked names. Cached edits mirror before document publication.

use anyhow::{ensure, Context};
use editor_core::{Transaction, Uri};
use event::{cancelable_future, register_hook, TaskController};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId, OffsetEncoding};
use std::sync::Arc;

use crate::{
    callbacks::EditorCallbackSender,
    document::Mode,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentFocusLost, DocumentLanguageDidChange,
        InputApplied, LanguageServerExited, LanguageServerInitialized, ModeDidChange,
        SelectionDidChange,
    },
    Document, DocumentId, Editor, ViewId,
};

mod ranges;
use ranges::Session;

#[derive(Default)]
pub(crate) struct State {
    request: TaskController,
    session: Option<Session>,
    pending: Option<Pending>,
    last_probe: Option<(ViewId, usize, i32, LanguageServerId)>,
    suspended: bool,
}

impl State {
    pub(crate) fn clear(&mut self) {
        self.request.cancel();
        self.session = None;
        self.pending = None;
        self.last_probe = None;
    }
}

struct Pending {
    view: ViewId,
    server: LanguageServerId,
    uri: Option<Uri>,
    before: buffer::Snapshot,
    savepoint: Arc<crate::document::SavePoint>,
    cursor: usize,
    expected_cursor: usize,
}

/// The editor-owned destination also used by synchronous document hooks.
#[derive(Clone)]
pub struct LinkedEditingHandler {
    callbacks: EditorCallbackSender,
}

impl LinkedEditingHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self { callbacks }
    }
}

/// A known pair carried through completion's independently prepared edits.
pub(crate) struct EditCheckpoint {
    session: Option<Session>,
    before: buffer::Snapshot,
    savepoint: Arc<crate::document::SavePoint>,
    view: ViewId,
}

impl EditCheckpoint {
    /// Completion first restores the provider checkpoint; update the cached
    /// ranges there before applying its primary and additional transactions.
    pub(crate) fn restored(&mut self, doc: &mut Document) {
        let projected = self
            .savepoint
            .changes_since(doc)
            .ok()
            .and_then(|changes| self.session.as_mut()?.project(doc.text(), &changes));
        if projected.is_none() {
            self.session = None;
        }
        self.before = doc.snapshot();
        self.savepoint = doc.savepoint(self.view);
    }

    fn finish(mut self, doc: &mut Document) -> Result<(), crate::editing::EditError> {
        let Some(mut session) = self.session.take() else {
            return Ok(());
        };
        // Active placeholders retain their independently rendered selections.
        if doc.active_snippet().is_some() || doc.selection(self.view).len() != 1 {
            return Ok(());
        }
        let Ok(changes) = self.savepoint.changes_since(doc) else {
            return Ok(());
        };
        if !changes.is_empty() {
            let source =
                Transaction::from(changes).with_selection(doc.selection(self.view).clone());
            let Some(mirrored) =
                session.reconcile(self.before.text(), doc.selection(self.view), &source)
            else {
                return Ok(());
            };
            crate::editing::apply(doc, &mirrored.catchup, self.view)?;
        }
        doc.linked_editing.session = Some(session);
        Ok(())
    }
}

/// Run prepared edits at their original coordinates, then reconcile a known
/// linked name. The suspension is restored on both success and failure.
pub(crate) fn prepared_edit<T, E: From<crate::editing::EditError>>(
    doc: &mut Document,
    view: ViewId,
    edit: impl FnOnce(&mut Document, Option<&mut EditCheckpoint>) -> Result<T, E>,
) -> Result<T, E> {
    let session = doc.linked_editing.session.take();
    doc.linked_editing.clear();
    let Some(session) = session else {
        return edit(doc, None);
    };
    let mut checkpoint = EditCheckpoint {
        session: Some(session),
        before: doc.snapshot(),
        savepoint: doc.savepoint(view),
        view,
    };
    let was_suspended = std::mem::replace(&mut doc.linked_editing.suspended, true);
    let result = edit(doc, Some(&mut checkpoint));
    let linked = if result.is_ok() {
        checkpoint.finish(doc)
    } else {
        Ok(())
    };
    doc.linked_editing.suspended = was_suspended;
    linked.map_err(E::from)?;
    result
}

/// Called before mutation so synchronization and observers see one combined change.
pub(crate) fn expand(
    doc: &mut Document,
    transaction: &Transaction,
    origin: Option<ViewId>,
) -> Option<Transaction> {
    if doc.linked_editing.suspended {
        return None;
    }
    let Some(view) = origin else {
        doc.linked_editing.clear();
        return None;
    };
    if doc
        .linked_editing
        .pending
        .as_ref()
        .is_some_and(|pending| pending.view != view)
        || doc.selection(view).len() != 1
    {
        doc.linked_editing.clear();
    }
    let mut session = doc.linked_editing.session.take()?;
    if session.uri != doc.uri()
        || doc.active_snippet().is_some()
        || !session.contains_selection(view, doc.selection(view), doc.text())
        || !doc
            .language_servers_with_feature(LanguageServerFeature::LinkedEditing)
            .next()
            .is_some_and(|server| server.id() == session.server)
    {
        return None;
    }
    let expanded = session.mirror(doc.text(), doc.selection(view), transaction)?;
    doc.linked_editing.session = Some(session);
    Some(expanded.combined)
}

fn discover(doc: &mut Document, view: ViewId, handler: &LinkedEditingHandler) {
    if !doc.config.load().lsp.enable
        || !doc.config.load().lsp.linked_editing
        || doc.is_content_readonly()
        || doc.is_binary()
        || doc.active_snippet().is_some()
        || doc
            .try_selection(view)
            .is_none_or(|selection| selection.len() != 1)
    {
        doc.linked_editing.clear();
        return;
    }
    if doc.linked_editing.suspended {
        return;
    }
    let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
    if doc.linked_editing.pending.is_some() && !doc.linked_editing.request.is_running() {
        // Delivery can be abandoned when a frontend queue closes or drops work.
        doc.linked_editing.clear();
    }
    let Some(server_id) = doc
        .language_servers_with_feature(LanguageServerFeature::LinkedEditing)
        .next()
        .map(|server| server.id())
    else {
        doc.linked_editing.clear();
        return;
    };
    if let Some(mut session) = doc.linked_editing.session.take() {
        let current = session.uri == doc.uri() && session.server == server_id;
        if current && session.view == view {
            if session.contains_selection(view, doc.selection(view), doc.text()) {
                doc.linked_editing.session = Some(session);
                return;
            }
            if let Some(leader) = session
                .ranges
                .iter()
                .position(|range| range.from() <= cursor && cursor < range.to())
            {
                session.leader = leader;
                doc.linked_editing.session = Some(session);
                return;
            }
        }
    }
    if let Some(pending) = &doc.linked_editing.pending {
        if pending.view == view && pending.uri == doc.uri() && pending.server == server_id {
            if pending.expected_cursor == cursor {
                return;
            }
            if !pending.before.is_current(&doc.buffer) {
                // Keep the pre-typing lookup while navigating within a name.
                // Its eventual ranges validate both the edits and final cursor.
                doc.linked_editing.pending.as_mut().unwrap().expected_cursor = cursor;
                return;
            }
        }
        doc.linked_editing.clear();
    }
    let key = (view, cursor, doc.version(), server_id);
    if doc.linked_editing.last_probe == Some(key) {
        return;
    }
    let Some(server) = doc
        .language_servers_with_feature(LanguageServerFeature::LinkedEditing)
        .next()
    else {
        return;
    };
    let server_id = server.id();
    let encoding = server.offset_encoding();
    let position = lsp_client::util::pos_to_lsp_pos(doc.text(), cursor, encoding);
    // Client requests are queued on creation, before a subsequent didChange.
    let Some(future) = server.linked_editing_ranges(doc.identifier(), position) else {
        return;
    };
    let pending = Pending {
        view,
        server: server_id,
        uri: doc.uri(),
        before: doc.snapshot(),
        savepoint: doc.savepoint(view),
        cursor,
        expected_cursor: cursor,
    };
    let cancel = doc.linked_editing.request.restart();
    doc.linked_editing.pending = Some(pending);
    doc.linked_editing.last_probe = Some(key);
    let id = doc.id();
    let callbacks = handler.callbacks.clone();
    tokio::spawn(async move {
        let Some(result) = cancelable_future(future, &cancel).await else {
            return;
        };
        let owner = callbacks.clone();
        callbacks
            .send(move |editor| {
                if cancel.is_canceled()
                    || !owner.same_destination(&editor.handlers.linked_editing.callbacks)
                {
                    return;
                }
                if let Err(error) = apply_discovery(editor, id, view, result, encoding) {
                    log::debug!("linked editing lookup ignored: {error}");
                }
            })
            .await;
    });
}

fn apply_discovery(
    editor: &mut Editor,
    id: DocumentId,
    view: ViewId,
    response: lsp_client::Result<Option<lsp::LinkedEditingRanges>>,
    encoding: OffsetEncoding,
) -> anyhow::Result<()> {
    if !editor.config().lsp.enable
        || !editor.config().lsp.linked_editing
        || editor.tree.focus() != view
        || editor
            .tree
            .try_get(view)
            .is_none_or(|target| target.doc != id)
    {
        if let Some(doc) = editor.document_mut(id) {
            doc.linked_editing.clear();
        }
        return Ok(());
    }
    let Some(doc) = editor.document_mut(id) else {
        return Ok(());
    };
    let Some(pending) = doc.linked_editing.pending.take() else {
        return Ok(());
    };
    if pending.view != view
        || pending.uri != doc.uri()
        || doc.active_snippet().is_some()
        || doc.try_selection(view).is_none_or(|selection| {
            selection.len() != 1
                || selection.primary().cursor(doc.text().slice(..)) != pending.expected_cursor
        })
        || !doc
            .language_servers_with_feature(LanguageServerFeature::LinkedEditing)
            .next()
            .is_some_and(|server| server.id() == pending.server)
    {
        return Ok(());
    }
    let Some(response) = response? else {
        return Ok(());
    };
    doc.validate_editable()?;
    let mut session = Session::from_text(
        pending.before.text(),
        pending.uri,
        view,
        pending.server,
        pending.cursor,
        response,
        encoding,
    )?;
    let changes = pending.savepoint.changes_since(doc)?;
    if changes.is_empty() {
        if !session.contains_selection(view, doc.selection(view), doc.text()) {
            let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
            let Some(leader) = session
                .ranges
                .iter()
                .position(|range| range.from() <= cursor && cursor < range.to())
            else {
                return Ok(());
            };
            session.leader = leader;
        }
        doc.linked_editing.session = Some(session);
        return Ok(());
    }
    // Late mirrors join the original typing group; never amend committed edits.
    ensure!(
        doc.get_current_revision() == pending.before.revision(),
        "Linked edit undo group has ended"
    );
    let (_, original_selection) = pending.savepoint.saved_state(doc)?;
    let source = Transaction::from(changes).with_selection(doc.selection(view).clone());
    let mirrored = session
        .mirror(pending.before.text(), &original_selection, &source)
        .context("Typing left the linked name or its word pattern")?;
    doc.linked_editing.session = Some(session);
    doc.linked_editing.suspended = true;
    let applied = crate::editing::apply(doc, &mirrored.catchup, view);
    doc.linked_editing.suspended = false;
    if applied.is_err() {
        doc.linked_editing.clear();
    }
    applied?;
    editor.redraw().request();
    Ok(())
}

pub(crate) fn refresh(editor: &mut Editor) {
    let Some(target) = editor.tree.try_get(editor.tree.focus()) else {
        return;
    };
    let (id, view) = (target.doc, target.id);
    let handler = editor.handlers.linked_editing.clone();
    if let Some(doc) = editor.document_mut(id) {
        discover(doc, view, &handler);
    }
}

pub(crate) fn mode_changed(editor: &mut Editor, old: Mode) {
    if old == Mode::Insert {
        for doc in editor.documents_mut() {
            if doc.config.load().lsp.linked_editing {
                doc.linked_editing.request.cancel();
                if doc.linked_editing.pending.take().is_some() {
                    doc.linked_editing.last_probe = None;
                }
            } else {
                doc.linked_editing.clear();
            }
        }
    }
}

pub(crate) fn register_hooks(events: &event::EventHub, hooks: crate::handlers::DocumentHooks) {
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        if let Some(handler) = hooks.upgrade().and_then(|services|
            services.get().map(|services| services.linked_editing.clone())) {
            discover(event.doc, event.view, &handler);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentDidChange<'_>| {
        if !event.doc.linked_editing.suspended
            && let Some(pending) = &event.doc.linked_editing.pending
            && event.view == Some(pending.view) {
            let cursor = event.doc.selection(pending.view).primary().cursor(event.doc.text().slice(..));
            event.doc.linked_editing.pending.as_mut().unwrap().expected_cursor = cursor;
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut InputApplied<'_>| {
        refresh(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut ModeDidChange<'_>| {
        refresh(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerInitialized<'_>| {
        refresh(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentLanguageDidChange<'_>| {
        refresh(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) { doc.linked_editing.clear(); }
        refresh(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        cancel(event.editor);
        refresh(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) { doc.linked_editing.clear(); }
        }
        Ok(())
    });
}

pub(crate) fn cancel(editor: &mut Editor) {
    for doc in editor.documents_mut() {
        doc.linked_editing.clear();
    }
}
