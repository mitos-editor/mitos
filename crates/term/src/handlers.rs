use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::config::Config;
use crate::events;

pub use view::handlers::{word_index, Handlers};

pub(crate) mod auto_reload;
mod auto_save;
pub mod completion;
mod diagnostics;
mod prompt;
pub(crate) mod signature_help;
mod snippet;
mod workspace_trust;

pub fn setup(
    config: Arc<ArcSwap<Config>>,
    callbacks: view::callbacks::EditorCallbackSender,
) -> Handlers {
    events::register();

    let completions = view::handlers::completion::CompletionHandler::new(
        callbacks.clone(),
        &config.load().editor,
    );
    let signature_hints =
        view::handlers::signature_help::SignatureHelpHandler::new(callbacks.clone());
    let auto_save = view::handlers::auto_save::AutoSaveHandler::new(callbacks.clone());
    let auto_reload = view::handlers::auto_reload::AutoReloadHandler::new(
        callbacks.clone(),
        &config.load().editor,
    );
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
        completions,
        signature_hints,
        auto_save,
        auto_reload,
        word_index,
        pull_diagnostics: view::handlers::diagnostics::pull::PullDiagnosticsHandler::new(
            callbacks.clone(),
        ),
        code_action_hint: view::handlers::code_action_hint::CodeActionHintHandler::new(
            callbacks.clone(),
        ),
        spelling: view::handlers::spelling::SpellingHandler::new(callbacks),
    };

    view::handlers::register_hooks(&handlers);
    completion::register_hooks();
    signature_help::register_hooks();
    view::handlers::document_highlight::register_hooks();
    view::handlers::code_action_hint::register_hooks();
    view::handlers::document_symbols::register_hooks();
    auto_save::register_hooks();
    diagnostics::register_hooks();
    view::handlers::diagnostics::pull::register_hooks();
    snippet::register_hooks(&handlers);
    view::handlers::document_colors::register_hooks();
    view::handlers::document_links::register_hooks();
    prompt::register_hooks(&handlers);
    workspace_trust::register_hooks(&handlers);
    view::handlers::spelling::register_hooks();
    handlers
}
