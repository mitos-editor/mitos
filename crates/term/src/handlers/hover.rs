//! Terminal presentation of a validated shared hover response.
use view::Editor;

use crate::{
    compositor::Compositor,
    ui::{lsp::hover::Hover, Popup},
};

pub fn show(
    editor: &mut Editor,
    compositor: &mut Compositor,
    update: view::features::hover::HoverUpdate,
) {
    let Some(hovers) = update.resolve(editor) else {
        return;
    };
    if hovers.is_empty() {
        editor.set_status("No hover results available.");
        return;
    }
    let contents = Hover::new(hovers, editor.syn_loader.clone());
    let popup = Popup::new(Hover::ID, contents).auto_close(true);
    compositor.replace_or_push(Hover::ID, popup);
}
