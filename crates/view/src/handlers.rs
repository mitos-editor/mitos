use completion::{CompletionEvent, CompletionHandler};
use event::{register_hook, send_blocking};
use spelling::SpellingHandler;
use tokio::sync::mpsc::Sender;

use crate::events::ConfigDidChange;
use crate::handlers::lsp::SignatureHelpInvoked;
use crate::{DocumentId, Editor, ViewId};

pub mod auto_reload;
pub mod code_action_hint;
pub mod completion;
pub mod dap;
pub mod diagnostics;
pub mod document_colors;
pub mod document_highlight;
pub mod document_links;
pub mod document_symbols;
pub mod lsp;
pub mod spelling;
pub mod syntax;
pub mod word_index;

#[derive(Debug)]
pub enum AutoSaveEvent {
    DocumentChanged { save_after: u64 },
    LeftInsertMode,
}

pub struct Handlers {
    pub document_symbols: document_symbols::DocumentSymbolsHandler,
    pub document_highlight: document_highlight::DocumentHighlightHandler,
    pub document_links: document_links::DocumentLinksHandler,
    pub document_colors: document_colors::DocumentColorsHandler,
    pub syntax: syntax::SyntaxHandler,
    // only public because most of the actual implementation is in term right now :/
    pub completions: CompletionHandler,
    pub signature_hints: Sender<lsp::SignatureHelpEvent>,
    pub auto_save: Sender<AutoSaveEvent>,
    pub auto_reload: auto_reload::AutoReloadHandler,
    pub word_index: word_index::Handler,
    pub pull_diagnostics: diagnostics::pull::PullDiagnosticsHandler,
    pub code_action_hint: code_action_hint::CodeActionHintHandler,
    pub spelling: SpellingHandler,
}

impl Handlers {
    /// Manually trigger completion (c-x)
    pub fn trigger_completions(&self, trigger_pos: usize, doc: DocumentId, view: ViewId) {
        self.completions.event(CompletionEvent::ManualTrigger {
            cursor: trigger_pos,
            doc,
            view,
        });
    }

    pub fn trigger_signature_help(&self, invocation: SignatureHelpInvoked, editor: &Editor) {
        let event = match invocation {
            SignatureHelpInvoked::Automatic => {
                if !editor.config().lsp.auto_signature_help {
                    return;
                }
                lsp::SignatureHelpEvent::Trigger
            }
            SignatureHelpInvoked::Manual => lsp::SignatureHelpEvent::Invoked,
        };
        send_blocking(&self.signature_hints, event)
    }

    pub fn word_index(&self) -> &word_index::WordIndex {
        &self.word_index.index
    }
}

pub fn register_hooks(handlers: &Handlers) {
    auto_reload::register_hooks();
    lsp::register_hooks(handlers);
    word_index::register_hooks(handlers);
    // must be done here because the file watcher is in helix-core
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        event.editor.file_watcher.reload(&event.new.file_watcher);
        event.editor.refresh_vcs_watches();
        Ok(())
    });
}
