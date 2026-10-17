use view::events::DocumentFocusLost;

use crate::{compositor::PromptSessions, job::JobSender};

/// Bind presentation cleanup to this application's editor and callback queue.
pub(super) fn register_hook(
    editor: &view::Editor,
    jobs: JobSender,
    prompts: PromptSessions,
) -> event::HookRegistration {
    let callbacks = editor.callback_sender().clone();
    event::register_owned_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if callbacks.same_destination(event.editor.callback_sender()) {
            let sessions = prompts.snapshot();
            if !sessions.is_empty() {
                jobs.send_blocking(move |_, compositor| {
                    compositor.remove_prompt_sessions(&sessions);
                });
            }
        }
        Ok(())
    })
}
