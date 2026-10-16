//! Viewport-aware inlay hints, independent of frontend scheduling and rendering.

use std::sync::Arc;

use event::register_hook;

use crate::{
    callbacks::EditorCallbackSender,
    events::{ConfigDidChange, LanguageServerExited},
};

mod requests;
mod state;

pub use requests::refresh;
pub(crate) use state::InlayHintsState;
pub use state::{DocumentInlayHints, DocumentInlayHintsId};

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

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.reset_all_inlay_hints();
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
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
