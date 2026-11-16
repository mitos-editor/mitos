//! Keyboard access to the complete labels and tooltips of inlay hints.

use super::Context;
use crate::{
    job::{Callback, JobSender},
    ui::{self, Popup, PromptEvent},
};
use stdx::rope::RopeExt as _;
use tui::widgets::Row;
use view::features::inlay_hints::{self, InlayHintItem, InlayHintTarget};

impl ui::menu::Item for InlayHintItem {
    type Data = ();
    fn format(&self, _: &()) -> Row<'_> {
        self.label.as_str().into()
    }
}

pub fn inlay_hints(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let line = doc.text().char_to_line_idx(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    );
    let mut hints = inlay_hints::hints_on_line(cx.editor, view.id, line);
    let jobs = cx.jobs.sender();
    if hints.len() == 1 {
        show_hint(hints.remove(0).target, jobs);
    } else if hints.is_empty() {
        cx.editor.set_error(|| "No inlay hints on the current line");
    } else {
        let mut menu = ui::Menu::new(hints, (), move |_, hint, event| {
            if event == PromptEvent::Validate
                && let Some(hint) = hint
            {
                show_hint(hint.target.clone(), jobs.clone());
            }
        });
        menu.move_down();
        cx.push_layer(Box::new(
            Popup::new("inlay-hints", menu)
                .with_border(ui::PopupBorder::Hover)
                .with_scrollbar(false)
                .auto_close(true),
        ));
    }
}

fn show_hint(target: InlayHintTarget, jobs: JobSender) {
    // The selection menu closes before this ready callback installs the loading
    // popup. Ready callbacks are drained before accepting the next input event.
    jobs.enqueue(Callback::ui(move |compositor, cx| {
        ui::inlay_hints::request(compositor, cx, target);
    }));
}
