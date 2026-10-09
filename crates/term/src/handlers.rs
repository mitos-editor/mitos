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
        use crate::events::{PostInsertChar, TerminalFocusGained, TerminalFocusLost};
        event::register_hook!(move |event: &mut PostInsertChar<'_, '_>| {
            event.cx.editor.queue_plugin_event(
                plugin_api::Event::PostInsertChar,
                serde_json::json!({
                    "character": event.c,
                    "source": if event.cx.editor.macro_replaying.is_empty() { "insert-char" } else { "macro" },
                }),
            );
            Ok(())
        });
        event::register_hook!(move |event: &mut TerminalFocusGained<'_, '_>| {
            event.cx.editor.queue_plugin_event(
                plugin_api::Event::TerminalFocusGained,
                serde_json::json!({ "focused": true }),
            );
            Ok(())
        });
        event::register_hook!(move |event: &mut TerminalFocusLost<'_, '_>| {
            event.cx.editor.queue_plugin_event(
                plugin_api::Event::TerminalFocusLost,
                serde_json::json!({ "focused": false }),
            );
            Ok(())
        });
    });
}
