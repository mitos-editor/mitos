//! Forward terminal mode transitions to the shared inline-completion service.
use crate::events::OnModeSwitch;

pub(super) fn register_hooks() {
    event::register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        // Normal-mode cleanup already runs in the shared editor operation.
        if event.new_mode != view::document::Mode::Normal {
            view::features::inline_completion::mode_changed(event.cx.editor, event.old_mode);
        }
        Ok(())
    });
}
