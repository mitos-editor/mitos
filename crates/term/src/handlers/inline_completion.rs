//! Forward terminal mode transitions to the shared inline-completion service.
use crate::events::OnModeSwitch;

pub(super) fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        event::register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
            // Normal-mode cleanup already runs in the shared editor operation.
            if event.new_mode != view::document::Mode::Normal {
                view::handlers::inline_completion::mode_changed(event.cx.editor, event.old_mode);
            }
            Ok(())
        });
    });
}
