//! Keyboard access to the complete labels and tooltips of inlay hints.

use super::Context;
use crate::{
    job::{Callback, JobSender},
    ui::{self, Popup, PromptEvent},
};
use stdx::rope::RopeExt as _;
use tui::widgets::Row;
use view::{
    features::inlay_hints::{self, InlayHintItem, InlayHintTarget},
    Editor,
};

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
        show_hint(cx.editor, hints.remove(0).target, jobs);
    } else if hints.is_empty() {
        cx.editor.set_error(|| "No inlay hints on the current line");
    } else {
        let mut menu = ui::Menu::new(hints, (), move |editor, hint, event| {
            if event == PromptEvent::Validate
                && let Some(hint) = hint
            {
                show_hint(editor, hint.target.clone(), jobs.clone());
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

fn show_hint(editor: &mut Editor, target: InlayHintTarget, jobs: JobSender) {
    let Some(future) = inlay_hints::resolve(editor, target) else {
        return;
    };
    tokio::spawn(async move {
        let Some(resolved) = future.await else { return };
        jobs.send_callback(Callback::ui(move |compositor, cx| {
            if !resolved.is_current(cx.editor) {
                return;
            }
            resolved.cache(cx.editor);
            let details = ui::inlay_hints::HintDetails::new(resolved, cx.editor);
            let popup = Popup::new(ui::inlay_hints::HintDetails::ID, details)
                .with_border(ui::PopupBorder::Hover)
                .auto_close(true);
            compositor.replace_or_push(ui::inlay_hints::HintDetails::ID, popup);
        }))
        .await;
    });
}
