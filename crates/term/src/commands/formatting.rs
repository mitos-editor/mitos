//! Terminal scheduling and error presentation for shared formatting operations.

use super::context::Context;
use crate::job::{Callback, Job};
use view::features::formatting::{self, FormattingTask};

pub(super) fn format_selections(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let (view, document) = (view.id, doc.id());
    match formatting::prepare_selections(cx.editor, view) {
        Ok(task) => {
            let activity = cx.editor.start_activity("Formatting", Some(document));
            cx.jobs
                .add(Job::with_callback(format_callback(task)).with_activity(activity));
        }
        Err(error) => cx.editor.set_error(|| error.to_string()),
    }
}

async fn format_callback(task: FormattingTask) -> anyhow::Result<Callback> {
    let update = task.run().await;
    Ok(Callback::editor(move |editor| {
        if let Err(error) = update.apply(editor) {
            editor.set_error(|| error.to_string());
        }
    }))
}

pub(super) mod typed {
    use super::{format_callback, formatting};
    use crate::compositor;
    use crate::job::Job;
    use command_line::Args;

    pub(in crate::commands) fn format(
        cx: &mut compositor::Context,
        _args: Args,
    ) -> anyhow::Result<()> {
        let (view, doc) = current_ref!(cx.editor);
        let (view, document) = (view.id, doc.id());
        let task = formatting::prepare_document(cx.editor, view)?;
        let activity = cx.editor.start_activity("Formatting", Some(document));
        cx.jobs
            .add(Job::with_callback(format_callback(task)).with_activity(activity));
        Ok(())
    }
}
