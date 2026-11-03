use view::events::DocumentFocusLost;

use crate::{compositor::PromptSessions, job::JobSender};

/// Bind presentation cleanup to this application's editor and callback queue.
pub(super) fn register_hook(
    editor: &view::Editor,
    jobs: JobSender,
    prompts: PromptSessions,
) -> event::HookRegistration {
    event::register_owned_hook!(editor.events() => move |event: &mut DocumentFocusLost<'_>| {
        let sessions = prompts.snapshot();
        if !sessions.is_empty() {
            jobs.send_blocking(move |compositor, _| {
                compositor.remove_prompt_sessions(&sessions);
            });
        }
        Ok(())
    })
}
