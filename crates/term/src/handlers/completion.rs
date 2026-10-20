use crate::{
    commands,
    compositor::Compositor,
    events::OnModeSwitch,
    ui::{self, editor::InsertEvent, lsp::signature_help::SignatureHelp, Popup},
};
use editor_core::chars::char_is_word;
use event::register_hook;
use view::{
    document::Mode,
    features::completion::{
        request_incomplete_completion_list, trigger_auto_completion, CompletionChange,
        CompletionUpdate,
    },
    Editor,
};

pub(crate) fn apply_update(
    editor: &mut Editor,
    compositor: &mut Compositor,
    update: CompletionUpdate,
) {
    let Some(change) = update.apply(editor) else {
        return;
    };
    let size = compositor.size();
    let ui = compositor.find::<ui::EditorView>().unwrap();
    match change {
        CompletionChange::Filter { character } => {
            update_completion_filter(editor, compositor, character)
        }
        CompletionChange::Started => ui.record_insert_event(InsertEvent::RequestCompletion),
        CompletionChange::Hide => {
            ui.clear_completion(editor);
        }
        CompletionChange::Show { items, session } => {
            let completion_area = ui.set_completion(editor, items, session, size);
            if ui.completion.is_none() {
                editor.dismiss_completions();
            }
            let signature_help_area = compositor
                .find_id::<Popup<SignatureHelp>>(SignatureHelp::ID)
                .map(|popup| popup.area(size, editor));
            if matches!((completion_area, signature_help_area), (Some(a), Some(b)) if a.intersects(b))
            {
                compositor.remove(SignatureHelp::ID);
            }
        }
        CompletionChange::Provider {
            mut response,
            is_incomplete,
        } => {
            if let Some(completion) = &mut ui.completion {
                completion.replace_provider_completions(&mut response, is_incomplete);
                if completion.is_empty() {
                    ui.clear_completion(editor);
                    trigger_auto_completion(editor, false);
                }
            }
        }
        CompletionChange::Resolved { old, item } => {
            if let Some(completion) = &mut ui.completion {
                completion.replace_item(&*old, *item);
            }
        }
    }
}

fn update_completion_filter(editor: &mut Editor, compositor: &mut Compositor, c: Option<char>) {
    let editor_view = compositor.find::<ui::EditorView>().unwrap();
    if let Some(completion) = &mut editor_view.completion {
        completion.update_filter(c);
        if completion.is_empty() || c.is_some_and(|c| !char_is_word(c)) {
            editor_view.clear_completion(editor);
            if c.is_some() {
                trigger_auto_completion(editor, false);
            }
        } else {
            request_incomplete_completion_list(editor);
        }
    }
}

fn clear_completions(cx: &mut commands::Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        let editor_view = compositor.find::<ui::EditorView>().unwrap();
        editor_view.clear_completion(cx.editor);
    }))
}

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        if event.old_mode == Mode::Insert {
            clear_completions(event.cx);
        }
        Ok(())
    });
}
