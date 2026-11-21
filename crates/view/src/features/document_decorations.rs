//! Refresh triggers shared by document colors and links.

use event::register_hook;
use lsp_client::LanguageServerId;

use crate::{
    events::{
        DocumentDidChange, DocumentDidOpen, DocumentLanguageDidChange, LanguageServerExited,
        LanguageServerInitialized,
    },
    handlers::DocumentHooks,
    Document, DocumentId, Editor,
};

use super::{document_colors, document_links};

pub(crate) fn register_hooks(events: &event::EventHub, hooks: DocumentHooks) {
    register_hook!(events => move |event: &mut DocumentDidOpen<'_>| {
        refresh(event.editor, event.doc, false);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentLanguageDidChange<'_>| {
        refresh(event.editor, event.doc, false);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentDidChange<'_>| {
        let services = hooks.upgrade();
        let services = services
            .as_ref()
            .and_then(|services| services.get())
            .filter(|_| !event.doc.is_content_readonly());
        document_colors::DocumentColorsHandler::refresh_after_edit(
            event.doc, services.map(|services| &services.document_colors)
        );
        document_links::DocumentLinksHandler::refresh_after_edit(
            event.doc, services.map(|services| &services.document_links)
        );
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerInitialized<'_>| {
        refresh_attached(event.editor, event.server_id, false);
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        refresh_attached(event.editor, event.server_id, true);
        Ok(())
    });
}

fn refresh(editor: &mut Editor, doc: DocumentId, server_exited: bool) {
    document_colors::refresh(editor, doc, server_exited);
    document_links::refresh(editor, doc, server_exited);
}

fn refresh_attached(editor: &mut Editor, server: LanguageServerId, server_exited: bool) {
    // Initialization opens attachments before notifying features. Exit marks
    // the client as stopping but leaves its attachments available to observers.
    let docs: Vec<_> = editor
        .documents()
        .filter(|doc| doc.supports_language_server(server))
        .map(Document::id)
        .collect();
    for doc in docs {
        refresh(editor, doc, server_exited);
    }
}
