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
    prompt::register_hooks();
    event::runtime_local! { static PLUGIN_HOOKS: std::sync::Once = std::sync::Once::new(); }
    PLUGIN_HOOKS.call_once(|| {
        use crate::events::{OnModeSwitch, PostCommand};
        event::register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
            event.cx.editor.queue_plugin_event(
                plugin_sdk::Event::ModeChanged,
                serde_json::json!({
                    "old-mode": event.old_mode.to_string(), "new-mode": event.new_mode.to_string()
                }),
            );
            Ok(())
        });
        event::register_hook!(move |event: &mut PostCommand<'_, '_>| {
            event.cx.editor.queue_plugin_event(
                plugin_sdk::Event::PostCommand,
                serde_json::json!({
                    "command": event.command.name()
                }),
            );
            Ok(())
        });
    });
}
