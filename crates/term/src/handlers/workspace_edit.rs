//! Present editor-owned annotation consent without retaining protocol or edit state in the UI.

use crate::{compositor::Compositor, ui};
use view::workspace_edit::confirmation::ConfirmationRequest;

#[derive(Clone, Copy)]
enum Choice {
    Cancel,
    Apply,
}

impl crate::ui::menu::Item for Choice {
    type Data = ();
    fn format(&self, _data: &()) -> tui::widgets::Row<'_> {
        tui::widgets::Row::new([match self {
            Self::Cancel => "Cancel",
            Self::Apply => "Apply changes",
        }])
    }
}

pub(crate) fn prompt(
    editor: &mut view::Editor,
    compositor: &mut Compositor,
    request: ConfirmationRequest,
) {
    if !request.is_current(editor) {
        request.resolve(editor, false);
        return;
    }
    let mut message = String::from("Apply these changes?");
    for annotation in request.annotations() {
        message.push_str("\n\n");
        message.push_str(&annotation.label);
        if let Some(description) = &annotation.description {
            message.push('\n');
            message.push_str(description);
        }
    }
    compositor.replace_or_push(
        "workspace-edit-confirmation",
        ui::Select::new(
            message,
            [Choice::Cancel, Choice::Apply],
            (),
            move |editor, choice, event| match event {
                ui::PromptEvent::Update => (),
                ui::PromptEvent::Abort => request.resolve(editor, false),
                ui::PromptEvent::Validate => {
                    request.resolve(editor, matches!(choice, Choice::Apply))
                }
            },
        ),
    );
}
