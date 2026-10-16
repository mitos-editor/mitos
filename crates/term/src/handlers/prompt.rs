use view::events::DocumentFocusLost;

use crate::{job::JobSender, ui};

/// Bind presentation cleanup to this application's editor and callback queue.
pub(super) fn register_hook(editor: &view::Editor, jobs: JobSender) -> event::HookRegistration {
    let callbacks = editor.callback_sender().clone();
    event::register_owned_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if callbacks.same_destination(event.editor.callback_sender()) {
            jobs.send_blocking(move |_, compositor| {
                compositor.remove_type::<ui::Prompt>();
            });
        }
        Ok(())
    })
}
