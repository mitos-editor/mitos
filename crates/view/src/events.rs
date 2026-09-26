use editor_core::{ChangeSet, Rope};
use event::events;
use lsp_client::LanguageServerId;

use crate::{config::Config, Document, DocumentId, Editor, ViewId};

events! {
    DocumentDidOpen<'a> {
        editor: &'a mut Editor,
        doc: DocumentId
    }
    DocumentDidChange<'a> {
        doc: &'a mut Document,
        view: ViewId,
        old_text: &'a Rope,
        changes: &'a ChangeSet,
        ghost_transaction: bool
    }
    DocumentDidClose<'a> {
        editor: &'a mut Editor,
        doc: Document
    }
    SelectionDidChange<'a> { doc: &'a mut Document, view: ViewId }
    DiagnosticsDidChange<'a> { editor: &'a mut Editor, doc: DocumentId }
    // called **after** a document loses focus (but not when its closed)
    DocumentFocusLost<'a> { editor: &'a mut Editor, doc: DocumentId }

    // Configuration is queued before dispatch; document-open hooks precede feature requests.
    LanguageServerInitialized<'a> {
        editor: &'a mut Editor,
        server_id: LanguageServerId
    }
    // Diagnostics are cleared, but the server is still registered during dispatch.
    LanguageServerExited<'a> {
        editor: &'a mut Editor,
        server_id: LanguageServerId
    }

    // NOTE: this event is simple for now and is expected to change as the config system evolves.
    // Ideally it would say what changed.
    ConfigDidChange<'a> {
        editor: &'a mut Editor,
        old: &'a Config,
        new: &'a Config
    }
}

/// Install editor event types before their feature hooks, once per event registry.
pub(crate) fn register() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        use event::register_event;
        register_event::<DocumentDidOpen>();
        register_event::<DocumentDidChange>();
        register_event::<DocumentDidClose>();
        register_event::<DocumentFocusLost>();
        register_event::<SelectionDidChange>();
        register_event::<DiagnosticsDidChange>();
        register_event::<LanguageServerInitialized>();
        register_event::<LanguageServerExited>();
        register_event::<ConfigDidChange>();
    });
}
