use super::{
    catalog::{SHELL_COMPLETER, SHELL_SIGNATURE},
    command_line::complete_command_args,
    Context,
};
use crate::{
    compositor,
    ui::{self, PromptEvent},
};
use std::borrow::Cow;
use view::features::shell::{run as run_shell, ShellBehavior};

pub(super) fn shell_pipe(cx: &mut Context) {
    shell_prompt_for_behavior(cx, "pipe:".into(), ShellBehavior::Replace);
}

pub(super) fn shell_pipe_to(cx: &mut Context) {
    shell_prompt_for_behavior(cx, "pipe-to:".into(), ShellBehavior::Ignore);
}

pub(super) fn shell_insert_output(cx: &mut Context) {
    shell_prompt_for_behavior(cx, "insert-output:".into(), ShellBehavior::Insert);
}

pub(super) fn shell_append_output(cx: &mut Context) {
    shell_prompt_for_behavior(cx, "append-output:".into(), ShellBehavior::Append);
}

pub(super) fn shell_keep_pipe(cx: &mut Context) {
    shell_prompt_for_behavior(cx, "keep-pipe:".into(), ShellBehavior::Keep);
}

pub(super) fn shell(cx: &mut compositor::Context, cmd: &str, behavior: ShellBehavior) {
    let view = cx.editor.tree.focus();
    cx.editor.request_shell_filter(view, cmd, behavior);
}

fn shell_prompt_for_behavior(cx: &mut Context, prompt: Cow<'static, str>, behavior: ShellBehavior) {
    ui::prompt(
        cx,
        prompt,
        Some('|'),
        |editor, input| complete_command_args(editor, SHELL_SIGNATURE, &SHELL_COMPLETER, input, 0),
        move |cx, input, event| {
            if event != PromptEvent::Validate || input.is_empty() {
                return;
            }
            if let Err(error) = super::command_line::execute_shell(cx, input, behavior) {
                cx.editor.set_error(|| error.to_string());
            }
        },
    );
}

pub(super) mod typed {
    //! Typable shell commands.

    use crate::{
        commands::shell::{run_shell, shell, ShellBehavior},
        compositor::{self, Compositor},
        job::{self, Callback},
        ui::{self, Popup, PromptEvent},
    };
    use ::command_line::Args;
    use view::Editor;

    #[cold]
    pub(in crate::commands) fn append_output(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        shell(cx, &args.join(" "), ShellBehavior::Append);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn insert_output(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        shell(cx, &args.join(" "), ShellBehavior::Insert);
        Ok(())
    }

    pub(in crate::commands) fn pipe_to(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        pipe_impl(cx, args, event, ShellBehavior::Ignore)
    }

    pub(in crate::commands) fn pipe(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        pipe_impl(cx, args, event, ShellBehavior::Replace)
    }

    #[cold]
    fn pipe_impl(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
        behavior: ShellBehavior,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        shell(cx, &args.join(" "), behavior);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn run_shell_command(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let shell = cx.editor.config().shell.clone();
        let args = args.join(" ");
        let directory = stdx::env::current_working_dir();
        let format_output = cx.editor.config().format_shell_output;

        let callback = async move {
            let output = run_shell(&shell, &args, None, &directory).await?;
            let call: job::Callback = Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, compositor: &mut Compositor| {
                    if !output.trim().is_empty() {
                        let contents = ui::Markdown::new(
                            if format_output {
                                format!("```sh\n{}\n```", output.trim_end())
                            } else {
                                output.trim_end().to_owned()
                            },
                            editor.syn_loader.clone(),
                        );
                        let popup = Popup::new("shell", contents).position(Some(
                            text_layout::DisplayPosition::new(
                                editor.cursor().0.unwrap_or_default().row,
                                2,
                            ),
                        ));
                        compositor.replace_or_push("shell", popup);
                    }
                    editor.set_status("Command run");
                },
            ));
            Ok(call)
        };
        cx.jobs.callback(callback);

        Ok(())
    }
}
