use completion::{CompletionEvent, CompletionHandler};
use event::register_hook;
use spelling::SpellingHandler;

use crate::handlers::signature_help::SignatureHelpInvoked;
use crate::{callbacks::EditorCallbackSender, config::Config, events::ConfigDidChange};
use crate::{DocumentId, Editor, ViewId};

pub mod auto_reload;
pub mod auto_save;
pub mod code_action_hint;
pub mod completion;
pub mod dap;
pub mod diagnostics;
pub mod document_colors;
pub mod document_highlight;
pub mod document_links;
pub mod document_symbols;
pub mod lsp;
pub mod signature_help;
mod snippet;
pub mod spelling;
pub mod syntax;
pub mod word_index;

pub struct Handlers {
    pub(crate) callbacks: EditorCallbackSender,
    pub document_symbols: document_symbols::DocumentSymbolsHandler,
    pub document_highlight: document_highlight::DocumentHighlightHandler,
    pub document_links: document_links::DocumentLinksHandler,
    pub document_colors: document_colors::DocumentColorsHandler,
    pub syntax: syntax::SyntaxHandler,
    pub completions: CompletionHandler,
    pub signature_hints: signature_help::SignatureHelpHandler,
    pub auto_save: auto_save::AutoSaveHandler,
    pub auto_reload: auto_reload::AutoReloadHandler,
    pub word_index: word_index::Handler,
    pub pull_diagnostics: diagnostics::pull::PullDiagnosticsHandler,
    pub code_action_hint: code_action_hint::CodeActionHintHandler,
    pub spelling: SpellingHandler,
}

impl Handlers {
    /// Construct the services for one editor and install shared hooks once per event registry.
    ///
    /// Call inside the editor's Tokio runtime. The frontend supplies a callback destination
    /// that runs callbacks against this editor; no terminal setup is required.
    pub fn new(config: &Config, callbacks: EditorCallbackSender) -> Self {
        crate::events::register();
        register_hooks();
        Self {
            document_symbols: document_symbols::DocumentSymbolsHandler::new(callbacks.clone()),
            document_highlight: document_highlight::DocumentHighlightHandler::new(
                callbacks.clone(),
            ),
            document_links: document_links::DocumentLinksHandler::new(callbacks.clone()),
            document_colors: document_colors::DocumentColorsHandler::new(callbacks.clone()),
            syntax: syntax::SyntaxHandler::new(callbacks.clone()),
            completions: CompletionHandler::new(callbacks.clone(), config),
            signature_hints: signature_help::SignatureHelpHandler::new(callbacks.clone()),
            auto_save: auto_save::AutoSaveHandler::new(callbacks.clone()),
            auto_reload: auto_reload::AutoReloadHandler::new(callbacks.clone(), config),
            word_index: word_index::Handler::spawn(),
            pull_diagnostics: diagnostics::pull::PullDiagnosticsHandler::new(callbacks.clone()),
            code_action_hint: code_action_hint::CodeActionHintHandler::new(callbacks.clone()),
            spelling: SpellingHandler::new(callbacks.clone()),
            callbacks,
        }
    }

    /// Manually trigger completion (c-x)
    pub fn trigger_completions(&self, trigger_pos: usize, doc: DocumentId, view: ViewId) {
        self.completions.event(CompletionEvent::ManualTrigger {
            cursor: trigger_pos,
            doc,
            view,
        });
    }

    pub fn trigger_signature_help(&self, invocation: SignatureHelpInvoked, editor: &Editor) {
        self.signature_hints.trigger(editor, invocation);
    }

    pub fn word_index(&self) -> &word_index::WordIndex {
        &self.word_index.index
    }
}

fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        auto_reload::register_hooks();
        auto_save::register_hooks();
        signature_help::register_hooks();
        completion::register_hooks();
        // Register didOpen/didChange before features can request results from the server.
        lsp::register_hooks();
        word_index::register_hooks();
        register_hook!(move |event: &mut ConfigDidChange<'_>| {
            event.editor.file_watcher.reload(&event.new.file_watcher);
            event.editor.refresh_vcs_watches();
            Ok(())
        });
        document_highlight::register_hooks();
        code_action_hint::register_hooks();
        document_symbols::register_hooks();
        diagnostics::pull::register_hooks();
        snippet::register_hooks();
        document_colors::register_hooks();
        document_links::register_hooks();
        spelling::register_hooks();
    });
}
