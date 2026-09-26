use crate::{
    commands,
    compositor::Compositor,
    events::{OnModeSwitch, PostCommand, PostInsertChar},
    keymap::MappableCommand,
    ui::{self, editor::InsertEvent, lsp::signature_help::SignatureHelp, Popup},
};
use editor_core::chars::char_is_word;
use event::register_hook;
pub use view::handlers::completion::{trigger_auto_completion, CompletionItem};
use view::{
    document::Mode,
    handlers::completion::{
        request_incomplete_completion_list, CompletionChange, CompletionEvent, CompletionUpdate,
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
        CompletionChange::Started => ui.record_insert_event(InsertEvent::RequestCompletion),
        CompletionChange::Hide => {
            ui.clear_completion(editor);
        }
        CompletionChange::Show {
            items,
            trigger_offset,
        } => {
            let completion_area = ui.set_completion(editor, items, trigger_offset, size);
            if ui.completion.is_none() {
                editor.handlers.completions.dismiss();
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

fn update_completion_filter(cx: &mut commands::Context, c: Option<char>) {
    cx.callback.push(Box::new(move |compositor, cx| {
        let editor_view = compositor.find::<ui::EditorView>().unwrap();
        if let Some(completion) = &mut editor_view.completion {
            completion.update_filter(c);
            if completion.is_empty() || c.is_some_and(|c| !char_is_word(c)) {
                editor_view.clear_completion(cx.editor);
                // clearing completions might mean we want to immediately rerequest them (usually
                // this occurs if typing a trigger char)
                if c.is_some() {
                    trigger_auto_completion(cx.editor, false);
                }
            } else {
                request_incomplete_completion_list(cx.editor)
            }
        }
    }))
}

fn clear_completions(cx: &mut commands::Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        let editor_view = compositor.find::<ui::EditorView>().unwrap();
        editor_view.clear_completion(cx.editor);
    }))
}

fn completion_post_command_hook(
    PostCommand { command, cx }: &mut PostCommand<'_, '_>,
) -> anyhow::Result<()> {
    if cx.editor.mode == Mode::Insert {
        if cx.editor.last_completion.is_some() {
            match command {
                MappableCommand::Static {
                    name: "delete_word_forward" | "delete_char_forward" | "completion",
                    ..
                } => (),
                MappableCommand::Static {
                    name: "delete_char_backward",
                    ..
                } => update_completion_filter(cx, None),
                _ => clear_completions(cx),
            }
        } else {
            let event = match command {
                MappableCommand::Static {
                    name: "delete_char_backward" | "delete_word_forward" | "delete_char_forward",
                    ..
                } => {
                    let (view, doc) = current!(cx.editor);
                    let primary_cursor = doc
                        .selection(view.id)
                        .primary()
                        .cursor(doc.text().slice(..));
                    CompletionEvent::DeleteText {
                        cursor: primary_cursor,
                    }
                }
                // hacks: some commands are handeled elsewhere and we don't want to
                // cancel in that case
                MappableCommand::Static {
                    name: "completion" | "insert_mode" | "append_mode",
                    ..
                } => return Ok(()),
                _ => CompletionEvent::Cancel,
            };
            cx.editor.handlers.completions.event(event);
        }
    }
    Ok(())
}

pub(super) fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut PostCommand<'_, '_>| completion_post_command_hook(event));

        register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
            if event.old_mode == Mode::Insert {
                event
                    .cx
                    .editor
                    .handlers
                    .completions
                    .event(CompletionEvent::Cancel);
                clear_completions(event.cx);
            } else if event.new_mode == Mode::Insert {
                trigger_auto_completion(event.cx.editor, false)
            }
            Ok(())
        });

        register_hook!(move |event: &mut PostInsertChar<'_, '_>| {
            if event.cx.editor.last_completion.is_some() {
                update_completion_filter(event.cx, Some(event.c))
            } else {
                trigger_auto_completion(event.cx.editor, false);
            }
            Ok(())
        });
    });
}
