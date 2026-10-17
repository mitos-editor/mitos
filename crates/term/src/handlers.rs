//! Terminal input and presentation hooks. Editor services are constructed by `view`.

pub(crate) mod auto_reload;
mod auto_save;
pub mod completion;
mod diagnostics;
pub(crate) mod hover;
mod inline_completion;
pub(crate) mod interactive;
pub(crate) mod navigation;
mod prompt;
pub(crate) mod signature_help;
pub(crate) mod workspace_trust;

/// Install shared presentation hooks once per registry, independent of application count.
pub fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        crate::events::register();
        completion::register_hooks();
        signature_help::register_hooks();
        inline_completion::register_hooks();
        navigation::register_hooks();
        auto_save::register_hooks();
        diagnostics::register_hooks();
    });
}

/// Registrations whose callbacks belong to this application's frontend.
pub(crate) fn bind_editor(
    editor: &view::Editor,
    jobs: crate::job::JobSender,
) -> event::HookRegistration {
    prompt::register_hook(editor, jobs)
}
