//! Terminal scheduling and error presentation for shared formatting operations.

use super::context::Context;
use crate::job::Callback;
use view::features::formatting::{self, FormattingTask};

pub(super) fn format_selections(cx: &mut Context) {
    let view = current_ref!(cx.editor).0.id;
    match formatting::prepare_selections(cx.editor, view) {
        Ok(task) => cx.jobs.callback(format_callback(task)),
        Err(error) => cx.editor.set_error(|| error.to_string()),
    }
}

async fn format_callback(task: FormattingTask) -> anyhow::Result<Callback> {
    let update = task.run().await;
    Ok(Callback::Editor(Box::new(move |editor| {
        if let Err(error) = update.apply(editor) {
            editor.set_error(|| error.to_string());
        }
    })))
}

pub(super) mod typed {
    use super::{format_callback, formatting};
    use crate::{compositor, ui::PromptEvent};
    use command_line::Args;

    pub(in crate::commands) fn format(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }
        let view = current_ref!(cx.editor).0.id;
        let task = formatting::prepare_document(cx.editor, view)?;
        cx.jobs.callback(format_callback(task));
        Ok(())
    }
}
