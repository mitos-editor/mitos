use crate::{
    callbacks::EditorCallbackSender,
    events::ConfigDidChange,
    features::{
        auto_reload, auto_save, blame, buffer_search, code_action_hint, code_actions, code_lenses,
        completion::{self, CompletionEvent, CompletionHandler},
        diagnostics, document_colors, document_highlight, document_links, document_symbols, hover,
        inlay_hints, inline_completion, navigation, rename,
        signature_help::{self, SignatureHelpInvoked},
        spelling::{self, SpellingHandler},
        syntax, word_index, workspace_symbols, workspace_trust,
    },
    DocumentId, Editor, ViewId,
};
use editor_config::Config;
use event::register_hook;

// Feature implementations live in `features`; this module composes their services
// and retains protocol bridges and registry setup.
pub mod dap;
pub mod lsp;
mod snippet;

/// Services configured before an editor takes ownership. Access owned services through
/// [`Editor::handlers()`]; use explicit editor methods when replacing a running service.
pub struct Handlers {
    pub(crate) lsp_progress: lsp_client::LspProgressMap,
    pub(crate) callbacks: EditorCallbackSender,
    pub(crate) status: event::status::StatusSender,
    pub(crate) status_messages: tokio::sync::mpsc::Receiver<event::status::StatusMessage>,
    pub document_symbols: document_symbols::DocumentSymbolsHandler,
    pub document_highlight: document_highlight::DocumentHighlightHandler,
    pub document_links: document_links::DocumentLinksHandler,
    pub document_colors: document_colors::DocumentColorsHandler,
    pub code_lenses: code_lenses::DocumentCodeLensesHandler,
    pub syntax: syntax::SyntaxHandler,
    pub buffer_search: buffer_search::Handler,
    pub completions: CompletionHandler,
    pub inline_completions: inline_completion::InlineCompletionHandler,
    pub hover: hover::HoverHandler,
    pub navigation: navigation::NavigationHandler,
    pub inlay_hints: inlay_hints::InlayHintsHandler,
    pub signature_hints: signature_help::SignatureHelpHandler,
    pub auto_save: auto_save::AutoSaveHandler,
    pub blame: blame::BlameHandler,
    pub(crate) change_review: parking_lot::Mutex<event::TaskController>,
    pub(crate) change_review_worker: std::sync::Arc<tokio::sync::Semaphore>,
    pub auto_reload: auto_reload::AutoReloadHandler,
    pub word_index: word_index::Handler,
    pub pull_diagnostics: diagnostics::pull::PullDiagnosticsHandler,
    pub code_action_hint: code_action_hint::CodeActionHintHandler,
    pub spelling: SpellingHandler,
    pub workspace_trust: workspace_trust::WorkspaceTrustHandler,
}

impl Handlers {
    pub fn redraw(&self) -> &event::Redraw {
        self.callbacks.redraw()
    }

    /// Construct the services for one editor and install shared hooks once per event registry.
    ///
    /// Call inside the editor's Tokio runtime. The frontend supplies a callback destination
    /// that runs callbacks against this editor; no terminal setup is required.
    pub fn new(config: &Config, callbacks: EditorCallbackSender) -> Self {
        crate::events::register();
        register_hooks();
        let (status, status_messages) = event::status::StatusSender::channel();
        Self {
            lsp_progress: Default::default(),
            status,
            status_messages,
            document_symbols: document_symbols::DocumentSymbolsHandler::new(callbacks.clone()),
            document_highlight: document_highlight::DocumentHighlightHandler::new(
                callbacks.clone(),
            ),
            document_links: document_links::DocumentLinksHandler::new(callbacks.clone()),
            document_colors: document_colors::DocumentColorsHandler::new(callbacks.clone()),
            code_lenses: code_lenses::DocumentCodeLensesHandler::new(callbacks.clone()),
            syntax: syntax::SyntaxHandler::new(callbacks.clone()),
            buffer_search: buffer_search::Handler::new(callbacks.clone()),
            completions: CompletionHandler::new(callbacks.clone(), config),
            inline_completions: inline_completion::InlineCompletionHandler::new(callbacks.clone()),
            hover: hover::HoverHandler::new(callbacks.clone()),
            navigation: navigation::NavigationHandler::new(callbacks.clone()),
            inlay_hints: inlay_hints::InlayHintsHandler::new(callbacks.clone()),
            signature_hints: signature_help::SignatureHelpHandler::new(callbacks.clone()),
            auto_save: auto_save::AutoSaveHandler::new(callbacks.clone()),
            blame: blame::BlameHandler::new(callbacks.clone()),
            change_review: parking_lot::Mutex::default(),
            change_review_worker: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
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
        doc.bind_signals(self.callbacks.redraw().clone(), self.status.clone());
        doc.document_colors
            .bind_handler(self.document_colors.clone());
        doc.code_lenses.bind_handler(self.code_lenses.clone());
        doc.document_links.bind_handler(self.document_links.clone());
        doc.document_highlights
            .bind_handler(self.document_highlight.clone());
        doc.document_symbols
            .bind_handler(self.document_symbols.clone());
        doc.pull_diagnostics
            .bind_handler(self.pull_diagnostics.clone());
        doc.code_action_hints
            .bind_handler(self.code_action_hint.clone());
        doc.inline_completions
            .bind_handler(&self.inline_completions);
        doc.signature_help_trigger = Some(self.signature_hints.document_trigger());
        doc.auto_save_trigger = Some(self.auto_save.trigger());
        doc.word_index_trigger = Some(self.word_index.document_trigger());
        doc.syntax_handler = Some(self.syntax.clone());
        doc.blame.bind_handler(self.blame.clone());
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
        self.word_index.index()
    }
}

impl Editor {
    /// Read or schedule editor services without replacing their document bindings.
    pub fn handlers(&self) -> &Handlers {
        &self.handlers
    }

    /// Dismiss the displayed completion session without canceling a new trigger.
    pub fn dismiss_completions(&mut self) {
        self.finish_completion();
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
        self.finish_completion();
        self.handlers.completions = handler;
        self.handlers.completions.invalidate();
    }

    /// Rebind documents and invalidate work from the previous inline-completion service.
    pub fn replace_inline_completion_handler(
        &mut self,
        handler: inline_completion::InlineCompletionHandler,
    ) {
        self.handlers.inline_completions.cancel();
        self.handlers.inline_completions = handler;
        for doc in self.documents.values_mut() {
            doc.inline_completions.clear();
            doc.inline_completions
                .bind_handler(&self.handlers.inline_completions);
        }
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
        blame::register_hooks();
        crate::features::change_review::register_hooks();
        signature_help::register_hooks();
        hover::register_hooks();
        navigation::register_hooks();
        crate::features::shell::register_hooks();
        rename::register_hooks();
        buffer_search::register_hooks();
        code_actions::register_hooks();
        workspace_symbols::register_hooks();
        inlay_hints::register_hooks();
        completion::register_hooks();
        crate::debugger::register_hooks();
        word_index::register_hooks();
        register_hook!(move |event: &mut ConfigDidChange<'_>| {
            event.editor.file_watcher.reload(&event.new.files.watch);
            event.editor.refresh_vcs_watches();
            Ok(())
        });
        document_highlight::register_hooks();
        code_action_hint::register_hooks();
        document_symbols::register_hooks();
        diagnostics::pull::register_hooks();
        snippet::register_hooks();
        document_colors::register_hooks();
        code_lenses::register_hooks();
        document_links::register_hooks();
        inline_completion::register_hooks();
        spelling::register_hooks();
        workspace_trust::register_hooks();
    });
}
