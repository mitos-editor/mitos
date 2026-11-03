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
    events: event::EventHub,
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
    #[cfg(feature = "git")]
    pub git_operations: crate::features::git_operations::GitOperationsHandler,
    pub(crate) change_review: crate::features::change_review::Handler,
    pub auto_reload: auto_reload::AutoReloadHandler,
    pub word_index: word_index::Handler,
    pub pull_diagnostics: diagnostics::pull::PullDiagnosticsHandler,
    pub code_action_hint: code_action_hint::CodeActionHintHandler,
    pub spelling: SpellingHandler,
    pub workspace_trust: workspace_trust::WorkspaceTrustHandler,
}

impl Handlers {
    pub fn events(&self) -> &event::EventHub {
        &self.events
    }

    pub fn redraw(&self) -> &event::Redraw {
        self.callbacks.redraw()
    }

    /// Construct services and a private event hub for one editor.
    ///
    /// Call inside the editor's Tokio runtime. The frontend supplies a callback destination
    /// that runs callbacks against this editor; no terminal setup is required.
    pub fn new(config: &Config, callbacks: EditorCallbackSender) -> Self {
        Self::with_events(config, callbacks, event::EventHub::default())
    }

    /// Install services in this editor's hub, after any preinstalled observers.
    /// Supply a fresh hub for each editor; clones share its subscriptions.
    pub fn with_events(
        config: &Config,
        callbacks: EditorCallbackSender,
        events: event::EventHub,
    ) -> Self {
        register_hooks(&events);
        let (status, status_messages) = event::status::StatusSender::channel();
        Self {
            events,
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
            #[cfg(feature = "git")]
            git_operations: crate::features::git_operations::GitOperationsHandler::default(),
            change_review: crate::features::change_review::Handler::new(callbacks.clone()),
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
        doc.events = self.events.clone();
        doc.bind_signals(self.callbacks.redraw().clone(), self.status.clone());
        if doc.historical.is_some() {
            doc.syntax_handler = Some(self.syntax.clone());
        }
        if doc.is_content_readonly() {
            return;
        }
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
        doc.review_trigger = Some(self.change_review.document_trigger());
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
    /// Synchronous observers belonging to this editor, independent of runtime.
    pub fn events(&self) -> &event::EventHub {
        self.handlers.events()
    }

    /// Read or schedule editor services without replacing their document bindings.
    pub fn handlers(&self) -> &Handlers {
        &self.handlers
    }

    /// Dismiss the displayed completion session without canceling a new trigger.
    pub fn dismiss_completions(&mut self) {
        self.finish_completion();
    }
}

// Install shared hooks once when constructing the services owned by an editor.
fn register_hooks(events: &event::EventHub) {
    auto_reload::register_hooks(events);
    auto_save::register_hooks(events);
    blame::register_hooks(events);
    crate::features::change_review::register_hooks(events);
    signature_help::register_hooks(events);
    hover::register_hooks(events);
    navigation::register_hooks(events);
    crate::features::shell::register_hooks(events);
    rename::register_hooks(events);
    buffer_search::register_hooks(events);
    code_actions::register_hooks(events);
    workspace_symbols::register_hooks(events);
    inlay_hints::register_hooks(events);
    completion::register_hooks(events);
    crate::debugger::register_hooks(events);
    word_index::register_hooks(events);
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        event.editor.file_watcher.reload(&event.new.files.watch, &event.editor.workspace);
        event.editor.refresh_vcs_watches();
        Ok(())
    });
    document_highlight::register_hooks(events);
    code_action_hint::register_hooks(events);
    document_symbols::register_hooks(events);
    diagnostics::pull::register_hooks(events);
    snippet::register_hooks(events);
    document_colors::register_hooks(events);
    code_lenses::register_hooks(events);
    document_links::register_hooks(events);
    inline_completion::register_hooks(events);
    spelling::register_hooks(events);
    workspace_trust::register_hooks(events);
}
