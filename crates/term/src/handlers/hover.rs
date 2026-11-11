//! Terminal presentation of a validated shared hover response.
use view::{features::hover, Editor};

use crate::{
    compositor::Compositor,
    ui::{self, lsp::hover::Hover, Popup},
};

pub fn show(
    editor: &mut Editor,
    compositor: &mut Compositor,
    update: view::features::hover::HoverUpdate,
) {
    let display = update.display();
    let Some(hovers) = update.resolve(editor) else {
        return;
    };
    if hovers.is_empty() {
        editor.set_status("No hover results available.");
        return;
    }
    match display {
        hover::HoverDisplay::Popup => {
            let contents = Hover::new(hovers, editor.syn_loader.clone());
            let popup = Popup::new(Hover::ID, contents)
                .with_border(crate::ui::PopupBorder::Hover)
                .auto_close(true);
            compositor.replace_or_push(Hover::ID, popup);
        }
        hover::HoverDisplay::Buffer => {
            hover::open_buffer(editor, ui::lsp::hover::buffer_text(hovers));
        }
    }
}
