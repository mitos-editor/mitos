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
mod document_debounce;
pub mod document_highlight;
pub mod document_links;
pub mod document_symbols;
pub mod lsp;
pub mod signature_help;
mod snippet;
pub mod spelling;
pub mod syntax;
pub mod word_index;
pub mod workspace_trust;

/// Services configured before an editor takes ownership. Access owned services through
/// [`Editor::handlers()`]; use explicit editor methods when replacing a running service.
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
    pub workspace_trust: workspace_trust::WorkspaceTrustHandler,
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
            workspace_trust: workspace_trust::WorkspaceTrustHandler::default(),
            callbacks,
        }
    }

    /// Attach every document-scoped service before opening or initializing a document.
    pub(crate) fn attach_document(&self, doc: &mut crate::Document) {
        doc.document_colors.handler = Some(self.document_colors.clone());
        doc.document_links.handler = Some(self.document_links.clone());
        doc.document_highlights.handler = Some(self.document_highlight.clone());
        doc.document_symbols.handler = Some(self.document_symbols.clone());
        doc.pull_diagnostics.handler = Some(self.pull_diagnostics.clone());
        doc.code_action_hints.handler = Some(self.code_action_hint.clone());
        doc.signature_help_trigger = Some(self.signature_hints.document_trigger());
        doc.auto_save_trigger = Some(self.auto_save.trigger());
        doc.word_index_trigger = Some(self.word_index.document_trigger());
        doc.syntax_handler = Some(self.syntax.clone());
        doc.spelling_events = Some(self.spelling.event_tx.clone());
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

impl Editor {
    /// Read or schedule editor services without replacing their document bindings.
    pub fn handlers(&self) -> &Handlers {
        &self.handlers
    }

    /// Dismiss the displayed completion session without canceling a new trigger.
    pub fn dismiss_completions(&mut self) {
        self.handlers.completions.dismiss();
    }

    /// Replace autosave coordination, rejecting queued saves from its previous owner.
    /// Existing documents schedule subsequent edits through the new handler.
    pub fn replace_auto_save_handler(&mut self, handler: auto_save::AutoSaveHandler) {
        self.handlers.auto_save = handler;
        for doc in self.documents.values_mut() {
            doc.auto_save_trigger = Some(self.handlers.auto_save.trigger());
        }
    }

    /// Replace signature help and rebind existing documents. Pending results from
    /// the previous handler retain its owner identity and cannot publish.
    pub fn replace_signature_help_handler(
        &mut self,
        handler: signature_help::SignatureHelpHandler,
    ) {
        self.handlers.signature_hints.cancel();
        self.handlers.signature_hints = handler;
        self.handlers.signature_hints.dismiss_replaced();
        for doc in self.documents.values_mut() {
            doc.signature_help_trigger = Some(self.handlers.signature_hints.document_trigger());
        }
    }

    /// Replace completion coordination and cancel its pending and displayed work.
    pub fn replace_completion_handler(&mut self, handler: CompletionHandler) {
        self.handlers.completions.event(CompletionEvent::Cancel);
        self.handlers.completions.dismiss();
        self.handlers.completions = handler;
        self.handlers.completions.invalidate();
    }

    /// Replace reload coordination, invalidating prompts and results from the old owner.
    /// File-watcher delivery remains independent of the reload handler's callback queue.
    pub fn replace_auto_reload_handler(&mut self, handler: auto_reload::AutoReloadHandler) {
        self.handlers.auto_reload = handler;
    }

    /// Forget queued trust prompts and their decisions without changing trust policy.
    pub fn reset_workspace_trust_prompts(&mut self) {
        self.handlers.workspace_trust = workspace_trust::WorkspaceTrustHandler::default();
    }
}

// This is the only entry point for shared hook registration. Keep the guard scoped
// to the event registry so separate test runtimes each install their own hooks.
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
        workspace_trust::register_hooks();
    });
}
