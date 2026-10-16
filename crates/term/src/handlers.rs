//! Terminal input and presentation hooks. Editor services are constructed by `view`.

pub(crate) mod auto_reload;
mod auto_save;
pub mod completion;
mod diagnostics;
mod inline_completion;
mod prompt;
pub(crate) mod signature_help;
pub(crate) mod workspace_trust;

pub fn register_hooks() {
    crate::events::register();
    completion::register_hooks();
    signature_help::register_hooks();
    inline_completion::register_hooks();
    auto_save::register_hooks();
    diagnostics::register_hooks();
}

/// Registrations whose callbacks belong to this application's frontend.
pub(crate) fn bind_editor(
    editor: &view::Editor,
    jobs: crate::job::JobSender,
) -> event::HookRegistration {
    prompt::register_hook(editor, jobs)
}
