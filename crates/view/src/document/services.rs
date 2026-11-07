//! One immutable scheduling destination shared by an editor's documents.

use std::sync::Arc;

use crate::{features, handlers::Handlers, Document};

pub(crate) struct DocumentServices {
    events: event::EventHub,
    redraw: event::Redraw,
    status: event::status::StatusSender,
    pub document_colors: features::document_colors::DocumentColorsHandler,
    pub code_lenses: features::code_lenses::DocumentCodeLensesHandler,
    pub document_links: features::document_links::DocumentLinksHandler,
    pub document_highlight: features::document_highlight::DocumentHighlightHandler,
    pub document_symbols: features::document_symbols::DocumentSymbolsHandler,
    pub pull_diagnostics: features::diagnostics::pull::PullDiagnosticsHandler,
    pub code_action_hint: features::code_action_hint::CodeActionHintHandler,
    pub inline_completion: features::inline_completion::InlineCompletionTrigger,
    pub signature_help: features::signature_help::SignatureHelpTrigger,
    pub auto_save: features::auto_save::AutoSaveTrigger,
    pub review: features::change_review::ReviewTrigger,
    pub word_index: features::word_index::WordIndexTrigger,
    pub syntax: features::syntax::SyntaxHandler,
    pub blame: features::blame::BlameHandler,
    pub spelling: tokio::sync::mpsc::Sender<features::spelling::SpellingEvent>,
}

impl DocumentServices {
    pub(crate) fn new(handlers: &Handlers) -> Self {
        Self {
            events: handlers.events().clone(),
            redraw: handlers.callbacks.redraw().clone(),
            status: handlers.status.clone(),
            document_colors: handlers.document_colors.clone(),
            code_lenses: handlers.code_lenses.clone(),
            document_links: handlers.document_links.clone(),
            document_highlight: handlers.document_highlight.clone(),
            document_symbols: handlers.document_symbols.clone(),
            pull_diagnostics: handlers.pull_diagnostics.clone(),
            code_action_hint: handlers.code_action_hint.clone(),
            inline_completion: handlers.inline_completions.document_trigger(),
            signature_help: handlers.signature_hints.document_trigger(),
            auto_save: handlers.auto_save.trigger(),
            review: handlers.change_review.document_trigger(),
            word_index: handlers.word_index.document_trigger(),
            syntax: handlers.syntax.clone(),
            blame: handlers.blame.clone(),
            spelling: handlers.spelling.event_tx.clone(),
        }
    }
}

pub(super) enum ServiceBinding {
    Standalone {
        redraw: event::Redraw,
        events: event::EventHub,
    },
    Editing(Arc<DocumentServices>),
    Historical(Arc<DocumentServices>),
    ReadOnly(Arc<DocumentServices>),
}

impl Document {
    pub(crate) fn bind_services(&mut self, services: Arc<DocumentServices>) {
        self.services = if self.historical.is_some() {
            ServiceBinding::Historical(services)
        } else if self.is_content_readonly() {
            ServiceBinding::ReadOnly(services)
        } else {
            self.document_links.cancel_navigation();
            ServiceBinding::Editing(services)
        };
    }

    pub(crate) fn services(&self) -> Option<&DocumentServices> {
        match &self.services {
            ServiceBinding::Editing(services) => Some(services),
            _ => None,
        }
    }

    pub(super) fn syntax_handler(&self) -> Option<&features::syntax::SyntaxHandler> {
        match &self.services {
            ServiceBinding::Editing(services) | ServiceBinding::Historical(services) => {
                Some(&services.syntax)
            }
            _ => None,
        }
    }

    /// Observers bound to this document; attached documents use their editor's hub.
    pub fn events(&self) -> &event::EventHub {
        match &self.services {
            ServiceBinding::Standalone { events, .. } => events,
            ServiceBinding::Editing(services)
            | ServiceBinding::Historical(services)
            | ServiceBinding::ReadOnly(services) => &services.events,
        }
    }

    /// Standalone documents retain their own signal; attached documents use their editor's.
    pub fn redraw(&self) -> &event::Redraw {
        match &self.services {
            ServiceBinding::Standalone { redraw, .. } => redraw,
            ServiceBinding::Editing(services)
            | ServiceBinding::Historical(services)
            | ServiceBinding::ReadOnly(services) => &services.redraw,
        }
    }

    pub(super) fn report_hook_errors(&self, errors: event::HookErrors) {
        match &self.services {
            ServiceBinding::Standalone { .. } => errors.log(),
            ServiceBinding::Editing(services)
            | ServiceBinding::Historical(services)
            | ServiceBinding::ReadOnly(services) => errors.report_to(&services.status),
        }
    }
}
