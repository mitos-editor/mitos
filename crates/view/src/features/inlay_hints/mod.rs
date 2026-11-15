//! Viewport-aware inlay hints, independent of frontend scheduling and rendering.

use std::sync::Arc;

use event::register_hook;
use language_config::LanguageServerFeature;
use lsp_client::LanguageServerId;

use crate::{
    callbacks::EditorCallbackSender,
    events::{ConfigDidChange, DocumentFocusLost, LanguageServerExited, SelectionDidChange},
};

mod details;
mod requests;
mod state;

pub use details::{hints_on_line, resolve, InlayHintItem, InlayHintTarget, ResolvedInlayHint};
pub use requests::refresh;
pub(crate) use state::HintStyle;
pub(crate) use state::InlayHintsState;
pub use state::{DocumentInlayHints, DocumentInlayHintsId};

/// Full label text, retaining the order of structured label parts.
pub fn label(label: &lsp_client::lsp::InlayHintLabel) -> String {
    match label {
        lsp_client::lsp::InlayHintLabel::String(label) => label.clone(),
        lsp_client::lsp::InlayHintLabel::LabelParts(parts) => {
            parts.iter().map(|part| part.value.as_str()).collect()
        }
    }
}

pub struct InlayHintsHandler {
    callbacks: Arc<EditorCallbackSender>,
}

impl InlayHintsHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            callbacks: Arc::new(callbacks),
        }
    }
}

/// Invalidate the sender's hints, including caches belonging to hidden buffers.
/// Only the first configured inlay provider supplies hints for a document.
pub fn refresh_for_language_server(editor: &mut crate::Editor, server: LanguageServerId) {
    for doc in editor.documents_mut() {
        if doc
            .language_servers_with_feature(LanguageServerFeature::InlayHints)
            .next()
            .is_some_and(|provider| provider.id() == server)
        {
            // Dropping pending request controllers also invalidates queued responses.
            doc.reset_all_inlay_hints();
        }
    }
    refresh(editor);
    editor.redraw().request();
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        if let Some(state) = event.doc.view_states.get_mut(&event.view) {
            state.inlay_hints.details.cancel();
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc)
            && let Some(state) = doc.view_states.get_mut(&event.view) {
            state.inlay_hints.details.cancel();
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.reset_all_inlay_hints();
            }
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if event.old.lsp.inlay_hints != event.new.lsp.inlay_hints
            || event.old.lsp.enable != event.new.lsp.enable
        {
            for doc in event.editor.documents_mut() {
                doc.reset_all_inlay_hints();
            }
        }
        Ok(())
    });
}
