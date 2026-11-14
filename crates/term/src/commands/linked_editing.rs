//! Terminal scheduling for explicit linked editing.

use super::context::Context;
use crate::{compositor, job::Callback};
use command_line::Args;
use view::features::linked_editing::{self, LinkedEditingTask};

pub(super) fn linked_editing(cx: &mut Context) {
    let view = current_ref!(cx.editor).0.id;
    match linked_editing::prepare(cx.editor, view) {
        Ok(task) => cx.jobs.callback(apply(task)),
        Err(error) => cx.editor.set_error(|| error.to_string()),
    }
}

async fn apply(task: LinkedEditingTask) -> anyhow::Result<Callback> {
    let update = task.run().await;
    Ok(Callback::editor(move |editor| {
        if let Some(update) = update
            && let Err(error) = update.apply(editor)
        {
            editor.set_error(|| error.to_string());
        }
    }))
}

pub(super) fn start(cx: &mut compositor::Context, _args: Args) -> anyhow::Result<()> {
    let view = current_ref!(cx.editor).0.id;
    cx.jobs
        .callback(apply(linked_editing::prepare(cx.editor, view)?));
    Ok(())
}
