use std::sync::Arc;

use arc_swap::ArcSwap;
use event::AsyncHook;

use crate::config::Config;
use crate::events;
use crate::handlers::auto_reload::PollHandler;
use crate::handlers::auto_save::AutoSaveHandler;
use crate::handlers::signature_help::SignatureHelpHandler;

pub use view::handlers::{word_index, Handlers};

pub(crate) mod auto_reload;
mod auto_save;
mod code_action_hint;
pub mod completion;
mod diagnostics;
mod prompt;
mod signature_help;
mod snippet;
mod workspace_trust;

pub fn setup(
    config: Arc<ArcSwap<Config>>,
    callbacks: view::callbacks::EditorCallbackSender,
) -> Handlers {
    events::register();

    let event_tx = completion::CompletionHandler::new(config.clone()).spawn();
    let signature_hints = SignatureHelpHandler::new().spawn();
    let auto_save = AutoSaveHandler::new().spawn();
    let auto_reload = PollHandler::new().spawn();
    let code_action_hint = code_action_hint::Handler::default().spawn();
    let word_index = word_index::Handler::spawn();

    let handlers = Handlers {
        document_symbols: view::handlers::document_symbols::DocumentSymbolsHandler::new(
            callbacks.clone(),
        ),
        document_highlight: view::handlers::document_highlight::DocumentHighlightHandler::new(
            callbacks.clone(),
        ),
        document_links: view::handlers::document_links::DocumentLinksHandler::new(
            callbacks.clone(),
        ),
        document_colors: view::handlers::document_colors::DocumentColorsHandler::new(
            callbacks.clone(),
        ),
        syntax: view::handlers::syntax::SyntaxHandler::new(callbacks.clone()),
        completions: view::handlers::completion::CompletionHandler::new(event_tx),
        signature_hints,
        auto_save,
        auto_reload,
        word_index,
        pull_diagnostics: view::handlers::diagnostics::pull::PullDiagnosticsHandler::new(
            callbacks.clone(),
        ),
        code_action_hint,
        spelling: view::handlers::spelling::SpellingHandler::new(callbacks),
    };

    view::handlers::register_hooks(&handlers);
    completion::register_hooks(&handlers);
    signature_help::register_hooks(&handlers);
    view::handlers::document_highlight::register_hooks();
    code_action_hint::register_hooks(&handlers);
    view::handlers::document_symbols::register_hooks();
    auto_save::register_hooks(&handlers);
    diagnostics::register_hooks();
    view::handlers::diagnostics::pull::register_hooks();
    snippet::register_hooks(&handlers);
    view::handlers::document_colors::register_hooks();
    view::handlers::document_links::register_hooks();
    prompt::register_hooks(&handlers);
    workspace_trust::register_hooks(&handlers);
    view::handlers::spelling::register_hooks();
    auto_reload::register_hooks(&handlers, &config.load().editor);
    handlers
}
