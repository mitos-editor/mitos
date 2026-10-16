//! Terminal navigation presentation and forwarding of input lifecycle changes.
use event::register_hook;
use view::{
    features::navigation::{self, NavigationUpdate},
    Editor,
};

use crate::{compositor::Compositor, events::OnModeSwitch};

pub fn show(editor: &mut Editor, compositor: &mut Compositor, update: NavigationUpdate) {
    if let Some(locations) = update.resolve(editor) {
        crate::commands::lsp::goto_impl(editor, compositor, locations);
    }
}

pub(super) fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
            if event.new_mode != view::document::Mode::Normal {
                navigation::mode_changed(event.cx.editor, event.old_mode);
            }
            Ok(())
        });
    });
}
