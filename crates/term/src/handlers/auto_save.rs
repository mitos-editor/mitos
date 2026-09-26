use event::register_hook;
use view::document::Mode;

use crate::events::OnModeSwitch;

pub(super) fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
            if event.old_mode == Mode::Insert {
                event.cx.editor.handlers.auto_save.left_insert_mode();
            }
            Ok(())
        });
    });
}
