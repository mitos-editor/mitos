//! Terminal presentation of a validated shared hover response.
use editor_core::Rope;
use view::{editor::Action, features::hover::HoverDisplay, Document, Editor};

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
        HoverDisplay::Popup => {
            let contents = Hover::new(hovers, editor.syn_loader.clone());
            let popup = Popup::new(Hover::ID, contents)
                .with_border(crate::ui::PopupBorder::Hover)
                .auto_close(true);
            compositor.replace_or_push(Hover::ID, popup);
        }
        HoverDisplay::Buffer => open_buffer(editor, ui::lsp::hover::buffer_text(hovers)),
    }
}

/// Replace the requesting view with a Markdown scratch buffer of the hover text.
/// The jump list keeps the source position, so `C-o` returns to it.
fn open_buffer(editor: &mut Editor, text: String) {
    let doc = Document::from(
        Rope::from(text),
        None,
        editor.config.clone(),
        editor.syn_loader.load_full(),
    );
    let id = editor.new_file_from_document(Action::Replace, doc);
    // Without a Markdown grammar the buffer stays plain text, which is still useful.
    if let Err(err) = editor.set_document_language(id, Some("markdown")) {
        log::debug!("goto_hover buffer stays plain text: {err}");
    }
}
