//! Terminal entry points for editor-owned task execution.

use crate::{
    compositor,
    ui::{completers::CompletionResult, picker::PickerCallbackResult, Picker, PickerColumn},
};
use command_line::Args;
use view::{editor::Action, Editor};

pub(super) fn complete(editor: &Editor, input: &str) -> CompletionResult {
    fuzzy::fuzzy_match(input, editor.config().tasks.keys(), fuzzy::MatchKind::Text)
        .into_iter()
        .map(|(name, _)| ((0..), name.clone().into()))
        .collect()
}

pub(super) fn run(cx: &mut compositor::Context, args: Args) -> anyhow::Result<()> {
    if let Some(name) = args.first() {
        cx.editor.start_task(name)?;
    } else {
        let tasks: Vec<_> = cx
            .editor
            .config()
            .tasks
            .iter()
            .map(|(name, task)| (name.clone(), task.command.clone()))
            .collect();
        anyhow::ensure!(
            !tasks.is_empty(),
            "No tasks configured; add [editor.tasks.build] to config.toml"
        );
        cx.jobs
            .sender()
            .enqueue(crate::job::Callback::ui(move |compositor, cx| {
                let picker = Picker::new_with_callback_result(
                    cx.editor.redraw().clone(),
                    [
                        PickerColumn::new("task", |item: &(String, String), _| {
                            item.0.as_str().into()
                        }),
                        PickerColumn::new("command", |item: &(String, String), _| {
                            item.1.as_str().into()
                        }),
                    ],
                    0,
                    tasks,
                    (),
                    |cx, item, _| {
                        if let Err(error) = cx.editor.start_task(&item.0) {
                            cx.editor.set_error(|| error.to_string());
                        }
                        PickerCallbackResult::Close
                    },
                )
                .with_title("Workspace tasks");
                compositor.push(Box::new(crate::ui::overlay::overlaid(picker)));
            }));
    }
    Ok(())
}

pub(super) fn rerun(cx: &mut compositor::Context, _: Args) -> anyhow::Result<()> {
    cx.editor.rerun_task()?;
    Ok(())
}

pub(super) fn cancel(cx: &mut compositor::Context, _: Args) -> anyhow::Result<()> {
    cx.editor.cancel_task()
}

pub(super) fn output(cx: &mut compositor::Context, _: Args) -> anyhow::Result<()> {
    cx.editor.open_task_output(Action::Replace)?;
    Ok(())
}

pub(super) fn errors(cx: &mut compositor::Context, _: Args) -> anyhow::Result<()> {
    let count = cx.editor.task_errors()?;
    cx.editor
        .set_status(format!("{count} task locations in quicklist"));
    Ok(())
}

pub(super) fn debug_output(cx: &mut compositor::Context, args: Args) -> anyhow::Result<()> {
    if let Some(number) = args.first() {
        cx.editor
            .open_debugger_output_for_session(number.parse()?, Action::Replace)?;
    } else {
        cx.editor.open_debugger_output(Action::Replace)?;
    }
    Ok(())
}

pub(super) fn complete_debug_output(editor: &Editor, input: &str) -> CompletionResult {
    let numbers: Vec<_> = editor
        .debugger()
        .output_sessions()
        .iter()
        .map(|session| session.number.to_string())
        .collect();
    fuzzy::fuzzy_match(input, &numbers, fuzzy::MatchKind::Text)
        .into_iter()
        .map(|(number, _)| ((0..), number.clone().into()))
        .collect()
}
