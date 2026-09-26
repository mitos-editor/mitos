//! Terminal input and presentation hooks. Editor services are constructed by `view`.

pub(crate) mod auto_reload;
mod auto_save;
pub mod completion;
mod diagnostics;
mod prompt;
pub(crate) mod signature_help;
mod workspace_trust;

pub fn register_hooks() {
    crate::events::register();
    completion::register_hooks();
    signature_help::register_hooks();
    auto_save::register_hooks();
    diagnostics::register_hooks();
    prompt::register_hooks();
    workspace_trust::register_hooks();
}
